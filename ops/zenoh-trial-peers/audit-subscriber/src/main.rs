// SPDX-License-Identifier: Apache-2.0
//
// goat-audit-subscriber — external Zenoh audit subscriber.
//
// Replaces the in-process audit-logger plugin per:
//   - docs/design/zenoh-audit-external-subscriber.md
//   - ADR 0858 (zenoh image upstream pivot enabled by this move)
//   - ADR 0811 (audit integrity with environment-owned retention)
//
// Shape:
//   - Opens an mTLS Zenoh session as a client peer of the audited router.
//   - Declares a subscriber on the configured key-expression (default **).
//   - Forwards each sample through a bounded mpsc channel to a writer
//     task that batches events into NDJSON objects and ships them to a
//     MinIO bucket (S3-compatible).
//   - Exposes Prometheus /metrics on a separate HTTP port.
//
// Audit metadata contains no body. Replay capture is gated pending governance.
// SourceInfo remains an unverified publisher claim.
mod records;
mod attribution;
mod state;
mod writer;
#[cfg(test)]
use records::AuditEvent;
use records::ReplayEvent;

use anyhow::{Context, Result};
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use chrono::{SecondsFormat, Utc};
use clap::Parser;
use prometheus::{
    register_counter_vec_with_registry, register_gauge_vec_with_registry,
    register_int_counter_with_registry, register_int_gauge_with_registry, CounterVec, Encoder,
    GaugeVec, IntCounter, IntGauge, Registry, TextEncoder,
};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use zenoh::key_expr::KeyExpr;

#[derive(Parser, Debug)]
#[command(
    name = "goat-audit-subscriber",
    about = "External Zenoh audit subscriber. Block 82S per docs/design/zenoh-audit-external-subscriber.md."
)]
struct Args {
    /// Zenoh router endpoint, e.g. `tls/zenoh-router:7447`.
    #[arg(long, env = "AUDIT_SUBSCRIBER_ENDPOINT")]
    endpoint: String,

    #[arg(long, env = "AUDIT_SUBSCRIBER_RECEIVING_SITE")]
    receiving_site: String,
    /// Rendered router configuration; evidence of intent, not live attestation.
    #[arg(long, env = "AUDIT_SUBSCRIBER_ROUTER_POLICY")]
    router_policy: Option<std::path::PathBuf>,
    #[arg(long, env = "AUDIT_SUBSCRIBER_S3_CA_BUNDLE")]
    s3_ca_bundle: Option<std::path::PathBuf>,

    /// Key-expression to subscribe to. Default `**` captures every
    /// sample the router routes to this client; narrow per-prefix when
    /// the trial fabric ACL gets tighter.
    #[arg(long, env = "AUDIT_SUBSCRIBER_KEY", default_value = "**")]
    subscribe_key: String,

    /// Comma-separated key-expressions to EXCLUDE from the audit /
    /// persistence path. Samples whose key intersects any of these are
    /// counted in `goat_audit_excluded_total` and dropped before hashing
    /// and upload — they never reach the MinIO bucket. The subscription
    /// stays `**` (future-proof against new top-level classes); the
    /// filter is applied client-side. Default `volatile/**` per ADR 1032:
    /// the non-persisted class (high-throughput telemetry + bounded
    /// large-file transfer) must not fill the locked audit store. Set to
    /// an empty string to capture everything (legacy behaviour).
    #[arg(
        long,
        env = "AUDIT_SUBSCRIBER_EXCLUDE_KEYS",
        default_value = "volatile/**"
    )]
    exclude_keys: String,

    /// Stable identifier for this subscriber's writes (e.g. `site-a`).
    /// Becomes a path component in MinIO object keys + a label on
    /// emitted metrics.
    #[arg(long, env = "AUDIT_SUBSCRIBER_ID")]
    audit_subscriber_id: String,

    /// Identity model of the audited goatnet (ADR 1025). `per-vendor`
    /// (the first key segment is a human vendor prefix) or
    /// `per-uuid-slot-pool` (the first segment is an opaque slot
    /// anchor uuid). Recorded verbatim on every audit event as
    /// `identity_model` so a downstream reader knows whether the
    /// `anchor`/`vendor` field is a human name or a uuid to be joined
    /// against the slot registry's assigned handle. Default `per-vendor`
    /// for compatibility.
    #[arg(
        long,
        env = "AUDIT_SUBSCRIBER_IDENTITY_MODEL",
        default_value = "per-vendor"
    )]
    identity_model: String,

    /// Path to the per-instance leaf X.509 certificate (PEM) for mTLS.
    /// Required when `endpoint` starts with `tls/`.
    #[arg(long, env = "AUDIT_SUBSCRIBER_CERT")]
    cert: Option<String>,

    /// Path to the matching private key (PEM). Required when
    /// `endpoint` starts with `tls/`.
    #[arg(long, env = "AUDIT_SUBSCRIBER_KEY_FILE")]
    key: Option<String>,

    /// Path to the CA root certificate (PEM) used to verify the
    /// router's server cert. Required when `endpoint` starts with
    /// `tls/`.
    #[arg(long, env = "AUDIT_SUBSCRIBER_ROOT_CA")]
    root_ca: Option<String>,

    /// Bind address for the Prometheus /metrics endpoint.
    #[arg(
        long,
        env = "AUDIT_SUBSCRIBER_METRICS_BIND",
        default_value = "0.0.0.0:9303"
    )]
    metrics_bind: SocketAddr,

    /// MinIO / S3 endpoint URL (e.g. `https://minio.site-a.local:9000`).
    /// When set, audit events are uploaded as NDJSON batches; when
    /// unset, the subscriber runs in observe-only mode (metrics still
    /// emit, no durable write — useful for first-deploy verification).
    #[arg(long, env = "AUDIT_SUBSCRIBER_S3_ENDPOINT")]
    s3_endpoint: Option<String>,

    /// MinIO / S3 bucket name. Per ADR 0811 the production target is a
    /// bucket with Object-Lock COMPLIANCE 1y.
    #[arg(
        long,
        env = "AUDIT_SUBSCRIBER_S3_BUCKET",
        default_value = "audit-integrity"
    )]
    s3_bucket: String,

    /// MinIO / S3 region. MinIO ignores the value but the SDK requires
    /// one to construct the client.
    #[arg(long, env = "AUDIT_SUBSCRIBER_S3_REGION", default_value = "us-east-1")]
    s3_region: String,

    /// Environment variable name holding the S3 access key. The value
    /// is read from the env at startup so the secret never appears in
    /// process listings.
    #[arg(
        long,
        env = "AUDIT_SUBSCRIBER_S3_ACCESS_KEY_ENV",
        default_value = "AUDIT_SUBSCRIBER_S3_ACCESS_KEY"
    )]
    s3_access_key_env: String,

    /// Environment variable name holding the S3 secret key.
    #[arg(
        long,
        env = "AUDIT_SUBSCRIBER_S3_SECRET_KEY_ENV",
        default_value = "AUDIT_SUBSCRIBER_S3_SECRET_KEY"
    )]
    s3_secret_key_env: String,

    /// Replay-only payload capture cap. Audit records never contain bodies. Publishes
    /// larger than this carry hash + length only. Default 16 KiB per
    /// the design-doc open question (revisit when we have trial-fabric
    /// payload-size distribution evidence).
    #[arg(
        long,
        env = "AUDIT_SUBSCRIBER_MAX_INLINE_PAYLOAD",
        default_value_t = 16384
    )]
    max_inline_payload: usize,

    /// Durable directory holding the subscriber lock and batch high-water mark.
    #[arg(long, env = "AUDIT_SUBSCRIBER_STATE_DIR")]
    state_dir: std::path::PathBuf,

    /// Initialize a NEW state directory above the final legacy batch timestamp,
    /// then exit. Never resets existing state. Use 0 only for an empty stream.
    #[arg(long)]
    initialize_state_after: Option<i64>,

    /// Separate replay bucket on the same S3 endpoint. Unset disables capture.
    #[arg(long, env = "AUDIT_SUBSCRIBER_REPLAY_BUCKET")]
    replay_bucket: Option<String>,
    #[arg(
        long,
        env = "AUDIT_SUBSCRIBER_REPLAY_ACCESS_KEY_ENV",
        default_value = "AUDIT_SUBSCRIBER_REPLAY_ACCESS_KEY"
    )]
    replay_access_key_env: String,
    #[arg(
        long,
        env = "AUDIT_SUBSCRIBER_REPLAY_SECRET_KEY_ENV",
        default_value = "AUDIT_SUBSCRIBER_REPLAY_SECRET_KEY"
    )]
    replay_secret_key_env: String,

    /// Maximum events per uploaded NDJSON batch.
    ///
    /// This is a BURST VALVE, not the normal flush trigger. It must sit well
    /// above `rate x batch_interval_ms` or it binds first and the interval
    /// becomes decorative — that is exactly what 256 did against a 5 s
    /// interval at tens of events/s. 10000 leaves headroom over that
    /// rate and bounds writer memory at about 6.5 MB held.
    #[arg(long, env = "AUDIT_SUBSCRIBER_BATCH_EVENTS", default_value_t = 10_000)]
    batch_events: usize,

    /// Maximum batch flush interval. The writer flushes when either
    /// `batch_events` accumulates OR this much time elapses since the
    /// batch started, whichever fires first.
    ///
    /// ONE FLUSH IS ONE S3 OBJECT, and MinIO's scanner reads one `xl.meta`
    /// per object on every pass — so this value, not the byte volume, sets
    /// the store's scan cost. A 5 s cadence accumulates hundreds of thousands of
    /// objects and millions of inodes on the host root filesystem. At 60 s the
    /// same bytes land in about a twelfth as many objects.
    ///
    /// IT IS ALSO THE DATA-LOSS WINDOW. Between flushes the batch is in this
    /// process's memory and nowhere else — the local NDJSON tail is appended
    /// inside `flush_batch`, not continuously — so a crash loses up to this
    /// much audit outright, not merely un-WORMed. 60 s was chosen by the
    /// operator with that stated.
    #[arg(
        long,
        env = "AUDIT_SUBSCRIBER_BATCH_INTERVAL_MS",
        default_value_t = 60_000
    )]
    batch_interval_ms: u64,

    /// Optional local directory to ALSO append each NDJSON batch to (a
    /// best-effort tail — the durable MinIO write is unaffected on failure).
    /// Off-box cyber-logging (goat-logship, ADR 1078) tails this to surface
    /// fabric writes as OCSF; the MinIO WORM store stays the system-of-record.
    #[arg(long, env = "AUDIT_SUBSCRIBER_LOCAL_NDJSON_DIR")]
    local_ndjson_dir: Option<String>,

    /// Bounded channel depth between subscriber and writer. Sized for
    /// burst tolerance; if the writer falls behind by more than this,
    /// `goat_audit_drop_total` increments and the publish is dropped
    /// from the audit log (fail-loud, not fail-silent).
    #[arg(long, env = "AUDIT_SUBSCRIBER_CHANNEL_DEPTH", default_value_t = 4096)]
    channel_depth: usize,
}

/// The two identity models an audited goatnet can run (ADR 1025).
const VALID_IDENTITY_MODELS: [&str; 2] = ["per-vendor", "per-uuid-slot-pool"];

/// Backoff between zenoh session-open attempts. Connection refused at
/// boot (router not up yet) is the common case; 5s keeps the audit gap
/// small without hammering the router.
const SESSION_OPEN_RETRY: Duration = Duration::from_secs(5);

#[derive(Clone)]
struct Metrics {
    samples_total: CounterVec,
    sample_bytes: CounterVec,
    excluded_total: CounterVec,
    write_errors_total: IntCounter,
    drop_total: IntCounter,
    batches_uploaded_total: IntCounter,
    batch_size_events: GaugeVec,
    session_state: IntGauge,
    replay_drop_total: IntCounter,
    replay_write_errors_total: IntCounter,
    replay_batches_uploaded_total: IntCounter,
    local_write_errors_total: IntCounter,
    registry: Registry,
}

impl Metrics {
    fn new(audit_subscriber_id: &str) -> Result<Self> {
        let registry = Registry::new();
        let labels = [("audit_subscriber_id", audit_subscriber_id)];
        let const_labels: std::collections::HashMap<String, String> = labels
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();

        let samples_total = register_counter_vec_with_registry!(
            prometheus::Opts::new(
                "goat_audit_samples_total",
                "Audit samples observed by this subscriber, by vendor + key-pattern."
            )
            .const_labels(const_labels.clone()),
            &["vendor", "key_pattern"],
            registry
        )?;
        let sample_bytes = register_counter_vec_with_registry!(
            prometheus::Opts::new(
                "goat_audit_sample_bytes_total",
                "Total payload bytes observed, by vendor."
            )
            .const_labels(const_labels.clone()),
            &["vendor"],
            registry
        )?;
        let excluded_total = register_counter_vec_with_registry!(
            prometheus::Opts::new(
                "goat_audit_excluded_total",
                "Samples deliberately dropped before the persistence path \
                 because their key matched an --exclude-key pattern (e.g. \
                 the non-persisted volatile/** class, ADR 1032). Makes the \
                 audit exemption observable rather than silent."
            )
            .const_labels(const_labels.clone()),
            &["exclude_pattern"],
            registry
        )?;
        let write_errors_total = register_int_counter_with_registry!(
            prometheus::Opts::new(
                "goat_audit_write_errors_total",
                "S3 PutObject failures (post-retry)."
            )
            .const_labels(const_labels.clone()),
            registry
        )?;
        let drop_total = register_int_counter_with_registry!(
            prometheus::Opts::new(
                "goat_audit_drop_total",
                "Audit samples lost to queue saturation or failed upload."
            )
            .const_labels(const_labels.clone()),
            registry
        )?;
        let batches_uploaded_total = register_int_counter_with_registry!(
            prometheus::Opts::new(
                "goat_audit_batches_uploaded_total",
                "NDJSON batches successfully uploaded to S3."
            )
            .const_labels(const_labels.clone()),
            registry
        )?;
        let batch_size_events = register_gauge_vec_with_registry!(
            prometheus::Opts::new(
                "goat_audit_batch_size_events",
                "Event count of the most recently uploaded batch."
            )
            .const_labels(const_labels.clone()),
            &["outcome"],
            registry
        )?;
        let session_state = register_int_gauge_with_registry!(
            prometheus::Opts::new(
                "goat_audit_subscriber_session_state",
                "1 when the zenoh session declared its subscriber, 0 otherwise."
            )
            .const_labels(const_labels.clone()),
            registry
        )?;

        let counter = |name: &str, help: &str| -> Result<IntCounter> {
            Ok(register_int_counter_with_registry!(
                prometheus::Opts::new(name, help).const_labels(const_labels.clone()),
                registry
            )?)
        };
        let replay_drop_total = counter(
            "goat_replay_drop_total",
            "Replay records lost to queue saturation or failed upload.",
        )?;
        let replay_write_errors_total = counter(
            "goat_replay_write_errors_total",
            "Replay uploads failed after SDK retries.",
        )?;
        let replay_batches_uploaded_total = counter(
            "goat_replay_batches_uploaded_total",
            "Replay batches uploaded.",
        )?;
        let local_write_errors_total = counter(
            "goat_audit_local_write_errors_total",
            "Metadata local-tail append failures.",
        )?;

        Ok(Self {
            samples_total,
            sample_bytes,
            excluded_total,
            write_errors_total,
            drop_total,
            batches_uploaded_total,
            batch_size_events,
            session_state,
            replay_drop_total,
            replay_write_errors_total,
            replay_batches_uploaded_total,
            local_write_errors_total,
            registry,
        })
    }
}

#[tokio::main]
// This standalone I/O supervisor has no relay shard affinity (ADR 0709).
#[allow(clippy::disallowed_methods)]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    anyhow::ensure!(args.replay_bucket.is_none(), "replay capture is gated pending owner governance");
    anyhow::ensure!(!args.receiving_site.trim().is_empty(), "receiving site is required");
    let policy = attribution::Policy::load(args.router_policy.as_deref())?;

    // Validate the identity model up front (ADR 1025) — a typo here would
    // mislabel every audit record's identity_model, so fail loud at startup.
    if !VALID_IDENTITY_MODELS.contains(&args.identity_model.as_str()) {
        anyhow::bail!(
            "--identity-model {:?} invalid; must be one of {:?}",
            args.identity_model,
            VALID_IDENTITY_MODELS
        );
    }

    anyhow::ensure!(
        args.batch_events > 0 && args.batch_interval_ms > 0 && args.channel_depth > 0,
        "batch and channel bounds must be positive"
    );
    if let Some(bucket) = &args.replay_bucket {
        anyhow::ensure!(
            !bucket.is_empty() && bucket != &args.s3_bucket,
            "replay bucket must be nonempty and different from audit bucket"
        );
        anyhow::ensure!(args.s3_endpoint.is_some(), "replay requires an S3 endpoint");
    }
    let clock = state::BatchClock::open(
        &args.state_dir,
        &args.audit_subscriber_id,
        args.initialize_state_after,
    )?;
    if args.initialize_state_after.is_some() {
        info!("batch state initialized; no subscription started");
        return Ok(());
    }
    let target =
        |bucket: &str, access_env: &str, secret_env: &str| -> Result<Option<writer::Target>> {
            args.s3_endpoint
                .as_ref()
                .map(|endpoint| {
                    Ok(writer::Target {
                        endpoint: endpoint.clone(),
                        ca_bundle: Some(args.s3_ca_bundle.clone().context("S3 CA bundle required")?),
                        bucket: bucket.to_string(),
                        region: args.s3_region.clone(),
                        access: read_env_opt(access_env).context("S3 access key missing")?,
                        secret: read_env_opt(secret_env).context("S3 secret key missing")?,
                    })
                })
                .transpose()
        };
    let writer_args = writer::WriterArgs {
        audit: target(
            &args.s3_bucket,
            &args.s3_access_key_env,
            &args.s3_secret_key_env,
        )?,
        replay: match &args.replay_bucket {
            Some(bucket) => target(
                bucket,
                &args.replay_access_key_env,
                &args.replay_secret_key_env,
            )?,
            None => None,
        },
        subscriber: args.audit_subscriber_id.clone(),
        batch_events: args.batch_events,
        batch_interval: Duration::from_millis(args.batch_interval_ms),
        local_dir: args.local_ndjson_dir.as_ref().map(Into::into),
    };
    if let (Some(audit), Some(replay)) = (&writer_args.audit, &writer_args.replay) {
        anyhow::ensure!(
            audit.access != replay.access,
            "audit and replay require separate S3 identities"
        );
    }
    let metrics = Metrics::new(&args.audit_subscriber_id)?;

    // Serve /metrics. Must be ready before zenoh open so Prometheus
    // scraping picks up the `session_state=0` pre-connect signal.
    let metrics_for_http = metrics.clone();
    let metrics_bind = args.metrics_bind;
    tokio::spawn(async move {
        if let Err(e) = serve_metrics(metrics_for_http, metrics_bind).await {
            error!(error = %e, "metrics server exited");
        }
    });

    let (tx, rx) = mpsc::channel::<ReplayEvent>(args.channel_depth);

    // Writer task — owns the S3 client (or none, for observe-only mode).
    let writer_metrics = metrics.clone();
    let mut writer_handle = tokio::spawn(writer::run(writer_args, writer_metrics, rx, clock));

    // Parse the audit exclude filter (ADR 1032). Done up front so a
    // malformed key-expression fails the process at startup rather than
    // silently capturing the class we meant to drop.
    let exclude_kes = parse_exclude_keys(&args.exclude_keys)?;
    if exclude_kes.is_empty() {
        info!("audit exclude filter: none (capturing all routed keys)");
    } else {
        let patterns: Vec<&str> = exclude_kes.iter().map(|k| k.as_str()).collect();
        info!(
            ?patterns,
            "audit exclude filter active — matching samples dropped before persistence"
        );
    }

    // Subscriber loop — opens the zenoh session and forwards samples.
    let subscriber_metrics = metrics.clone();
    let subscriber_args = SubscriberArgs {
        endpoint: args.endpoint.clone(),
        subscribe_key: args.subscribe_key.clone(),
        cert: args.cert.clone(),
        key: args.key.clone(),
        root_ca: args.root_ca.clone(),
        audit_subscriber_id: args.audit_subscriber_id.clone(),
        identity_model: args.identity_model.clone(),
        receiving_site: args.receiving_site.clone(),
        policy,
        max_inline_payload: args.max_inline_payload,
        replay: args.replay_bucket.is_some(),
        exclude_kes,
    };
    let mut subscriber_handle =
        tokio::spawn(run_subscriber(subscriber_args, subscriber_metrics, tx));
    tokio::select! {
        r = shutdown_signal() => {
            r?;
            subscriber_handle.abort();
            let _ = subscriber_handle.await;
            tokio::time::timeout(Duration::from_secs(60), &mut writer_handle).await
                .context("writers did not drain before shutdown deadline")?
                .context("writer task panicked")??;
            Ok(())
        }
        r = &mut writer_handle => {
            subscriber_handle.abort();
            r.context("writer task panicked")??;
            anyhow::bail!("writer task ended unexpectedly")
        }
        r = &mut subscriber_handle => {
            let drain = tokio::time::timeout(Duration::from_secs(60), &mut writer_handle).await;
            if let Err(e) = drain { error!(error=%e, "writer drain timed out"); }
            r.context("subscriber task panicked")??;
            anyhow::bail!("subscriber task ended unexpectedly")
        }
    }
}

async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .context("install SIGTERM handler")?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.context("install SIGINT handler")?,
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .context("install shutdown handler")?;
    Ok(())
}

fn read_env_opt(var_name: &str) -> Option<String> {
    std::env::var(var_name).ok().filter(|v| !v.is_empty())
}

/// Parse the comma-separated `--exclude-keys` string into owned
/// key-expressions. Empty / whitespace-only entries are ignored, so
/// `""` yields no filter (capture everything). A malformed entry is a
/// hard startup error — better to fail loud than silently capture a
/// class an operator meant to exclude.
fn parse_exclude_keys(raw: &str) -> Result<Vec<KeyExpr<'static>>> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            KeyExpr::new(s.to_string())
                .map(|ke| ke.into_owned())
                .map_err(|e| anyhow::anyhow!("invalid --exclude-keys entry {s:?}: {e}"))
        })
        .collect()
}

async fn serve_metrics(metrics: Metrics, bind: SocketAddr) -> Result<()> {
    let registry = Arc::new(metrics.registry);
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
                let body = String::from_utf8(buf).unwrap_or_default();
                (StatusCode::OK, body)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind(bind).await?;
    info!(%bind, "metrics listening");
    axum::serve(listener, app).await?;
    Ok(())
}

struct SubscriberArgs {
    endpoint: String,
    subscribe_key: String,
    cert: Option<String>,
    key: Option<String>,
    root_ca: Option<String>,
    audit_subscriber_id: String,
    identity_model: String,
    receiving_site: String,
    policy: attribution::Policy,
    max_inline_payload: usize,
    replay: bool,
    exclude_kes: Vec<KeyExpr<'static>>,
}

async fn run_subscriber(
    args: SubscriberArgs,
    metrics: Metrics,
    tx: mpsc::Sender<ReplayEvent>,
) -> Result<()> {
    // Retry the initial open forever: at host boot the router routinely
    // comes up after us, and a one-shot open turned that ordering into a
    // dead subscriber. `session_state` stays 0 while retrying, so the
    // outage window is visible on the existing metric. Once open, zenoh
    // handles link re-establishment itself.
    let session = loop {
        match open_session(&args).await {
            Ok(s) => break s,
            Err(e) => {
                warn!(
                    error = %e,
                    retry_in_s = SESSION_OPEN_RETRY.as_secs(),
                    "zenoh session open failed; retrying"
                );
                tokio::time::sleep(SESSION_OPEN_RETRY).await;
            }
        }
    };
    info!(
        zid = %session.zid(),
        endpoint = args.endpoint.as_str(),
        subscribe_key = args.subscribe_key.as_str(),
        "session open",
    );

    let subscriber = session
        .declare_subscriber(args.subscribe_key.clone())
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "declare_subscriber({}) failed: {e}",
                args.subscribe_key.as_str()
            )
        })?;
    metrics.session_state.set(1);

    // Per-producer audit rate meter (GHSA-hq6c): counts and alerts when one
    // producer exceeds its budget; drops only when enforcement is opted in.
    // See quota.rs.
    let mut producer_quota = quota::ProducerQuota::new(
        quota::Limits::from_env(),
        Some(&metrics.registry),
        &args.audit_subscriber_id,
    );

    loop {
        let sample = match subscriber.recv_async().await {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "subscriber recv error; loop ending");
                metrics.session_state.set(0);
                break;
            }
        };

        // Audit exclude filter (ADR 1032): drop the non-persisted classes
        // (default volatile/**) before any hashing / channel / upload
        // work, so the high-throughput firehose never reaches the locked
        // MinIO bucket. Counted per matched pattern so the exemption is
        // observable rather than silent.
        if let Some(pat) = args
            .exclude_kes
            .iter()
            .find(|ke| ke.intersects(sample.key_expr()))
        {
            metrics
                .excluded_total
                .with_label_values(&[pat.as_str()])
                .inc();
            continue;
        }

        // Before any hashing, queueing or upload: meter the sample against
        // its producer's rate budget (producer derived from the key per the
        // router ACL). Observe-only by default; only when enforcement is on
        // is an over-budget sample refused (never ops/audit/**). See quota.rs.
        if !producer_quota.admit(
            sample.key_expr().as_str(),
            sample.payload().len(),
            args.max_inline_payload,
        ) {
            continue;
        }

        let key_expr = sample.key_expr().to_string();
        // The enforced write-anchor is the first key segment in BOTH identity
        // models (ADR 1025): a vendor prefix under per-vendor, an opaque slot
        // uuid under per-uuid-slot-pool. The `vendor` field/label keeps this
        // value for back-compat; `identity_model` disambiguates downstream.
        let anchor = anchor_of_key(&key_expr).to_string();
        let key_pattern = key_pattern_label(&key_expr);
        let payload_bytes = sample.payload().to_bytes();
        let payload_len = payload_bytes.len();
        let source = sample
            .source_info()
            .map(|si| si.source_id().zid().to_string());
        let event = records::record(
            &key_expr,
            &payload_bytes,
            source,
            Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            records::Capture {
                sample_kind: match sample.kind() { zenoh::sample::SampleKind::Put => "put", zenoh::sample::SampleKind::Delete => "delete" },
                receiving_site: &args.receiving_site,
                policy: &args.policy,
                subscriber: &args.audit_subscriber_id,
                identity_model: &args.identity_model,
                cap: args.max_inline_payload,
                replay: args.replay,
            },
        );

        metrics
            .samples_total
            .with_label_values(&[&anchor, &key_pattern])
            .inc();
        metrics
            .sample_bytes
            .with_label_values(&[&anchor])
            .inc_by(payload_len as f64);

        match tx.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                metrics.drop_total.inc();
                producer_quota.note_queue_drop(&key_expr);
                warn!(
                    key_expr = key_expr.as_str(),
                    "writer channel full; dropping sample"
                );
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                warn!("writer channel closed; subscriber stopping");
                break;
            }
        }
    }
    Ok(())
}

mod quota;

/// The enforced write-anchor of a key-expression: its first `/`-segment.
/// This is the ACL-bound identity in BOTH identity models (ADR 1025) — a
/// vendor prefix under `per-vendor`, a slot-anchor uuid under
/// `per-uuid-slot-pool`. The taxonomy (ADR 0844) nests verbatim below it,
/// so this single rule serves both shapes.
fn anchor_of_key(key_expr: &str) -> &str {
    key_expr.split('/').next().unwrap_or("")
}

/// Reduce a key-expression to a stable label suitable for a Prometheus
/// `key_pattern` dimension. Replaces all but the first two path
/// segments with `*` so cardinality stays bounded.
fn key_pattern_label(key_expr: &str) -> String {
    let segments: Vec<&str> = key_expr.split('/').collect();
    match segments.len() {
        0 => String::new(),
        1 => segments[0].to_string(),
        _ => format!("{}/{}/*", segments[0], segments[1]),
    }
}

async fn open_session(args: &SubscriberArgs) -> Result<zenoh::Session> {
    let mut config = zenoh::Config::default();
    config
        .insert_json5("mode", "\"client\"")
        .map_err(|e| anyhow::anyhow!("config mode: {e}"))?;
    config
        .insert_json5("connect/endpoints", &format!("[\"{}\"]", args.endpoint))
        .map_err(|e| anyhow::anyhow!("config connect/endpoints: {e}"))?;
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .map_err(|e| anyhow::anyhow!("config scouting: {e}"))?;
    if args.endpoint.starts_with("tls/") {
        let cert = args
            .cert
            .as_ref()
            .context("--cert required for tls/ endpoint")?;
        let key = args
            .key
            .as_ref()
            .context("--key required for tls/ endpoint")?;
        let root_ca = args
            .root_ca
            .as_ref()
            .context("--root-ca required for tls/ endpoint")?;
        // Same shape as synth-vendor + panoscope: TLS is selected by
        // the endpoint scheme; turn off hostname verification because
        // the router cert SAN binds the mesh IP, not the docker DNS
        // name we connect through.
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

#[cfg(test)]
mod s3_tls_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[allow(clippy::disallowed_methods)] // Zenoh requires block_in_place, incompatible with LocalSet.
    async fn loopback_capture_preserves_assertions_and_excludes_volatile() {
        let mut config = zenoh::Config::default();
        config.insert_json5("mode", "\"router\"").unwrap();
        config
            .insert_json5("listen/endpoints", "[\"tcp/127.0.0.1:0\"]")
            .unwrap();
        config
            .insert_json5("scouting/multicast/enabled", "false")
            .unwrap();
        let session = zenoh::open(config).await.unwrap();
        let endpoint = session.info().locators().await[0].to_string();
        let metrics = Metrics::new("site").unwrap();
        let (tx, mut rx) = mpsc::channel(8);
        let args = SubscriberArgs {
            endpoint,
            subscribe_key: "**".into(),
            cert: None,
            key: None,
            root_ca: None,
            audit_subscriber_id: "site".into(),
            identity_model: "per-vendor".into(),
            receiving_site: "fixture".into(),
            policy: attribution::Policy::default(),
            max_inline_payload: 16384,
            replay: true,
            exclude_kes: parse_exclude_keys("volatile/**").unwrap(),
        };
        {
            let task = tokio::spawn(run_subscriber(args, metrics.clone(), tx));
            let publisher = session.declare_publisher("vendor/topic").await.unwrap();
            tokio::time::timeout(Duration::from_secs(10), async {
                while !publisher.matching_status().await.unwrap().matching() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            publisher.put("no-source").await.unwrap();
            let absent = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(absent.audit.source_provenance, "unavailable");
            publisher
                .put("asserted-source")
                .source_info(zenoh::sample::SourceInfo::new(publisher.id(), 7))
                .await
                .unwrap();
            let asserted = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(asserted.audit.src_zid, publisher.id().zid().to_string());
            assert_eq!(asserted.audit.source_provenance, "publisher_asserted");
            session
                .put("volatile/slot/data", "excluded-secret")
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                while metrics
                    .excluded_total
                    .with_label_values(&["volatile/**"])
                    .get()
                    == 0.0
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert!(rx.try_recv().is_err());
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        }
        session.close().await.unwrap();
    }

    // The anchor is the first segment in BOTH identity models — this is what
    // makes the audit path dual-shape without branching on the model.
    #[test]
    fn anchor_is_first_segment_in_both_shapes() {
        // per-vendor: human prefix.
        assert_eq!(anchor_of_key("vendor-b/sensorgrid/track/v1"), "vendor-b");
        // per-uuid-slot-pool: opaque slot anchor uuid + nested taxonomy.
        assert_eq!(
            anchor_of_key("61dc1e84ca2c7cfea827728a0f4bfa4d/data/track/site/v1"),
            "61dc1e84ca2c7cfea827728a0f4bfa4d"
        );
        assert_eq!(anchor_of_key("solo"), "solo");
        assert_eq!(anchor_of_key(""), "");
    }

    #[test]
    fn key_pattern_label_bounds_cardinality() {
        assert_eq!(key_pattern_label("v/s/a/b/c"), "v/s/*");
        assert_eq!(key_pattern_label("v/s"), "v/s/*");
        assert_eq!(key_pattern_label("v"), "v");
        assert_eq!(key_pattern_label(""), "");
    }

    // The volatile exclusion must hold for a uuid anchor exactly as for a
    // vendor prefix: volatile/<anchor>/** is dropped, the persisted
    // <anchor>/** namespace is kept (ADR 1032 + ADR 1025).
    #[test]
    fn volatile_exclusion_matches_uuid_anchor_but_not_persisted() {
        let kes = parse_exclude_keys("volatile/**").unwrap();
        let vol = KeyExpr::new("volatile/61dc1e84ca2c7cfea827728a0f4bfa4d/stream/x").unwrap();
        assert!(
            kes.iter().any(|e| e.intersects(&vol)),
            "volatile/<uuid>/** must be excluded from audit"
        );
        let persisted = KeyExpr::new("61dc1e84ca2c7cfea827728a0f4bfa4d/data/track/v1").unwrap();
        assert!(
            !kes.iter().any(|e| e.intersects(&persisted)),
            "persisted <uuid>/** must NOT be excluded"
        );
    }

    #[test]
    fn identity_model_set_is_the_two_adr_1025_shapes() {
        assert!(VALID_IDENTITY_MODELS.contains(&"per-vendor"));
        assert!(VALID_IDENTITY_MODELS.contains(&"per-uuid-slot-pool"));
        assert!(!VALID_IDENTITY_MODELS.contains(&"per-org"));
    }

    // The audit record keeps `vendor` as a value-identical alias of `anchor`
    // so the portal tailer (which reads json:"vendor") is unaffected, while
    // new consumers can read `anchor` + `identity_model`.
    #[test]
    fn event_vendor_aliases_anchor_and_serializes_model() {
        let anchor = anchor_of_key("61dc1e84ca2c7cfea827728a0f4bfa4d/data/x/v1").to_string();
        let ev = AuditEvent {
            sample_kind: "put".into(), receiving_site: "fixture".into(), outcome: "observed".into(), attribution: Default::default(),
            schema_version: 2,
            event_id: "batch#0".into(),
            source_provenance: "unavailable",
            payload_capture: "disabled",
            onboarding_classification: "other",
            anchor: anchor.clone(),
            vendor: anchor.clone(),
            identity_model: "per-uuid-slot-pool".to_string(),
            key_expr: "61dc1e84ca2c7cfea827728a0f4bfa4d/data/x/v1".to_string(),
            ts: "2026-06-08T00:00:00.000Z".to_string(),
            src_zid: String::new(),
            payload_hash: "sha256:00".to_string(),
            payload_len: 0,
            audit_subscriber_id: "zagros".to_string(),
        };
        let j: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&ev).unwrap()).unwrap();
        assert_eq!(j["anchor"], j["vendor"]);
        assert_eq!(j["identity_model"], "per-uuid-slot-pool");
    }
}
