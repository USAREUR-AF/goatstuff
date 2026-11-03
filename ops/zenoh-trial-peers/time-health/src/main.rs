// SPDX-License-Identifier: Apache-2.0
//
// goat-time-health — the time domain of the environment-health surface.
//
// ADR 1036 (env-health) + ADR 1037 (internal-sync; this rolls the
// attestation/enforcement plane up into the non-enforcing VIEWING plane) +
// F-184. A normal mTLS Zenoh client peer (templated on goat-fabric-health):
//   - Subscribes the signed per-host attestations on ops/time/attest/** (the
//     NtpState JSON the ntp-health-agent signs, republished by goat-ntp-health-pub).
//   - Keeps the latest verdict per host (+ the local receive-time, the honest
//     staleness clock — we do NOT trust the verdict's own timestamps to decide
//     if it's stale; that's the thing being measured).
//   - On a timer, rolls the hosts up into a per-site honesty-graded reading and
//     publishes it back on the bus in C0 (Block + high priority):
//       ops/time/health/<site>   <- EnvHealthV1{domain:"time"}
//   - Persists nothing.
//
// Honesty (docs/design/environment-health-surface.md §6): color is the 5-state
// signal — healthy→green, warning→amber, degraded→red, unknown→idle; an
// UNATTESTED verdict (attested=false) reads idle (UNCONFIRMED, never green); a
// STALE host (no fresh sample within the staleness window) reads idle, never a
// false green. overall_color is the worst *grounded* host (red>amber>green),
// idle when no host is grounded, unmetered when no host has reported at all.
//
// Two-axis note (ADR 1037): until the internal-sync engine produces an internal-
// coherence reading, `color` here is the UTC-confidence axis (the agent's
// ntp_class verdict). Internal coherence is not yet a field — it is honestly
// unmeasured, not asserted.

use anyhow::{Context, Result};
use clap::Parser;
use prost::Message as _;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use tracing::{info, warn};
use zenoh::qos::{CongestionControl, Priority};

// ---------------------------------------------------------------------------
// Wire types — keep field tags in lock-step with proto/env_health_v1.proto.
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, prost::Message)]
struct HostClockHealthV1 {
    #[prost(string, tag = "1")]
    host_id: String,
    #[prost(string, tag = "2")]
    color: String,
    #[prost(string, tag = "3")]
    verdict: String,
    #[prost(string, repeated, tag = "4")]
    verdict_reasons: Vec<String>,
    #[prost(double, tag = "5")]
    rms_offset_ms: f64,
    #[prost(int32, tag = "6")]
    stratum: i32,
    #[prost(bool, tag = "7")]
    gnss_present: bool,
    #[prost(double, tag = "8")]
    peer_median_offset_ms: f64,
    #[prost(bool, tag = "9")]
    divergence_detected: bool,
    #[prost(int64, tag = "10")]
    attestation_age_ms: i64,
    #[prost(bool, tag = "11")]
    attested: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
struct TimeDetailV1 {
    #[prost(message, repeated, tag = "1")]
    hosts: Vec<HostClockHealthV1>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct FabricDetailV1 {}

#[derive(Clone, PartialEq, prost::Message)]
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
    #[prost(oneof = "EnvDetail", tags = "10, 11")]
    detail: Option<EnvDetail>,
}

#[derive(Clone, PartialEq, prost::Oneof)]
enum EnvDetail {
    #[prost(message, tag = "10")]
    Fabric(FabricDetailV1),
    #[prost(message, tag = "11")]
    Time(TimeDetailV1),
}

// ---------------------------------------------------------------------------
// Incoming agent verdict — the subset of the ntp-health-agent NtpState JSON
// (ops/time-hygiene/internal/state) that the roll-up reads. Extra fields are
// ignored. `#[serde(default)]` keeps a malformed/partial verdict from killing
// the whole sample.
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize, Default)]
struct AgentVerdict {
    #[serde(default)]
    host_id: String,
    #[serde(default)]
    ntp_class_str: String,
    #[serde(default)]
    selected_source: String,
    #[serde(default)]
    stratum: i32,
    #[serde(default)]
    rms_offset_ns: i64,
    #[serde(default)]
    attested: bool,
}

/// What we keep per host: the verdict + the LOCAL receive instant (the honest
/// staleness clock).
struct HostEntry {
    verdict: AgentVerdict,
    received_at: Instant,
}

type Accum = HashMap<String, HostEntry>;

/// The 5-state honesty color for a host. Stale or unattested → idle (never a
/// false green); else map the verdict.
fn host_color(verdict: &str, attested: bool, stale: bool) -> &'static str {
    if stale || !attested {
        return "idle";
    }
    match verdict {
        "healthy" => "green",
        "warning" => "amber",
        "degraded" => "red",
        _ => "idle", // unknown / unrecognized → unconfirmed
    }
}

/// Worst *grounded* color across hosts: red > amber > green; idle when hosts
/// exist but none is grounded; unmetered when no host has reported at all.
fn overall_color(host_colors: &[&str]) -> &'static str {
    if host_colors.is_empty() {
        return "unmetered";
    }
    if host_colors.contains(&"red") {
        "red"
    } else if host_colors.contains(&"amber") {
        "amber"
    } else if host_colors.contains(&"green") {
        "green"
    } else {
        "idle"
    }
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Build the per-site EnvHealthV1{domain:"time"} from the accumulator snapshot.
fn roll_up(snapshot: &Accum, site: &str, stale_after: Duration, window_secs: u32) -> EnvHealthV1 {
    let now = Instant::now();
    let mut hosts: Vec<HostClockHealthV1> = Vec::with_capacity(snapshot.len());
    for entry in snapshot.values() {
        let age = now.saturating_duration_since(entry.received_at);
        let stale = age > stale_after;
        let v = &entry.verdict;
        let color = host_color(&v.ntp_class_str, v.attested, stale);
        hosts.push(HostClockHealthV1 {
            host_id: v.host_id.clone(),
            color: color.to_string(),
            verdict: v.ntp_class_str.clone(),
            verdict_reasons: Vec::new(),
            rms_offset_ms: v.rms_offset_ns as f64 / 1.0e6,
            stratum: v.stratum,
            gnss_present: v.selected_source == "gnss",
            peer_median_offset_ms: 0.0, // populated when §5 cross-site consensus ships
            divergence_detected: false,
            attestation_age_ms: age.as_millis() as i64,
            attested: v.attested,
        });
    }
    hosts.sort_by(|a, b| a.host_id.cmp(&b.host_id)); // deterministic ordering
    let colors: Vec<&str> = hosts.iter().map(|h| h.color.as_str()).collect();
    EnvHealthV1 {
        domain: "time".to_string(),
        scope: site.to_string(),
        ts_unix_ms: now_unix_ms(),
        overall_color: overall_color(&colors).to_string(),
        sample_count: hosts.len() as u64,
        window_secs,
        detail: Some(EnvDetail::Time(TimeDetailV1 { hosts })),
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "goat-time-health",
    about = "Time domain of the environment-health surface — per-site clock-health roll-up (ADR 1036/1037, F-184)"
)]
struct Args {
    /// Zenoh router endpoint, e.g. `tls/goat-zenoh-router:7447`.
    #[arg(long, env = "TIME_HEALTH_ENDPOINT")]
    endpoint: String,

    /// Site identifier — the `<site>` segment of ops/time/health/<site>.
    #[arg(long, env = "TIME_HEALTH_SITE")]
    site: String,

    /// Leaf X.509 cert (PEM) for mTLS. Required for a `tls/` endpoint.
    #[arg(long, env = "TIME_HEALTH_CERT")]
    cert: Option<String>,
    /// Matching private key (PEM).
    #[arg(long, env = "TIME_HEALTH_KEY_FILE")]
    key: Option<String>,
    /// CA root cert (PEM) used to verify the router.
    #[arg(long, env = "TIME_HEALTH_ROOT_CA")]
    root_ca: Option<String>,

    /// Publish interval (ms). The published window_secs reflects it.
    #[arg(long, env = "TIME_HEALTH_INTERVAL_MS", default_value_t = 30_000)]
    interval_ms: u64,

    /// Staleness window (ms): a host with no fresh attestation within this
    /// window reads `idle` (never a false green). Default = 3 heartbeats.
    #[arg(long, env = "TIME_HEALTH_STALE_AFTER_MS", default_value_t = 90_000)]
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
        let cert = args.cert.as_ref().context("--cert required for tls/ endpoint")?;
        let key = args.key.as_ref().context("--key required for tls/ endpoint")?;
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let session = Arc::new(open_session(&args).await.context("open zenoh session")?);
    let health_key = format!("ops/time/health/{}", args.site);
    info!(
        zid = %session.zid(),
        endpoint = args.endpoint.as_str(),
        site = args.site.as_str(),
        publishes = health_key.as_str(),
        "session open; rolling up ops/time/attest/** -> ops/time/health/<site>"
    );

    let accum: Arc<Mutex<Accum>> = Arc::new(Mutex::new(HashMap::new()));

    // Subscriber: keep the latest verdict per host.
    let subscriber = session
        .declare_subscriber("ops/time/attest/**")
        .await
        .map_err(|e| anyhow::anyhow!("declare_subscriber(ops/time/attest/**): {e}"))?;

    let interval = Duration::from_millis(args.interval_ms);
    let stale_after = Duration::from_millis(args.stale_after_ms);
    let site = args.site.clone();
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await; // consume the immediate first tick
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("install SIGTERM handler")?;

    loop {
        tokio::select! {
            recv = subscriber.recv_async() => {
                match recv {
                    Ok(sample) => {
                        let bytes = sample.payload().to_bytes();
                        match serde_json::from_slice::<AgentVerdict>(&bytes) {
                            Ok(mut v) => {
                                // Trust the signed host_id; fall back to the key's
                                // last segment if the verdict omits it.
                                if v.host_id.is_empty() {
                                    if let Some(seg) = sample.key_expr().as_str().rsplit('/').next() {
                                        v.host_id = seg.to_string();
                                    }
                                }
                                if v.host_id.is_empty() {
                                    warn!("attest sample with no host_id; skipping");
                                } else {
                                    let mut g = accum.lock().await;
                                    g.insert(v.host_id.clone(), HostEntry { verdict: v, received_at: Instant::now() });
                                }
                            }
                            Err(e) => warn!(error = %e, key = sample.key_expr().as_str(), "undecodable attest verdict; skipping"),
                        }
                    }
                    Err(e) => { warn!(error = %e, "subscriber closed"); break; }
                }
            }
            _ = ticker.tick() => {
                let snapshot_guard = accum.lock().await;
                let reading = roll_up(&snapshot_guard, &site, stale_after, (interval.as_secs() as u32).max(1));
                let n = reading.sample_count;
                let overall = reading.overall_color.clone();
                drop(snapshot_guard);
                let payload = reading.encode_to_vec();
                match session
                    .put(&health_key, payload)
                    .congestion_control(CongestionControl::Block)
                    .priority(Priority::InteractiveHigh)
                    .await
                {
                    Ok(()) => info!(key = health_key.as_str(), hosts = n, overall = overall.as_str(), "published time health"),
                    Err(e) => warn!(error = %e, "publish time health failed"),
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

    #[test]
    fn color_mapping_is_honest() {
        // Attested + fresh → verdict colors.
        assert_eq!(host_color("healthy", true, false), "green");
        assert_eq!(host_color("warning", true, false), "amber");
        assert_eq!(host_color("degraded", true, false), "red");
        assert_eq!(host_color("unknown", true, false), "idle");
        // Stale never trusts a green.
        assert_eq!(host_color("healthy", true, true), "idle");
        // Unattested → unconfirmed, never green.
        assert_eq!(host_color("healthy", false, false), "idle");
    }

    #[test]
    fn overall_is_worst_grounded_never_false_green() {
        assert_eq!(overall_color(&[]), "unmetered"); // no host reported
        assert_eq!(overall_color(&["idle", "idle"]), "idle"); // none grounded
        assert_eq!(overall_color(&["green", "idle"]), "green");
        assert_eq!(overall_color(&["green", "amber"]), "amber");
        assert_eq!(overall_color(&["green", "amber", "red"]), "red");
    }

    #[test]
    fn rollup_builds_time_envelope() {
        let mut acc: Accum = HashMap::new();
        acc.insert(
            "host-a".to_string(),
            HostEntry {
                verdict: AgentVerdict {
                    host_id: "host-a".to_string(),
                    ntp_class_str: "healthy".to_string(),
                    selected_source: "gnss".to_string(),
                    stratum: 2,
                    rms_offset_ns: 1_000_000,
                    attested: true,
                },
                received_at: Instant::now(),
            },
        );
        let e = roll_up(&acc, "nova-a", Duration::from_secs(90), 30);
        assert_eq!(e.domain, "time");
        assert_eq!(e.scope, "nova-a");
        assert_eq!(e.sample_count, 1);
        assert_eq!(e.overall_color, "green");
        match e.detail {
            Some(EnvDetail::Time(ref t)) => {
                assert_eq!(t.hosts.len(), 1);
                assert_eq!(t.hosts[0].host_id, "host-a");
                assert_eq!(t.hosts[0].color, "green");
                assert!(t.hosts[0].gnss_present);
                assert_eq!(t.hosts[0].rms_offset_ms, 1.0); // 1_000_000 ns
            }
            _ => panic!("expected Time detail"),
        }
        // Round-trips through protobuf.
        let bytes = e.encode_to_vec();
        let back = EnvHealthV1::decode(bytes.as_slice()).expect("decode");
        assert_eq!(back.domain, "time");
        assert_eq!(back.overall_color, "green");
    }

    #[test]
    fn parses_agent_ntpstate_json_subset() {
        // A representative ntp-health-agent /health body (extra fields ignored).
        let json = r#"{"host_id":"vega-01","ntp_class":0,"ntp_class_str":"healthy",
            "selected_source":"gnss","stratum":2,"ref_id":"GPS","rms_offset_ns":187000,
            "leap_status":0,"attested":true,"signature":"abc","valid_until":"2026-06-09T00:05:00Z"}"#;
        let v: AgentVerdict = serde_json::from_slice(json.as_bytes()).expect("parse");
        assert_eq!(v.host_id, "vega-01");
        assert_eq!(v.ntp_class_str, "healthy");
        assert!(v.attested);
        assert_eq!(v.rms_offset_ns, 187000);
    }
}
