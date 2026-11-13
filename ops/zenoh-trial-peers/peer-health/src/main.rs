// SPDX-License-Identifier: Apache-2.0
//
// goat-peer-health binary: flags, the log input thread, the Zenoh publisher and
// the tick loop. Everything that decides anything is in the library (lib.rs).

use anyhow::{Context, Result};
use clap::Parser;
use goat_peer_health::clock;
use goat_peer_health::detector::{Config, Detector};
use goat_peer_health::follow::{read_stream, FileFollower, Input};
use goat_peer_health::{load_sessions, publish_reading, Attribution, Publisher};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};
use zenoh::qos::{CongestionControl, Priority};

#[derive(Parser, Debug)]
#[command(
    name = "goat-peer-health",
    about = "Peer domain of the environment-health surface — per-peer clock faults read from a Zenoh router's log (ADR 1232)"
)]
#[command(group(clap::ArgGroup::new("input").required(true).args(["stdin", "log_file"])))]
struct Args {
    /// Read the router log from standard input
    /// (`docker logs -f --since 1m <router> 2>&1 | goat-peer-health --stdin ...`).
    #[arg(long)]
    stdin: bool,

    /// Follow a router log file like `tail -F` (survives rotation and truncation).
    #[arg(long, env = "PEER_HEALTH_LOG_FILE")]
    log_file: Option<PathBuf>,

    /// With --log-file: read the file from its start instead of its end.
    #[arg(long, env = "PEER_HEALTH_FROM_START")]
    from_start: bool,

    /// Zenoh router endpoint, e.g. `tls/goat-zenoh-router:7447`. Required unless --dry-run.
    #[arg(long, env = "PEER_HEALTH_ENDPOINT")]
    endpoint: Option<String>,

    /// Site identifier — the `<site>` segment of ops/peer/health/<site>.
    #[arg(long, env = "PEER_HEALTH_SITE")]
    site: String,

    /// Leaf X.509 cert (PEM) for mTLS. Required for a `tls/` endpoint.
    #[arg(long, env = "PEER_HEALTH_CERT")]
    cert: Option<String>,
    /// Matching private key (PEM).
    #[arg(long, env = "PEER_HEALTH_KEY_FILE")]
    key: Option<String>,
    /// CA root cert (PEM) used to verify the router.
    #[arg(long, env = "PEER_HEALTH_ROOT_CA")]
    root_ca: Option<String>,

    /// Publish interval (ms).
    #[arg(long, env = "PEER_HEALTH_INTERVAL_MS", default_value_t = 30_000)]
    interval_ms: u64,

    /// Sliding window for rejections per minute and min/max offset (s).
    #[arg(long, env = "PEER_HEALTH_WINDOW_SECS", default_value_t = 60)]
    window_secs: u64,

    /// A peer with no rejection for this long is reported once as cleared (s).
    #[arg(long, env = "PEER_HEALTH_CLEAN_AFTER_SECS", default_value_t = 600)]
    clean_after_secs: u64,

    /// No log line of any kind for this long makes the reading unmeasured (s).
    #[arg(long, env = "PEER_HEALTH_SILENCE_AFTER_SECS", default_value_t = 900)]
    silence_after_secs: u64,

    /// Declared number of peers this router serves: the denominator of the
    /// "most peers show the same offset" rule. 0 = only the peers seen.
    #[arg(long, env = "PEER_HEALTH_PEER_POPULATION", default_value_t = 0)]
    peer_population: u32,

    /// JSON map from Zenoh ID (or source address) to {"name", "owner"}.
    #[arg(long, env = "PEER_HEALTH_PEERS_FILE")]
    peers_file: Option<PathBuf>,

    /// JSON list of the Zenoh IDs holding a session with our router, re-read
    /// every tick. Fills direct_session per peer; absent = unknown.
    #[arg(long, env = "PEER_HEALTH_SESSIONS_FILE")]
    sessions_file: Option<PathBuf>,

    /// Name of the one peering this reader serves (beside a per-partner edge
    /// router). Every rejection is attributed to it; per-ID rows become detail.
    #[arg(long, env = "PEER_HEALTH_PEERING")]
    peering: Option<String>,

    /// Our router's Zenoh ID, when the log does not carry its start-up line.
    #[arg(long, env = "PEER_HEALTH_OBSERVED_BY")]
    observed_by: Option<String>,

    /// Local clock check: auto (the kernel, adjtimex), synced or unsynced.
    #[arg(long, env = "PEER_HEALTH_LOCAL_CLOCK", default_value = "auto")]
    local_clock: String,

    /// Print what would be published as JSON lines on stdout; open no session.
    #[arg(long)]
    dry_run: bool,
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

async fn open_session(args: &Args) -> Result<zenoh::Session> {
    let endpoint = args
        .endpoint
        .as_ref()
        .context("--endpoint required unless --dry-run")?;
    let mut config = zenoh::Config::default();
    config
        .insert_json5("mode", "\"client\"")
        .map_err(|e| anyhow::anyhow!("config mode: {e}"))?;
    config
        .insert_json5("connect/endpoints", &format!("[\"{endpoint}\"]"))
        .map_err(|e| anyhow::anyhow!("config connect/endpoints: {e}"))?;
    if endpoint.starts_with("tls/") {
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
}

/// Puts on the fabric in C0 (Block + high priority), like goat-time-health.
struct ZenohPublisher(Arc<zenoh::Session>);

impl Publisher for ZenohPublisher {
    fn put(&mut self, key: &str, payload: Vec<u8>) -> impl Future<Output = Result<()>> + Send {
        let session = self.0.clone();
        let key = key.to_string();
        async move {
            session
                .put(&key, payload)
                .congestion_control(CongestionControl::Block)
                .priority(Priority::InteractiveHigh)
                .await
                .map_err(|e| anyhow::anyhow!("put {key}: {e}"))
        }
    }
}

/// A readable JSON rendering of the protobuf envelope, for --dry-run.
fn envelope_summary(key: &str, payload: &[u8]) -> serde_json::Value {
    use goat_peer_health::wire::{EnvDetail, EnvHealthV1};
    use prost::Message as _;
    let Ok(env) = EnvHealthV1::decode(payload) else {
        return serde_json::json!({ "key": key, "error": "undecodable envelope" });
    };
    let Some(EnvDetail::Peer(d)) = env.detail else {
        return serde_json::json!({ "key": key, "overall_color": env.overall_color });
    };
    let peers: Vec<_> = d
        .peers
        .iter()
        .map(|p| {
            serde_json::json!({
                "zid": p.zid, "grade": p.grade, "color": p.color,
                "offset_seconds": p.offset_seconds, "min_offset_seconds": p.min_offset_seconds,
                "max_offset_seconds": p.max_offset_seconds, "limit_seconds": p.limit_seconds,
                "rejections_per_minute": p.rejections_per_minute,
                "first_seen_unix_ms": p.first_seen_unix_ms, "last_seen_unix_ms": p.last_seen_unix_ms,
                "observed_by": p.observed_by, "direct_session": p.direct_session,
                "attribution_name": p.attribution_name, "attribution_owner": p.attribution_owner,
            })
        })
        .collect();
    serde_json::json!({
        "key": key, "domain": env.domain, "scope": env.scope, "overall_color": env.overall_color,
        "window_secs": env.window_secs, "verdict": d.verdict, "verdict_reason": d.verdict_reason,
        "local_clock": d.local_clock, "observed_by": d.observed_by, "peers_known": d.peers_known,
        "last_line_age_ms": d.last_line_age_ms, "unparsed_lines_window": d.unparsed_lines_window,
        "peering": d.peering, "peering_rejections_per_minute": d.peering_rejections_per_minute,
        "peering_max_offset_seconds": d.peering_max_offset_seconds, "peers": peers,
    })
}

/// --dry-run: one JSON line per put on stdout.
struct StdoutPublisher;

impl Publisher for StdoutPublisher {
    fn put(&mut self, key: &str, payload: Vec<u8>) -> impl Future<Output = Result<()>> + Send {
        let line = if key.starts_with(goat_peer_health::HEALTH_PREFIX) {
            envelope_summary(key, &payload).to_string()
        } else {
            let doc: serde_json::Value = serde_json::from_slice(&payload).unwrap_or_default();
            serde_json::json!({ "key": key, "value": doc }).to_string()
        };
        println!("{line}");
        std::future::ready(Ok(()))
    }
}

fn spawn_input(args: &Args, tx: tokio::sync::mpsc::UnboundedSender<Input>) {
    if let Some(path) = args.log_file.clone() {
        let from_start = args.from_start;
        std::thread::spawn(move || {
            FileFollower::new(path, from_start)
                .run(Duration::from_millis(250), |i| tx.send(i).is_ok());
        });
    } else {
        std::thread::spawn(move || {
            read_stream(std::io::stdin().lock(), |i| tx.send(i).is_ok());
        });
    }
}

async fn run<P: Publisher>(args: &Args, mut publisher: P) -> Result<()> {
    let attribution = match &args.peers_file {
        Some(p) => Attribution::load(p)?,
        None => Attribution::default(),
    };
    let clock_check = clock::from_mode(&args.local_clock).map_err(anyhow::Error::msg)?;
    let cfg = Config {
        site: args.site.clone(),
        window_ms: (args.window_secs.max(1) * 1000) as i64,
        clean_after_ms: (args.clean_after_secs * 1000) as i64,
        silence_after_ms: (args.silence_after_secs * 1000) as i64,
        peer_population: args.peer_population,
        observed_by: args.observed_by.clone(),
        peering: args.peering.clone().filter(|p| !p.is_empty()),
    };
    info!(
        site = args.site.as_str(),
        peers_file_entries = attribution.len(),
        local_clock = %clock_check.check(),
        "reading the router log; publishing ops/peer/health/<site> and ops/peer/verdict/<zid>"
    );
    let mut det = Detector::new(cfg, attribution);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    spawn_input(args, tx);
    let mut input_open = true;

    let mut ticker = tokio::time::interval(Duration::from_millis(args.interval_ms.max(1)));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await; // consume the immediate first tick
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("install SIGTERM handler")?;

    loop {
        tokio::select! {
            got = rx.recv(), if input_open => match got {
                Some(Input::Line(l)) => det.ingest(&l, now_unix_ms()),
                Some(Input::Closed) | None => {
                    warn!("log input ended; the reading is now unmeasured");
                    det.mark_input_closed();
                    input_open = false;
                }
            },
            _ = ticker.tick() => {
                if let Some(path) = &args.sessions_file {
                    match load_sessions(path) {
                        Ok(z) => det.set_sessions(Some(z)),
                        Err(e) => { warn!(error = %e, "sessions unknown this tick"); det.set_sessions(None); }
                    }
                }
                let reading = det.tick(now_unix_ms(), &clock_check.check());
                let d = reading.detail();
                let (verdict, peers, overall) = (d.verdict.clone(), d.peers.len(), reading.envelope.overall_color.clone());
                match publish_reading(&mut publisher, &args.site, &reading).await {
                    Ok(()) => info!(verdict = verdict.as_str(), overall = overall.as_str(), peers, verdicts = reading.verdicts.len(), "published peer health"),
                    Err(e) => warn!(error = %e, "publish peer health failed"),
                }
            }
            _ = sigterm.recv() => { info!("SIGTERM received; shutting down"); break; }
            _ = tokio::signal::ctrl_c() => { info!("SIGINT received; shutting down"); break; }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    if args.dry_run {
        return run(&args, StdoutPublisher).await;
    }
    let session = Arc::new(open_session(&args).await.context("open zenoh session")?);
    info!(zid = %session.zid(), endpoint = args.endpoint.as_deref().unwrap_or(""), "session open");
    let result = run(&args, ZenohPublisher(session.clone())).await;
    let _ = session.close().await;
    result
}
