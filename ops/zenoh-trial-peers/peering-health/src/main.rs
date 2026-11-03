// SPDX-License-Identifier: Apache-2.0
//
// goat-peering-health — the `peering` domain of the environment-health surface.
//
// docs/operations/peering-denial-observability.md + ADR 1036 (env-health
// contract). A normal mTLS Zenoh client peer (templated on
// goat-node-net-health):
//   - Reads the peering-denial snapshot at a fixed path (written atomically by
//     the peering-denials-snapshot systemd timer — ops/peering-denials/pd-snapshot.py
//     — bind-mounted read-only). That collector reads the edge router's own ACL
//     decision log and accumulates monotonic per-(session, key, action,
//     direction) counters.
//   - Grades it with the 5-state honesty colors and, on a timer, publishes a
//     per-edge reading back on the bus in C0:
//       ops/peering/health/<edge>   <- EnvHealthV1{domain:"peering"}
//   - Subscribes nothing in publish mode. Persists nothing. Observation, not
//     enforcement — the ACL is the enforcement and this only reveals it.
//
// WHY THIS EXISTS. F-177: an ACL denial at a peering edge is silent at both
// ends. The partner's `put` is fire-and-forget so it gets no error, and nothing
// on our side raises anything, so "the partner is being refused" and "the
// partner never sent anything" look identical — to an operator and to an
// assessor. Continuous monitoring cannot assert an enforcement it cannot see.
//
// THE FIVE STATES, and the two absences they separate:
//
//   unmetered  The observer is OFF on that edge (the ACL interceptor is not at
//              trace), or the snapshot is stale/missing, or the log carried
//              decision lines the collector could not parse and none it could.
//              We cannot see. NEVER green.
//   idle       The observer is on and working, and there were NO ACL decisions
//              at all — neither allowed nor denied. Nothing was asked. Neutral:
//              absence is not illness (ADR 1036 §3.4), a partner is allowed to
//              be quiet.
//   green      Decisions observed, none denied. The link is being used and
//              nothing is being refused.
//   amber      Denials observed AND allows observed: part of what the partner
//              sends lands and part is refused — the lane-mismatch shape. Also
//              amber when some decision lines did not parse, because a reading
//              that may be incomplete must not read clean.
//   red        Denials observed and ZERO allows: the partner is being refused
//              wholesale and nothing is getting through. This is the state
//              F-177 says two organisations currently spend a day misreading as
//              a network fault.
//
// `idle` and `red` are the pair that must never render the same. They differ on
// exactly one measured fact — whether any ACL decision was denied — and that
// fact comes from the same log, in the same window, as the allow side.

use anyhow::{Context, Result};
use clap::Parser;
use prost::Message as _;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};
use zenoh::qos::{CongestionControl, Priority};

// ---------------------------------------------------------------------------
// Wire types — keep field tags in lock-step with the canonical schema at
// ops/zenoh-trial-peers/time-health/proto/env_health_v1.proto (arm 16).
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, prost::Message, serde::Serialize)]
struct PeeringDenialV1 {
    #[prost(string, tag = "1")]
    zid: String,
    #[prost(string, tag = "2")]
    subject: String,
    #[prost(string, tag = "3")]
    cert_common_name: String,
    #[prost(string, tag = "4")]
    key_expr: String,
    #[prost(string, tag = "5")]
    action: String,
    #[prost(string, tag = "6")]
    direction: String,
    #[prost(string, tag = "7")]
    verdict: String,
    #[prost(uint64, tag = "8")]
    window_count: u64,
    #[prost(uint64, tag = "9")]
    cached_count: u64,
    #[prost(uint64, tag = "10")]
    evaluated_count: u64,
    #[prost(uint64, tag = "11")]
    total_count: u64,
    #[prost(int64, tag = "12")]
    first_seen_unix_ms: i64,
    #[prost(int64, tag = "13")]
    last_seen_unix_ms: i64,
}

#[derive(Clone, PartialEq, prost::Message, serde::Serialize)]
struct PeeringDetailV1 {
    #[prost(message, repeated, tag = "1")]
    denials: Vec<PeeringDenialV1>,
    #[prost(string, tag = "2")]
    edge: String,
    #[prost(string, tag = "3")]
    partner_id: String,
    #[prost(bool, tag = "4")]
    observer_enabled: bool,
    #[prost(string, tag = "5")]
    observer_note: String,
    #[prost(uint64, tag = "6")]
    allow_window: u64,
    #[prost(uint64, tag = "7")]
    deny_window: u64,
    #[prost(uint64, tag = "8")]
    allow_total: u64,
    #[prost(uint64, tag = "9")]
    deny_total: u64,
    #[prost(uint64, tag = "10")]
    unparsed_acl_lines: u64,
    #[prost(bool, tag = "11")]
    repeats_observed: bool,
    #[prost(uint64, tag = "12")]
    cached_denial_lines: u64,
    #[prost(uint32, tag = "13")]
    snapshot_age_secs: u32,
}

#[derive(Clone, PartialEq, prost::Message, serde::Serialize)]
struct EnvHealthV1 {
    #[prost(string, tag = "1")]
    domain: String,
    #[prost(string, tag = "2")]
    scope: String,
    #[prost(int64, tag = "3")]
    ts_unix_ms: i64,
    #[prost(string, tag = "4")]
    overall_color: String,
    #[prost(uint64, tag = "5")]
    sample_count: u64,
    #[prost(uint32, tag = "6")]
    window_secs: u32,
    #[prost(oneof = "EnvDetail", tags = "16")]
    detail: Option<EnvDetail>,
}

#[derive(Clone, PartialEq, prost::Oneof, serde::Serialize)]
enum EnvDetail {
    #[prost(message, tag = "16")]
    Peering(PeeringDetailV1),
}

// ---------------------------------------------------------------------------
// Incoming snapshot — the JSON ops/peering-denials/pd-snapshot.py writes.
// `#[serde(default)]` keeps a partial snapshot from killing the whole read.
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize, Default)]
struct Snapshot {
    #[serde(default)]
    #[allow(dead_code)]
    generated_unix_ms: i64,
    #[serde(default)]
    edge: String,
    #[serde(default)]
    partner_id: String,
    #[serde(default)]
    observer_enabled: bool,
    #[serde(default)]
    observer_note: String,
    #[serde(default)]
    allow_window: u64,
    #[serde(default)]
    deny_window: u64,
    #[serde(default)]
    allow_total: u64,
    #[serde(default)]
    deny_total: u64,
    #[serde(default)]
    unparsed_acl_lines: u64,
    #[serde(default)]
    repeats_observed: bool,
    #[serde(default)]
    cached_denial_lines: u64,
    #[serde(default)]
    denials: Vec<SnapDenial>,
}

#[derive(serde::Deserialize, Default)]
struct SnapDenial {
    #[serde(default)]
    zid: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    cert_common_name: String,
    #[serde(default)]
    key_expr: String,
    #[serde(default)]
    action: String,
    #[serde(default)]
    direction: String,
    #[serde(default)]
    window_count: u64,
    #[serde(default)]
    cached_count: u64,
    #[serde(default)]
    evaluated_count: u64,
    #[serde(default)]
    total_count: u64,
    #[serde(default)]
    first_seen_unix_ms: i64,
    #[serde(default)]
    last_seen_unix_ms: i64,
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The 5-state grade. Split out from `roll_up` so the state machine is one
/// readable thing and the tests assert it directly.
fn grade(snap: &Snapshot) -> &'static str {
    if !snap.observer_enabled {
        // The observer is off or the container is gone. An empty denial list
        // means nothing at all here, so it must not read as "no denials".
        return "unmetered";
    }
    let decisions = snap.allow_window + snap.deny_window;
    if decisions == 0 {
        if snap.unparsed_acl_lines > 0 {
            // Decision lines are being printed and NONE of them parsed — the
            // log shape moved under the collector. We are looking at something
            // we do not understand, which is not the same as looking at nothing.
            return "unmetered";
        }
        // Nothing was asked. Not a failure: a partner may be legitimately
        // quiet, and this is the state that must never look like a refusal.
        return "idle";
    }
    if snap.deny_window == 0 {
        // Some lines did not parse, so the denial count may be short. A reading
        // that cannot vouch for its own completeness does not get to be green.
        if snap.unparsed_acl_lines > 0 {
            return "amber";
        }
        return "green";
    }
    if snap.allow_window == 0 {
        // Refused wholesale: every decision in this window was a denial, so
        // nothing the partner sent is landing.
        return "red";
    }
    // Some of it lands, some is refused — the prefix/lane mismatch shape.
    "amber"
}

/// Build the per-edge EnvHealthV1{domain:"peering"} from a parsed snapshot.
fn roll_up(snap: &Snapshot, edge: &str, window_secs: u32, snapshot_age_secs: u32) -> EnvHealthV1 {
    let denials: Vec<PeeringDenialV1> = snap
        .denials
        .iter()
        .map(|d| PeeringDenialV1 {
            zid: d.zid.clone(),
            subject: d.subject.clone(),
            cert_common_name: d.cert_common_name.clone(),
            key_expr: d.key_expr.clone(),
            action: d.action.clone(),
            direction: d.direction.clone(),
            verdict: "deny".to_string(),
            window_count: d.window_count,
            cached_count: d.cached_count,
            evaluated_count: d.evaluated_count,
            total_count: d.total_count,
            first_seen_unix_ms: d.first_seen_unix_ms,
            last_seen_unix_ms: d.last_seen_unix_ms,
        })
        .collect();

    EnvHealthV1 {
        domain: "peering".to_string(),
        scope: edge.to_string(),
        ts_unix_ms: now_unix_ms(),
        overall_color: grade(snap).to_string(),
        // Grounding n = the ACL decisions this window actually saw, allows
        // included. A reading with n=0 is `idle` or `unmetered`, never green.
        sample_count: snap.allow_window + snap.deny_window,
        window_secs,
        detail: Some(EnvDetail::Peering(PeeringDetailV1 {
            denials,
            edge: if snap.edge.is_empty() {
                edge.to_string()
            } else {
                snap.edge.clone()
            },
            partner_id: snap.partner_id.clone(),
            observer_enabled: snap.observer_enabled,
            observer_note: snap.observer_note.clone(),
            allow_window: snap.allow_window,
            deny_window: snap.deny_window,
            allow_total: snap.allow_total,
            deny_total: snap.deny_total,
            unparsed_acl_lines: snap.unparsed_acl_lines,
            repeats_observed: snap.repeats_observed,
            cached_denial_lines: snap.cached_denial_lines,
            snapshot_age_secs,
        })),
    }
}

/// The honest "nothing to read" envelope — stale or missing snapshot. `note`
/// says which, because "the collector is not running" and "the observer is off"
/// need different remedies from the operator.
fn unmetered(edge: &str, window_secs: u32, note: &str) -> EnvHealthV1 {
    EnvHealthV1 {
        domain: "peering".to_string(),
        scope: edge.to_string(),
        ts_unix_ms: now_unix_ms(),
        overall_color: "unmetered".to_string(),
        sample_count: 0,
        window_secs,
        detail: Some(EnvDetail::Peering(PeeringDetailV1 {
            edge: edge.to_string(),
            observer_note: note.to_string(),
            ..Default::default()
        })),
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "goat-peering-health",
    about = "Peering domain of the environment-health surface — per-edge ACL denial observability"
)]
struct Args {
    /// Zenoh router endpoint, e.g. `tls/zenoh-router:7447`.
    #[arg(long, env = "PDH_ENDPOINT")]
    endpoint: String,

    /// publish (default): grade this edge's snapshot -> ops/peering/health/<edge>.
    /// rollup: subscribe ops/peering/health/** and write the fleet JSON a
    /// dashboard reads (the passive roll-up reader — brokerless-safe, no
    /// central poller of edges; edges still self-publish).
    #[arg(long, env = "PDH_MODE", default_value = "publish")]
    mode: String,

    /// Edge identifier — the `<edge>` segment of ops/peering/health/<edge>
    /// (publish mode).
    #[arg(long, env = "PDH_EDGE", default_value = "")]
    edge: String,

    /// rollup mode: output JSON path (written atomically).
    #[arg(
        long,
        env = "PDH_ROLLUP_OUT",
        default_value = "/www/peering-health.json"
    )]
    rollup_out: String,

    /// Path of the peering-denial snapshot (bind-mounted read-only; written
    /// atomically by the host-side peering-denials-snapshot systemd timer).
    #[arg(
        long,
        env = "PDH_SNAPSHOT_FILE",
        default_value = "/state/snapshot.json"
    )]
    snapshot_file: String,

    /// Leaf X.509 cert (PEM) for mTLS. Required for a `tls/` endpoint.
    #[arg(long, env = "PDH_CERT")]
    cert: Option<String>,
    /// Matching private key (PEM).
    #[arg(long, env = "PDH_KEY_FILE")]
    key: Option<String>,
    /// CA root cert (PEM) used to verify the router.
    #[arg(long, env = "PDH_ROOT_CA")]
    root_ca: Option<String>,

    /// Publish interval (ms). Re-publishes the latest snapshot each beat — same
    /// cadence as the sibling env-health meters, so a one-shot
    /// `goat health --domain peering` catches a beat.
    #[arg(long, env = "PDH_INTERVAL_MS", default_value_t = 30_000)]
    interval_ms: u64,

    /// Staleness window (ms): a snapshot older than this publishes `unmetered`
    /// (never a false green). Default = ~4 collector intervals + slack, for a
    /// collector on a 60s timer.
    #[arg(long, env = "PDH_STALE_AFTER_MS", default_value_t = 300_000)]
    stale_after_ms: u64,
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

/// rollup mode — the passive fleet reader: subscribe ops/peering/health/**,
/// keep each edge's latest envelope, write the JSON a dashboard reads (atomic
/// rename). Reveal-only: edges self-publish; this only listens.
async fn rollup(session: &zenoh::Session, out: &str) -> Result<()> {
    #[derive(serde::Serialize)]
    struct EdgeEntry {
        received_at_unix_ms: i64,
        env: EnvHealthV1,
    }
    let subscriber = session
        .declare_subscriber("ops/peering/health/**")
        .await
        .map_err(|e| anyhow::anyhow!("declare subscriber: {e}"))?;
    info!(out = out, "rollup mode: listening on ops/peering/health/**");
    let mut edges: std::collections::BTreeMap<String, EdgeEntry> = Default::default();
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("install SIGTERM handler")?;
    loop {
        tokio::select! {
            s = subscriber.recv_async() => {
                let Ok(sample) = s else { break };
                let payload = sample.payload().to_bytes();
                match EnvHealthV1::decode(payload.as_ref()) {
                    Ok(env) if env.domain == "peering" => {
                        let scope = env.scope.clone();
                        edges.insert(scope.clone(), EdgeEntry { received_at_unix_ms: now_unix_ms(), env });
                        let doc = serde_json::json!({
                            "generated_unix_ms": now_unix_ms(),
                            "edges": edges,
                        });
                        let tmp = format!("{out}.tmp");
                        if let Err(e) = std::fs::write(&tmp, serde_json::to_vec(&doc)?)
                            .and_then(|_| std::fs::rename(&tmp, out))
                        {
                            warn!(error = %e, "rollup write failed");
                        } else {
                            info!(edge = scope.as_str(), edges = edges.len(), "rollup updated");
                        }
                    }
                    Ok(_) => {}
                    Err(e) => warn!(error = %e, "undecodable envelope on ops/peering/health/**"),
                }
            }
            _ = sigterm.recv() => { info!("SIGTERM received; shutting down"); break; }
            _ = tokio::signal::ctrl_c() => { info!("SIGINT received; shutting down"); break; }
        }
    }
    Ok(())
}

/// Read + parse the snapshot, honestly: stale mtime, missing file, or a parse
/// failure each return the reason (the caller publishes `unmetered` with it).
fn read_snapshot(path: &Path, stale_after: Duration) -> Result<(Snapshot, u32), String> {
    let meta = std::fs::metadata(path)
        .map_err(|e| format!("snapshot missing at {}: {e}", path.display()))?;
    let age = meta
        .modified()
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .unwrap_or(Duration::ZERO);
    if age > stale_after {
        return Err(format!(
            "snapshot at {} is {}s old — the peering-denials collector is not running",
            path.display(),
            age.as_secs()
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("snapshot unreadable: {e}"))?;
    serde_json::from_slice::<Snapshot>(&bytes)
        .map(|s| (s, age.as_secs() as u32))
        .map_err(|e| format!("snapshot undecodable: {e}"))
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
    let session = open_session(&args).await.context("open zenoh session")?;
    if args.mode == "rollup" {
        return rollup(&session, &args.rollup_out).await;
    }
    anyhow::ensure!(!args.edge.is_empty(), "--edge required in publish mode");
    let health_key = format!("ops/peering/health/{}", args.edge);
    info!(
        zid = %session.zid(),
        endpoint = args.endpoint.as_str(),
        edge = args.edge.as_str(),
        snapshot_file = args.snapshot_file.as_str(),
        publishes = health_key.as_str(),
        "session open; grading peering-denial snapshot -> ops/peering/health/<edge>"
    );

    let interval = Duration::from_millis(args.interval_ms);
    let stale_after = Duration::from_millis(args.stale_after_ms);
    let snapshot_path = std::path::PathBuf::from(&args.snapshot_file);
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("install SIGTERM handler")?;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let window_secs = (interval.as_secs() as u32).max(1);
                let reading = match read_snapshot(&snapshot_path, stale_after) {
                    Ok((snap, age)) => roll_up(&snap, &args.edge, window_secs, age),
                    Err(note) => {
                        warn!(note = note.as_str(), "cannot read peering-denial snapshot");
                        unmetered(&args.edge, window_secs, &note)
                    }
                };
                let n = reading.sample_count;
                let overall = reading.overall_color.clone();
                let payload = reading.encode_to_vec();
                match session
                    .put(&health_key, payload)
                    .congestion_control(CongestionControl::Block)
                    .priority(Priority::InteractiveHigh)
                    .await
                {
                    Ok(()) => info!(key = health_key.as_str(), n = n, overall = overall.as_str(), "published peering health"),
                    Err(e) => warn!(error = %e, "publish peering health failed"),
                }
            }
            _ = sigterm.recv() => { info!("SIGTERM received; shutting down"); break; }
            _ = tokio::signal::ctrl_c() => { info!("SIGINT received; shutting down"); break; }
        }
    }

    let _ = session.close().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every fixture below is REAL OUTPUT of ops/peering-denials/pd-snapshot.py
    // run against the log fixtures in ops/peering-denials/fixtures/, not a
    // hand-written approximation of it. If the collector's shape changes, these
    // stop parsing.

    // ops/peering-denials/fixtures/cached-denial.log — one evaluated denial
    // followed by eleven served from zenoh's ACL cache.
    const CACHED_DENIAL: &str = r#"{
      "allow_total": 0, "allow_window": 0, "cached_denial_lines": 11,
      "denials": [{
        "action": "Put", "cached_count": 11, "cert_common_name": "peer-svn",
        "direction": "ingress", "evaluated_count": 1,
        "first_seen_unix_ms": 1788519660000, "key_expr": "from/svn-staging/telemetry",
        "last_seen_unix_ms": 1788519671000,
        "subject": "Interface(eth0)+CertCommonName(peer-svn)+Zid(4a1f7c2b9d0e3a56)",
        "total_count": 12, "verdict": "deny", "window_count": 12,
        "zid": "4a1f7c2b9d0e3a56"
      }],
      "deny_total": 12, "deny_window": 12, "edge": "zagros-edge-svn",
      "first_run": true, "generated_unix_ms": 1788519700000,
      "log_lines_scanned": 15, "observer_enabled": true, "observer_note": "",
      "partner_id": "svn", "repeats_observed": true, "unparsed_acl_lines": 0
    }"#;

    // ops/peering-denials/fixtures/repeated-denial.log — six denials, no allows.
    const REPEATED_DENIAL: &str = r#"{
      "allow_total": 0, "allow_window": 0, "cached_denial_lines": 0,
      "denials": [{
        "action": "Put", "cached_count": 0, "cert_common_name": "peer-svn",
        "direction": "ingress", "evaluated_count": 6,
        "first_seen_unix_ms": 1788519660000, "key_expr": "from/svn-staging/telemetry",
        "last_seen_unix_ms": 1788519685000, "subject": "CertCommonName(peer-svn)",
        "total_count": 6, "verdict": "deny", "window_count": 6,
        "zid": "4a1f7c2b9d0e3a56"
      }],
      "deny_total": 6, "deny_window": 6, "edge": "zagros-edge-svn",
      "first_run": true, "generated_unix_ms": 1788519700000,
      "log_lines_scanned": 9, "observer_enabled": true, "observer_note": "",
      "partner_id": "svn", "repeats_observed": true, "unparsed_acl_lines": 0
    }"#;

    // ops/peering-denials/fixtures/single-denial.log — one denial beside two
    // accepted writes.
    const SINGLE_DENIAL: &str = r#"{
      "allow_total": 2, "allow_window": 2, "cached_denial_lines": 0,
      "denials": [{
        "action": "Put", "cached_count": 0, "cert_common_name": "peer-svn",
        "direction": "ingress", "evaluated_count": 1,
        "first_seen_unix_ms": 1788519670000, "key_expr": "ops/fabric/hijack",
        "last_seen_unix_ms": 1788519670000, "subject": "CertCommonName(peer-svn)",
        "total_count": 1, "verdict": "deny", "window_count": 1,
        "zid": "4a1f7c2b9d0e3a56"
      }],
      "deny_total": 1, "deny_window": 1, "edge": "zagros-edge-svn",
      "first_run": true, "generated_unix_ms": 1788519700000,
      "log_lines_scanned": 6, "observer_enabled": true, "observer_note": "",
      "partner_id": "svn", "repeats_observed": false, "unparsed_acl_lines": 0
    }"#;

    // ops/peering-denials/fixtures/no-denials-traffic.log
    const CLEAN_TRAFFIC: &str = r#"{
      "allow_total": 9, "allow_window": 9, "cached_denial_lines": 0,
      "denials": [], "deny_total": 0, "deny_window": 0, "edge": "zagros-edge-svn",
      "first_run": true, "generated_unix_ms": 1788519700000,
      "log_lines_scanned": 12, "observer_enabled": true, "observer_note": "",
      "partner_id": "svn", "repeats_observed": false, "unparsed_acl_lines": 0
    }"#;

    // ops/peering-denials/fixtures/no-traffic.log — the edge is up, the
    // observer is on, and nobody asked it anything.
    const NO_TRAFFIC: &str = r#"{
      "allow_total": 0, "allow_window": 0, "cached_denial_lines": 0,
      "denials": [], "deny_total": 0, "deny_window": 0, "edge": "zagros-edge-svn",
      "first_run": true, "generated_unix_ms": 1788519700000,
      "log_lines_scanned": 4, "observer_enabled": true, "observer_note": "",
      "partner_id": "svn", "repeats_observed": false, "unparsed_acl_lines": 0
    }"#;

    // ops/peering-denials/fixtures/unrecognised-shape.log — verdict lines the
    // collector could not parse, and none it could.
    const UNRECOGNISED: &str = r#"{
      "allow_total": 0, "allow_window": 0, "cached_denial_lines": 0,
      "denials": [], "deny_total": 0, "deny_window": 0, "edge": "zagros-edge-svn",
      "first_run": true, "generated_unix_ms": 1788519700000,
      "log_lines_scanned": 5, "observer_enabled": true, "observer_note": "",
      "partner_id": "svn", "repeats_observed": false, "unparsed_acl_lines": 2
    }"#;

    // The trace switch is off on the edge, so nothing can be observed at all.
    const OBSERVER_OFF: &str = r#"{
      "allow_total": 0, "allow_window": 0, "cached_denial_lines": 0,
      "denials": [], "deny_total": 0, "deny_window": 0, "edge": "zagros-edge-svn",
      "first_run": true, "generated_unix_ms": 1788519700000,
      "log_lines_scanned": 0, "observer_enabled": false,
      "observer_note": "the ACL interceptor is not at trace on zagros-edge-svn, so no decision is logged and nothing can be observed. Set goat_zenoh_edge_acl_trace: true for this edge and re-deploy.",
      "partner_id": "svn", "repeats_observed": false, "unparsed_acl_lines": 0
    }"#;

    fn parse(s: &str) -> Snapshot {
        serde_json::from_str(s).expect("fixture parses")
    }

    fn env(s: &str) -> EnvHealthV1 {
        roll_up(&parse(s), "zagros-edge-svn", 30, 12)
    }

    fn detail(e: &EnvHealthV1) -> &PeeringDetailV1 {
        match e.detail {
            Some(EnvDetail::Peering(ref d)) => d,
            _ => panic!("expected Peering detail"),
        }
    }

    // ---------------------------------------------------------------------
    // THE ONE THIS SURFACE EXISTS FOR.
    //
    // F-177/F-448 record the ACL decision as "logged once and never repeated".
    // If that were the whole story a repeated denial would be invisible and
    // this surface would be theatre. The pinned zenoh =1.9.0 source says the
    // cache short-circuits the EVALUATION and still prints a line per message
    // (access_control.rs:84 / 393) — so the twelve refusals of one partner on
    // one key must read as twelve, with the cached path named as the source of
    // eleven of them.
    // ---------------------------------------------------------------------
    #[test]
    fn a_cached_denial_reads_as_repeated_not_as_one() {
        let e = env(CACHED_DENIAL);
        let d = detail(&e);
        assert_eq!(e.overall_color, "red"); // refused, nothing landing
        assert_eq!(e.sample_count, 12);
        assert_eq!(d.deny_window, 12);
        assert_eq!(d.cached_denial_lines, 11);
        assert!(d.repeats_observed);
        assert_eq!(d.denials[0].window_count, 12);
        assert_eq!(d.denials[0].cached_count, 11);
        assert_eq!(d.denials[0].evaluated_count, 1);
        // The key expression is the diagnosis: the partner is on its staging
        // name, not the agreed wire name.
        assert_eq!(d.denials[0].key_expr, "from/svn-staging/telemetry");
        assert_eq!(d.denials[0].direction, "ingress");
        assert_eq!(d.denials[0].cert_common_name, "peer-svn");
        // Round-trips through protobuf on arm 16.
        let back = EnvHealthV1::decode(e.encode_to_vec().as_slice()).expect("decode");
        assert_eq!(back.domain, "peering");
        assert_eq!(back.overall_color, "red");
        assert_eq!(detail(&back).denials[0].cached_count, 11);
    }

    // ---------------------------------------------------------------------
    // REFUSED vs NEVER ATTEMPTED — the two absences that must not render the
    // same. Both deliver zero data to us; only one is a problem.
    // ---------------------------------------------------------------------
    #[test]
    fn refused_and_never_attempted_render_differently() {
        let refused = env(REPEATED_DENIAL);
        let quiet = env(NO_TRAFFIC);
        assert_eq!(refused.overall_color, "red");
        assert_eq!(quiet.overall_color, "idle");
        assert_ne!(refused.overall_color, quiet.overall_color);
        // And the reason is legible in the envelope, not only in the colour.
        assert_eq!(detail(&refused).deny_window, 6);
        assert_eq!(detail(&quiet).deny_window, 0);
        assert_eq!(detail(&quiet).allow_window, 0);
        assert!(detail(&quiet).observer_enabled); // idle because QUIET, not because blind
        assert_eq!(quiet.sample_count, 0);
    }

    #[test]
    fn a_single_denial_beside_traffic_is_amber_not_red() {
        let e = env(SINGLE_DENIAL);
        assert_eq!(e.overall_color, "amber"); // part lands, part refused
        let d = detail(&e);
        assert_eq!(d.allow_window, 2);
        assert_eq!(d.deny_window, 1);
        assert!(!d.repeats_observed);
        assert_eq!(d.denials[0].key_expr, "ops/fabric/hijack");
    }

    #[test]
    fn traffic_with_no_denials_is_green() {
        let e = env(CLEAN_TRAFFIC);
        assert_eq!(e.overall_color, "green");
        assert_eq!(e.sample_count, 9);
        assert!(detail(&e).denials.is_empty());
    }

    #[test]
    fn observer_off_is_unmetered_never_green() {
        let e = env(OBSERVER_OFF);
        assert_eq!(e.overall_color, "unmetered");
        assert_eq!(e.sample_count, 0);
        let d = detail(&e);
        assert!(!d.observer_enabled);
        assert!(d.observer_note.contains("goat_zenoh_edge_acl_trace"));
        // Distinct from `idle`: an empty denial list under a disabled observer
        // is not evidence of anything.
        assert_ne!(e.overall_color, env(NO_TRAFFIC).overall_color);
    }

    #[test]
    fn an_unreadable_log_shape_is_unmetered_never_green() {
        let e = env(UNRECOGNISED);
        assert_eq!(e.overall_color, "unmetered");
        assert_eq!(detail(&e).unparsed_acl_lines, 2);
    }

    #[test]
    fn partial_parse_failure_downgrades_a_clean_window() {
        // Allows understood, some lines not. There may be denials hiding in the
        // lines we could not read, so this must not read green.
        let mut s = parse(CLEAN_TRAFFIC);
        s.unparsed_acl_lines = 3;
        assert_eq!(grade(&s), "amber");
    }

    #[test]
    fn a_missing_snapshot_is_unmetered_with_the_reason() {
        let e = unmetered(
            "zagros-edge-svn",
            30,
            "snapshot missing at /state/snapshot.json",
        );
        assert_eq!(e.overall_color, "unmetered");
        assert_eq!(e.sample_count, 0);
        assert!(detail(&e).observer_note.contains("snapshot missing"));
        assert!(!detail(&e).observer_enabled);
    }

    #[test]
    fn the_monotonic_counter_survives_a_quiet_window() {
        // The window went quiet but the total stands: an operator watching the
        // counter still sees that 12 refusals happened, which is the minimum
        // viable evidence the S4 gate asks for.
        let mut s = parse(CACHED_DENIAL);
        s.deny_window = 0;
        s.allow_window = 0;
        s.repeats_observed = false;
        for d in s.denials.iter_mut() {
            d.window_count = 0;
        }
        let e = roll_up(&s, "zagros-edge-svn", 30, 12);
        assert_eq!(e.overall_color, "idle"); // nothing asked THIS window
        let d = detail(&e);
        assert_eq!(d.deny_total, 12); // ...but the refusals are still on record
        assert_eq!(d.denials[0].total_count, 12);
    }
}
