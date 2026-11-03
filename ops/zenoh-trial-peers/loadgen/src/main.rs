// SPDX-License-Identifier: Apache-2.0
//
// goat-loadgen — Zenoh fabric load generator for the load-testing apparatus.
//
// See:
//   - docs/design/zenoh-loadtest.md (apparatus design + pre-registered predictions)
//   - docs/adr/1012-load-testing-discipline.md (the discipline this serves)
//
// What it does:
//   - Opens N mTLS Zenoh client sessions (distinct ZIDs) to the REAL router.
//   - Each session pumps PUTs into the dedicated `loadtest/<run-id>/<seq>`
//     namespace at a controlled aggregate rate, for a fixed duration.
//   - Every message stamps {seq, t_ns} in BOTH the key (the trailing seq) and
//     the JSON payload, so the rocksdb GET-back probe and the MinIO list probe
//     can measure completeness (distinct stored seq / published seq) and
//     publish->storage lag without ambiguity.
//   - Exposes Prometheus /metrics (sent / errors / put-latency histogram) so
//     the orchestrator builds the throughput + put-latency timeline by scraping
//     once per second. The put-latency series is the non-blocking proof: held
//     against storage-lag, flat == decoupled, rising-in-lockstep == backpressure.
//
// What it deliberately does NOT do:
//   - Touch any real vendor prefix. It only ever writes under `--key-prefix`
//     (default `loadtest`). The ACL leaf cert it carries is scoped to that
//     prefix, so a misconfigured prefix fails closed at the router.
//   - Decide where data is wiped. Teardown is ops/loadtest/wipe-loadtest.sh.
//
// Mirrors the synth-vendor session/mTLS plumbing and the audit-subscriber
// axum+prometheus metrics surface in this same directory — no new substrate.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use clap::{Parser, ValueEnum};
use prometheus::{
    register_histogram_with_registry, register_int_counter_with_registry, Encoder, Histogram,
    IntCounter, Registry, TextEncoder,
};
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::time::{sleep_until, Instant as TokioInstant};
use tracing::{error, info, warn};

#[derive(Parser, Debug, Clone)]
#[command(
    name = "goat-loadgen",
    about = "Zenoh fabric load generator. Per docs/design/zenoh-loadtest.md + ADR 1012."
)]
struct Args {
    /// Zenoh router endpoint, e.g. `tls/goat-zenoh-router:7447`.
    #[arg(long, env = "LOADGEN_ENDPOINT")]
    endpoint: String,

    /// Base key prefix. Keys are `<key-prefix>/<run-id>/<seq>`. MUST stay the
    /// dedicated loadtest namespace — the leaf cert ACL is scoped to it.
    #[arg(long, env = "LOADGEN_KEY_PREFIX", default_value = "loadtest")]
    key_prefix: String,

    /// Run identifier; isolates this run's keyspace and bucket objects so the
    /// probes and wipe only ever address one run. Pass a fresh id per run.
    #[arg(long, env = "LOADGEN_RUN_ID")]
    run_id: String,

    /// Target AGGREGATE publish rate across all sessions, msgs/sec.
    /// 0 = unbounded (pump as fast as the sessions can — route-only ceiling).
    #[arg(long, env = "LOADGEN_RATE", default_value_t = 0.0)]
    rate: f64,

    /// Total payload size in bytes (JSON header + 'a' padding to this size).
    /// Header alone is the floor; smaller values clamp up to the header size.
    #[arg(long, env = "LOADGEN_PAYLOAD_BYTES", default_value_t = 100)]
    payload_bytes: usize,

    /// Run duration in seconds. 0 = until SIGTERM/Ctrl-C (watchdog abort).
    #[arg(long, env = "LOADGEN_DURATION_SECS", default_value_t = 60)]
    duration_secs: u64,

    /// Concurrent publisher sessions (distinct ZIDs). >1 needed to saturate
    /// cores for the route-only ceiling. Aggregate rate is split evenly.
    #[arg(long, env = "LOADGEN_SESSIONS", default_value_t = 1)]
    sessions: usize,

    /// Pipeline depth: max puts kept concurrently in flight PER SESSION before
    /// the next admission blocks. This is what makes the generator measure
    /// throughput rather than latency. The old serial `put().await` loop kept
    /// exactly 1 in flight, so with `Block` + zenoh's shallow (2-batch) tx queue
    /// the offered rate collapsed to 1/RTT (far below what the fabric routes) — the publisher,
    /// not the fabric, was the bottleneck (M-053 RCA; zenoh's own
    /// z_pub_thr does ~1.2M/s routed on the same host). With a window of N,
    /// `Block` backpressures on the *window* (honest delivered throughput, no
    /// silent drops); `Drop` sheds beyond the tx queue. Only the unbounded
    /// (`--rate 0`) ceiling run needs depth; paced runs stay near 1 in flight.
    #[arg(long, env = "LOADGEN_PIPELINE_DEPTH", default_value_t = 256)]
    pipeline_depth: usize,

    /// Per-put congestion control. `block` (default) makes put() reflect real
    /// transport backpressure (true delivered throughput + honest put-latency);
    /// `drop` lets the publisher never block (raw offered-load probing).
    #[arg(long, env = "LOADGEN_CONGESTION", value_enum, default_value_t = Congestion::Block)]
    congestion: Congestion,

    /// Bind address for the Prometheus /metrics endpoint.
    #[arg(long, env = "LOADGEN_METRICS_BIND", default_value = "0.0.0.0:9304")]
    metrics_bind: SocketAddr,

    /// mTLS leaf cert (PEM). Required for `tls/` endpoints.
    #[arg(long, env = "LOADGEN_CERT")]
    cert: Option<String>,

    /// mTLS private key (PEM). Required for `tls/` endpoints.
    #[arg(long, env = "LOADGEN_KEY")]
    key: Option<String>,

    /// CA root (PEM) to verify the router cert. Required for `tls/` endpoints.
    #[arg(long, env = "LOADGEN_ROOT_CA")]
    root_ca: Option<String>,

    /// Seconds between stdout JSONL progress lines (0 = none). /metrics is
    /// always live regardless; this is for human-tailing the container log.
    #[arg(long, env = "LOADGEN_PROGRESS_SECS", default_value_t = 5)]
    progress_secs: u64,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Congestion {
    Block,
    Drop,
}

struct Metrics {
    registry: Registry,
    sent_total: IntCounter,
    send_errors_total: IntCounter,
    put_latency_seconds: Histogram,
}

impl Metrics {
    fn new() -> Result<Self> {
        let registry = Registry::new();
        let sent_total = register_int_counter_with_registry!(
            "loadgen_sent_total",
            "Total messages successfully put to the router.",
            registry
        )?;
        let send_errors_total = register_int_counter_with_registry!(
            "loadgen_send_errors_total",
            "Total put() calls that returned an error.",
            registry
        )?;
        // Buckets span ~10us (loopback put) to ~1s (saturated/backpressured),
        // so the non-blocking proof can resolve both the flat and the
        // rising-in-lockstep regimes.
        let put_latency_seconds = register_histogram_with_registry!(
            "loadgen_put_latency_seconds",
            "Wall time of each put() call, seconds.",
            vec![
                0.000_010, 0.000_025, 0.000_050, 0.000_100, 0.000_250, 0.000_500, 0.001, 0.0025, // oss-allow: histogram buckets
                0.005, 0.010, 0.025, 0.050, 0.100, 0.250, 0.500, 1.0, 2.5 // oss-allow: histogram buckets
            ],
            registry
        )?;
        Ok(Self {
            registry,
            sent_total,
            send_errors_total,
            put_latency_seconds,
        })
    }
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Build a payload of exactly `target` bytes (or the header floor if larger),
/// carrying seq + publish-time so the probes can join against the stored copy.
/// Hand-rolled (not serde) so the padding math hits the target size exactly.
fn make_payload(seq: u64, run: &str, t_ns: u128, target: usize) -> Vec<u8> {
    let head = format!("{{\"seq\":{seq},\"run\":\"{run}\",\"t_ns\":{t_ns},\"pad\":\"");
    let tail = "\"}";
    let pad_len = target.saturating_sub(head.len() + tail.len());
    let mut s = String::with_capacity(head.len() + pad_len + tail.len());
    s.push_str(&head);
    s.extend(std::iter::repeat('a').take(pad_len));
    s.push_str(tail);
    s.into_bytes()
}

async fn open_session(args: &Args) -> Result<zenoh::Session> {
    let mut config = zenoh::Config::default();
    config
        .insert_json5("mode", "\"client\"")
        .map_err(|e| anyhow::anyhow!("config mode: {e}"))?;
    config
        .insert_json5("connect/endpoints", &format!("[\"{}\"]", args.endpoint))
        .map_err(|e| anyhow::anyhow!("config connect/endpoints: {e}"))?;
    if args.endpoint.starts_with("tls/") {
        let cert = args.cert.as_ref().context("--cert required for tls/ endpoint")?;
        let key = args.key.as_ref().context("--key required for tls/ endpoint")?;
        let root_ca = args
            .root_ca
            .as_ref()
            .context("--root-ca required for tls/ endpoint")?;
        // Same TLS block synth-vendor uses: TLS selected by the `tls/` scheme;
        // hostname verification off because the router cert SAN binds the mesh
        // IP, not the docker DNS name we dial.
        let tls_block = format!(
            r#"{{ "root_ca_certificate": "{root_ca}", "connect_certificate": "{cert}", "connect_private_key": "{key}", "enable_mtls": true, "verify_name_on_connect": false }}"#
        );
        config
            .insert_json5("transport/link/tls", &tls_block)
            .map_err(|e| anyhow::anyhow!("config transport/link/tls: {e}"))?;
    }
    zenoh::open(config)
        .await
        .map_err(|e| anyhow::anyhow!("zenoh::open failed: {e}"))
        .context("opening zenoh session")
}

#[allow(clippy::too_many_arguments)]
async fn run_session(
    idx: usize,
    args: Args,
    metrics: Arc<Metrics>,
    seq_counter: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let session = open_session(&args).await?;
    info!(session = idx, zid = %session.zid(), "session open");

    // Per-session pacing: split the aggregate target evenly. 0 == unbounded.
    let per_session_rate = if args.rate > 0.0 {
        args.rate / args.sessions as f64
    } else {
        0.0
    };
    let period = if per_session_rate > 0.0 {
        Some(Duration::from_secs_f64(1.0 / per_session_rate))
    } else {
        None
    };
    let mut next = TokioInstant::now();
    let depth = args.pipeline_depth.max(1);
    let congestion = args.congestion;
    let session_ref = &session;

    // Bounded window of in-flight puts. Admitting a new put blocks once `depth`
    // are outstanding; draining records each completion. Keeping the window full
    // keeps zenoh's tx pipeline full, so the measured rate is throughput, not
    // 1/RTT. The old loop awaited every put serially (exactly 1 in flight): with
    // `Block` + zenoh's shallow (2-batch) tx queue that pinned the offered rate
    // far below what the fabric routes.
    let mut inflight = FuturesUnordered::new();

    loop {
        let stopping = stop.load(Ordering::Relaxed);
        if stopping && inflight.is_empty() {
            break;
        }

        // Admit up to the pipeline depth. Paced mode admits one per period then
        // drains (rate stays controlled, depth stays ~1). Unbounded mode fills
        // the window so the pipe stays full and we measure the real ceiling.
        while !stopping && inflight.len() < depth {
            if let Some(p) = period {
                sleep_until(next).await;
                next += p;
            }
            let seq = seq_counter.fetch_add(1, Ordering::Relaxed);
            let key = format!("{}/{}/{}", args.key_prefix, args.run_id, seq);
            let payload = make_payload(seq, &args.run_id, unix_nanos(), args.payload_bytes);
            let started = Instant::now();
            inflight.push(async move {
                let cc = match congestion {
                    Congestion::Block => zenoh::qos::CongestionControl::Block,
                    Congestion::Drop => zenoh::qos::CongestionControl::Drop,
                };
                let res = session_ref
                    .put(key, payload)
                    .congestion_control(cc)
                    .await;
                (res, started.elapsed())
            });
            if period.is_some() {
                break; // paced: one admission per loop iteration, then drain
            }
        }

        // Make progress by awaiting the next completion. When the window is full
        // this is the only await, so it is where `Block` backpressure is felt.
        if let Some((res, elapsed)) = inflight.next().await {
            match res {
                Ok(()) => {
                    metrics.put_latency_seconds.observe(elapsed.as_secs_f64());
                    metrics.sent_total.inc();
                }
                Err(e) => {
                    metrics.send_errors_total.inc();
                    // Don't spam: a saturated router can error every put. Log sparsely.
                    if metrics.send_errors_total.get() % 1000 == 1 {
                        error!(session = idx, error = %format!("{e}"), "put failed (sampled 1/1000)");
                    }
                }
            }
        } else if stopping {
            break;
        }
    }
    info!(session = idx, "session stopping");
    Ok(())
}

async fn serve_metrics(registry: Arc<Registry>, bind: SocketAddr) -> Result<()> {
    let app = Router::new().route(
        "/metrics",
        get(move || {
            let registry = registry.clone();
            async move {
                let metric_families = registry.gather();
                let mut buf = Vec::new();
                let encoder = TextEncoder::new();
                if let Err(e) = encoder.encode(&metric_families, &mut buf) {
                    return (StatusCode::INTERNAL_SERVER_ERROR, format!("encode: {e}"));
                }
                (StatusCode::OK, String::from_utf8(buf).unwrap_or_default())
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind(bind).await?;
    info!(%bind, "metrics listening");
    axum::serve(listener, app).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    if args.sessions == 0 {
        anyhow::bail!("--sessions must be >= 1");
    }
    info!(
        endpoint = %args.endpoint,
        key_prefix = %args.key_prefix,
        run_id = %args.run_id,
        rate = args.rate,
        payload_bytes = args.payload_bytes,
        duration_secs = args.duration_secs,
        sessions = args.sessions,
        "goat-loadgen starting"
    );

    let metrics = Arc::new(Metrics::new()?);
    let seq_counter = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));

    // Metrics server (background). It must serve the SAME registry the counters
    // live on; prometheus::Registry is Arc-backed so the clone is a shared handle.
    let metrics_registry = Arc::new(clone_registry_handle(&metrics));
    tokio::spawn({
        let bind = args.metrics_bind;
        async move {
            if let Err(e) = serve_metrics(metrics_registry, bind).await {
                error!(error = %format!("{e}"), "metrics server exited");
            }
        }
    });

    // Publisher sessions.
    let mut handles = Vec::with_capacity(args.sessions);
    for idx in 0..args.sessions {
        let h = tokio::spawn(run_session(
            idx,
            args.clone(),
            metrics.clone(),
            seq_counter.clone(),
            stop.clone(),
        ));
        handles.push(h);
    }

    // Optional stdout progress for human log-tailing.
    let progress = if args.progress_secs > 0 {
        let metrics = metrics.clone();
        let stop = stop.clone();
        let every = args.progress_secs;
        Some(tokio::spawn(async move {
            let mut last_sent = 0u64;
            let mut ticker = tokio::time::interval(Duration::from_secs(every));
            loop {
                ticker.tick().await;
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let sent = metrics.sent_total.get();
                let errs = metrics.send_errors_total.get();
                let rate = (sent - last_sent) as f64 / every as f64;
                last_sent = sent;
                println!(
                    "{{\"ts_ns\":{},\"sent\":{},\"errors\":{},\"rate_msg_s\":{:.1}}}",
                    unix_nanos(),
                    sent,
                    errs,
                    rate
                );
            }
        }))
    } else {
        None
    };

    // Stop on duration OR signal (watchdog abort sends SIGTERM).
    let dur = args.duration_secs;
    tokio::select! {
        _ = async { if dur > 0 { tokio::time::sleep(Duration::from_secs(dur)).await } else { std::future::pending::<()>().await } } => {
            info!("duration elapsed");
        }
        _ = tokio::signal::ctrl_c() => { warn!("SIGINT — stopping"); }
        _ = sigterm() => { warn!("SIGTERM — stopping (watchdog abort or operator)"); }
    }
    stop.store(true, Ordering::Relaxed);

    for h in handles {
        let _ = h.await;
    }
    if let Some(p) = progress {
        let _ = p.await;
    }

    let sent = metrics.sent_total.get();
    let errs = metrics.send_errors_total.get();
    println!(
        "{{\"event\":\"final\",\"ts_ns\":{},\"run_id\":\"{}\",\"sent\":{},\"errors\":{},\"sessions\":{},\"rate_target\":{}}}",
        unix_nanos(),
        args.run_id,
        sent,
        errs,
        args.sessions,
        args.rate
    );
    info!(sent, errs, "goat-loadgen done");
    Ok(())
}

#[cfg(unix)]
async fn sigterm() {
    use tokio::signal::unix::{signal, SignalKind};
    if let Ok(mut s) = signal(SignalKind::terminate()) {
        s.recv().await;
    } else {
        std::future::pending::<()>().await;
    }
}

#[cfg(not(unix))]
async fn sigterm() {
    std::future::pending::<()>().await;
}

/// The Registry behind a Metrics. prometheus::Registry is internally an Arc, so
/// `.clone()` yields another handle to the SAME registry — gathering it returns
/// the live counter values. (Used so the metrics server and the counters share
/// one registry without moving the Metrics struct.)
fn clone_registry_handle(metrics: &Metrics) -> Registry {
    metrics.registry.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_hits_target_size() {
        let p = make_payload(42, "run-x", 1_700_000_000_000_000_000u128, 200);
        assert_eq!(p.len(), 200);
        let v: serde_json::Value = serde_json::from_slice(&p).unwrap();
        assert_eq!(v["seq"], 42);
        assert_eq!(v["run"], "run-x");
    }

    #[test]
    fn payload_clamps_up_to_header_floor() {
        // target smaller than the header: result is the header+tail, still valid JSON.
        let p = make_payload(1, "r", 1u128, 1);
        let v: serde_json::Value = serde_json::from_slice(&p).unwrap();
        assert_eq!(v["seq"], 1);
        assert_eq!(v["pad"], "");
    }

    #[test]
    fn seq_and_time_are_parseable_by_probes() {
        // The probes parse seq + t_ns out of the payload; lock the contract.
        let p = make_payload(999, "abc", 123u128, 100);
        let v: serde_json::Value = serde_json::from_slice(&p).unwrap();
        assert_eq!(v["seq"].as_u64(), Some(999));
        assert_eq!(v["t_ns"].as_u64(), Some(123));
    }
}
