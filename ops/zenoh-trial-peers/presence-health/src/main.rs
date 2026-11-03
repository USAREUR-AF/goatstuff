// SPDX-License-Identifier: Apache-2.0
//
// goat-presence-health — the presence domain of the environment-health surface.
//
// ADR 1040 gap 3 (liveliness presence) + ADR 1036 (env-health contract). A
// normal mTLS Zenoh client peer (templated on goat-mesh-health):
//   - Declares a liveliness subscriber on the fabric's presence-token keyexpr
//     (`**/presence/**` — the `<write_root>/presence/<label>` convention from
//     crates/panoscope-core/src/presence.rs; this crate hand-rolls its own tiny
//     parse, no panoscope-core dependency, same isolation as mesh-health).
//     `history(true)` replays the current token set on (re)connect. NATIVE —
//     liveliness is push-based presence; the router tells the watcher the
//     instant a token drops (session close, crash, partition), the
//     quiet-vs-dead distinction passive traffic metering can't make.
//   - Tracks each token's alive/dropped state and, on a timer, publishes a
//     per-site reading back on the bus in C0:
//       ops/presence/health/<site>   <- EnvHealthV1{domain:"presence"}
//   - Persists nothing.
//
// Honesty (docs/design/environment-health-surface.md §6, ADR 1040 gap 3):
//   - no token ever seen this run        → idle (nothing declares presence yet
//     — UNCONFIRMED, never a false green; also the honest reading against a
//     router whose ACL predates the liveliness grant — F-177 class).
//   Presence is an ALIVENESS COUNT, not a health alarm (env-health-surface.md
//   §3.4); ABSENCE IS NOT ILLNESS (§3.4 tenet 2) — a peer that leaves may be
//   deliberately off, so departures never grade hot:
//   - ≥1 alive                           → green (whatever recently left).
//   - tokens seen but ZERO alive         → idle (the fleet is now absent —
//     "quiet," not a failure; we don't assume peers must be always-on).
//   - never saw a token                  → idle (UNCONFIRMED).
//   - overall sample_count = alive count; dropped_recent = peers not alive whose
//     drop began within the last window — a NEUTRAL count in the detail, not a color.

use anyhow::{Context, Result};
use clap::Parser;
use prost::Message as _;
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};
use zenoh::qos::{CongestionControl, Priority};
use zenoh::sample::SampleKind;

// ---------------------------------------------------------------------------
// Wire types — keep field tags in lock-step with the canonical schema at
// ops/zenoh-trial-peers/time-health/proto/env_health_v1.proto.
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, prost::Message)]
struct PresencePeerV1 {
    #[prost(string, tag = "1")]
    token_key: String,
    #[prost(string, tag = "2")]
    write_root: String,
    #[prost(string, tag = "3")]
    label: String,
    #[prost(bool, tag = "4")]
    alive: bool,
    #[prost(int64, tag = "5")]
    since_unix_ms: i64,
    #[prost(uint64, tag = "6")]
    drop_count: u64,
}

#[derive(Clone, PartialEq, prost::Message)]
struct PresenceDetailV1 {
    #[prost(message, repeated, tag = "1")]
    peers: Vec<PresencePeerV1>,
    #[prost(uint32, tag = "2")]
    alive_total: u32,
    #[prost(uint32, tag = "3")]
    dropped_recent: u32,
}

#[derive(Clone, PartialEq, prost::Message)]
struct FabricDetailV1 {}

#[derive(Clone, PartialEq, prost::Message)]
struct TimeDetailV1 {}

#[derive(Clone, PartialEq, prost::Message)]
struct MeshDetailV1 {}

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
    #[prost(oneof = "EnvDetail", tags = "10, 11, 12, 13")]
    detail: Option<EnvDetail>,
}

#[derive(Clone, PartialEq, prost::Oneof)]
enum EnvDetail {
    #[prost(message, tag = "10")]
    Fabric(FabricDetailV1),
    #[prost(message, tag = "11")]
    Time(TimeDetailV1),
    #[prost(message, tag = "12")]
    Mesh(MeshDetailV1),
    #[prost(message, tag = "13")]
    Presence(PresenceDetailV1),
}

/// Split a presence-token key on its FIRST `/presence/` segment boundary —
/// the `<write_root>/presence/<label>` convention from
/// crates/panoscope-core/src/presence.rs (hand-rolled here; no dependency).
/// A non-convention key keeps the whole key as the label, write_root empty.
fn parse_token_key(key: &str) -> (String, String) {
    let chunks: Vec<&str> = key.split('/').collect();
    if let Some(idx) = chunks.iter().position(|c| *c == "presence") {
        if idx > 0 && idx + 1 < chunks.len() {
            return (chunks[..idx].join("/"), chunks[idx + 1..].join("/"));
        }
    }
    (String::new(), key.to_string())
}

/// One token's observed state across this meter run.
#[derive(Debug, Clone)]
struct TokenState {
    write_root: String,
    label: String,
    alive: bool,
    since_unix_ms: i64,    // when the current alive/dropped state began
    drop_count: u64,       // drops observed this run
    last_drop_unix_ms: i64, // 0 = never dropped (recent-churn signal)
}

/// The watcher's token table: liveliness Put = alive, Delete = dropped.
#[derive(Default)]
struct Tokens {
    map: HashMap<String, TokenState>,
}

impl Tokens {
    /// A token declared (SampleKind::Put) — first sighting inserts; a
    /// re-declare of a dropped token flips it back alive (since resets);
    /// a Put on an already-alive token changes nothing.
    fn observe_put(&mut self, key: &str, now_ms: i64) {
        let (write_root, label) = parse_token_key(key);
        let entry = self.map.entry(key.to_string()).or_insert(TokenState {
            write_root,
            label,
            alive: true,
            since_unix_ms: now_ms,
            drop_count: 0,
            last_drop_unix_ms: 0,
        });
        if !entry.alive {
            entry.alive = true;
            entry.since_unix_ms = now_ms;
        }
    }

    /// A token dropped (SampleKind::Delete) — session close/crash/partition.
    fn observe_delete(&mut self, key: &str, now_ms: i64) {
        let (write_root, label) = parse_token_key(key);
        let entry = self.map.entry(key.to_string()).or_insert(TokenState {
            write_root,
            label,
            alive: true,
            since_unix_ms: now_ms,
            drop_count: 0,
            last_drop_unix_ms: 0,
        });
        if entry.alive {
            entry.alive = false;
            entry.since_unix_ms = now_ms;
            entry.drop_count += 1;
            entry.last_drop_unix_ms = now_ms;
        }
    }
}

/// The 5-state honesty color for the whole reading — presence is an ALIVENESS
/// COUNT, not a health alarm (environment-health-surface.md §3.4). ABSENCE IS
/// NOT ILLNESS (§3.4 tenet 2): a peer that leaves may be deliberately off, so
/// neither a departure nor an empty fleet grades hot — we do not assume every
/// peer must be powered on. Drops are a neutral count in the detail, never
/// "churn-amber."
///   - never saw a token   → idle (UNCONFIRMED — also the honest reading against
///     a router whose ACL predates the liveliness grant).
///   - ≥1 alive            → green (regardless of how many recently left).
///   - tokens seen, 0 alive → idle (the fleet that was here is now absent — that
///     is "quiet," not a failure; a real always-on expectation is a future
///     roster-expectation overlay, not an assumption baked in here).
fn overall_color(ever_seen: bool, alive_total: u32, _drops_in_window: u32) -> &'static str {
    if !ever_seen {
        return "idle"; // nothing declares presence yet — never a false green
    }
    if alive_total == 0 {
        return "idle"; // absence is not illness — the fleet may be deliberately off
    }
    "green" // ≥1 alive; departures are a neutral count, never an alarm
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Build the per-site EnvHealthV1{domain:"presence"} from the token table.
fn roll_up(tokens: &Tokens, site: &str, window_secs: u32, now_ms: i64) -> EnvHealthV1 {
    let window_ms = i64::from(window_secs) * 1000;
    let mut peers: Vec<PresencePeerV1> = tokens
        .map
        .iter()
        .map(|(key, t)| PresencePeerV1 {
            token_key: key.clone(),
            write_root: t.write_root.clone(),
            label: t.label.clone(),
            alive: t.alive,
            since_unix_ms: t.since_unix_ms,
            drop_count: t.drop_count,
        })
        .collect();
    peers.sort_by(|a, b| a.token_key.cmp(&b.token_key)); // deterministic ordering
    let alive_total = peers.iter().filter(|p| p.alive).count() as u32;
    let dropped_recent = peers
        .iter()
        .filter(|p| !p.alive && now_ms - p.since_unix_ms <= window_ms)
        .count() as u32;
    let drops_in_window = tokens
        .map
        .values()
        .filter(|t| t.last_drop_unix_ms > 0 && now_ms - t.last_drop_unix_ms <= window_ms)
        .count() as u32;
    EnvHealthV1 {
        domain: "presence".to_string(),
        scope: site.to_string(),
        ts_unix_ms: now_ms,
        overall_color: overall_color(!peers.is_empty(), alive_total, drops_in_window).to_string(),
        sample_count: u64::from(alive_total),
        window_secs,
        detail: Some(EnvDetail::Presence(PresenceDetailV1 {
            peers,
            alive_total,
            dropped_recent,
        })),
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "goat-presence-health",
    about = "Presence domain of the environment-health surface — per-site liveliness presence roll-up (ADR 1040 gap 3 / ADR 1036)"
)]
struct Args {
    /// Zenoh router endpoint, e.g. `tls/goat-zenoh-router:7447`.
    #[arg(long, env = "PRESENCE_HEALTH_ENDPOINT")]
    endpoint: String,

    /// Site identifier — the `<site>` segment of ops/presence/health/<site>.
    #[arg(long, env = "PRESENCE_HEALTH_SITE")]
    site: String,

    /// Leaf X.509 cert (PEM) for mTLS. Required for a `tls/` endpoint.
    #[arg(long, env = "PRESENCE_HEALTH_CERT")]
    cert: Option<String>,
    /// Matching private key (PEM).
    #[arg(long, env = "PRESENCE_HEALTH_KEY_FILE")]
    key: Option<String>,
    /// CA root cert (PEM) used to verify the router.
    #[arg(long, env = "PRESENCE_HEALTH_ROOT_CA")]
    root_ca: Option<String>,

    /// Publish interval (ms). The published window_secs reflects it.
    #[arg(long, env = "PRESENCE_HEALTH_INTERVAL_MS", default_value_t = 30_000)]
    interval_ms: u64,

    /// Liveliness keyexpr to watch — the presence-token convention
    /// `<write_root>/presence/<label>` (crates/panoscope-core/src/presence.rs).
    #[arg(long, env = "PRESENCE_HEALTH_WATCH_KEY", default_value = "**/presence/**")]
    watch_key: String,
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
    let session = open_session(&args).await.context("open zenoh session")?;
    let health_key = format!("ops/presence/health/{}", args.site);
    let subscriber = session
        .liveliness()
        .declare_subscriber(&args.watch_key)
        .history(true) // replay the current token set on (re)connect
        .await
        .map_err(|e| anyhow::anyhow!("declare liveliness subscriber failed: {e}"))?;
    info!(
        zid = %session.zid(),
        endpoint = args.endpoint.as_str(),
        site = args.site.as_str(),
        watch_key = args.watch_key.as_str(),
        publishes = health_key.as_str(),
        "session open; watching liveliness tokens -> ops/presence/health/<site>"
    );

    let interval = Duration::from_millis(args.interval_ms);
    let mut tokens = Tokens::default();
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("install SIGTERM handler")?;

    loop {
        tokio::select! {
            sample = subscriber.recv_async() => {
                match sample {
                    Ok(s) => {
                        let key = s.key_expr().as_str().to_string();
                        match s.kind() {
                            SampleKind::Put => {
                                tokens.observe_put(&key, now_unix_ms());
                                info!(token = key.as_str(), "presence token alive");
                            }
                            SampleKind::Delete => {
                                tokens.observe_delete(&key, now_unix_ms());
                                info!(token = key.as_str(), "presence token dropped");
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "liveliness subscriber channel closed; shutting down");
                        break;
                    }
                }
            }
            _ = ticker.tick() => {
                let window_secs = (interval.as_secs() as u32).max(1);
                let reading = roll_up(&tokens, &args.site, window_secs, now_unix_ms());
                let n = reading.sample_count;
                let overall = reading.overall_color.clone();
                let payload = reading.encode_to_vec();
                match session
                    .put(&health_key, payload)
                    .congestion_control(CongestionControl::Block)
                    .priority(Priority::InteractiveHigh)
                    .await
                {
                    Ok(()) => info!(key = health_key.as_str(), alive = n, overall = overall.as_str(), "published presence health"),
                    Err(e) => warn!(error = %e, "publish presence health failed"),
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

    const ANCHOR: &str = "e7a9c75b00112233445566778899aabb";

    fn key(label: &str) -> String {
        format!("{ANCHOR}/presence/{label}")
    }

    #[test]
    fn parse_splits_on_first_presence_segment() {
        // ZAGROS slot-anchor root.
        let (root, label) = parse_token_key(&key("tarsier-alpha"));
        assert_eq!(root, ANCHOR);
        assert_eq!(label, "tarsier-alpha");
        // Multi-chunk ops infra root + a label containing `presence` stays whole.
        let (root, label) = parse_token_key("ops/foo/presence/a/presence/b");
        assert_eq!(root, "ops/foo");
        assert_eq!(label, "a/presence/b");
        // Non-convention keys keep the whole key as the label.
        let (root, label) = parse_token_key("presence/orphan");
        assert_eq!(root, "");
        assert_eq!(label, "presence/orphan");
        let (root, label) = parse_token_key("notional/presence");
        assert_eq!(root, "");
        assert_eq!(label, "notional/presence");
    }

    #[test]
    fn color_is_aliveness_count_not_alarm() {
        // Never a false green: no token ever seen this run → idle.
        assert_eq!(overall_color(false, 0, 0), "idle");
        // Alive set → green.
        assert_eq!(overall_color(true, 3, 0), "green");
        // Absence is not illness (§3.4 tenet 2): departures NEVER grade hot —
        // an alive set with churn stays green (drops are a neutral count), and
        // a now-empty fleet is idle ("quiet"), never amber/red. Peers may be
        // deliberately off; we don't assume always-on.
        assert_eq!(overall_color(true, 2, 1), "green");
        assert_eq!(overall_color(true, 0, 0), "idle");
        assert_eq!(overall_color(true, 0, 2), "idle");
    }

    #[test]
    fn rollup_builds_presence_envelope() {
        let mut tokens = Tokens::default();
        let t0 = 1_000_000;
        tokens.observe_put(&key("b-peer"), t0);
        tokens.observe_put(&key("a-peer"), t0);
        tokens.observe_put("notional/presence/kingfisher", t0);
        let e = roll_up(&tokens, "zagros-sandbox", 30, t0 + 1000);
        assert_eq!(e.domain, "presence");
        assert_eq!(e.scope, "zagros-sandbox");
        assert_eq!(e.overall_color, "green");
        assert_eq!(e.sample_count, 3);
        assert_eq!(e.window_secs, 30);
        match e.detail {
            Some(EnvDetail::Presence(ref d)) => {
                assert_eq!(d.alive_total, 3);
                assert_eq!(d.dropped_recent, 0);
                // Sorted by token_key for determinism.
                let keys: Vec<&str> = d.peers.iter().map(|p| p.token_key.as_str()).collect();
                assert_eq!(
                    keys,
                    vec![
                        key("a-peer").as_str(),
                        key("b-peer").as_str(),
                        "notional/presence/kingfisher",
                    ]
                );
                let a = &d.peers[0];
                assert_eq!(a.write_root, ANCHOR);
                assert_eq!(a.label, "a-peer");
                assert!(a.alive);
                assert_eq!(a.since_unix_ms, t0);
                assert_eq!(a.drop_count, 0);
            }
            _ => panic!("expected Presence detail"),
        }
        // Round-trips through protobuf.
        let bytes = e.encode_to_vec();
        let back = EnvHealthV1::decode(bytes.as_slice()).expect("decode");
        assert_eq!(back.domain, "presence");
        assert_eq!(back.overall_color, "green");
        assert_eq!(back.sample_count, 3);
    }

    #[test]
    fn drops_do_not_alarm_and_absence_is_idle() {
        let mut tokens = Tokens::default();
        let t0 = 1_000_000;
        tokens.observe_put(&key("steady"), t0);
        tokens.observe_put(&key("flappy"), t0);
        tokens.observe_delete(&key("flappy"), t0 + 5_000);
        // One alive + one recent drop → GREEN (§3.4 tenet 2: a departure is not
        // illness). The drop is still counted (dropped_recent=1), just not colored.
        let e = roll_up(&tokens, "x", 30, t0 + 10_000);
        assert_eq!(e.overall_color, "green");
        assert_eq!(e.sample_count, 1);
        match e.detail {
            Some(EnvDetail::Presence(ref d)) => {
                assert_eq!(d.alive_total, 1);
                assert_eq!(d.dropped_recent, 1);
            }
            _ => panic!("expected Presence detail"),
        }
        // The whole fleet goes quiet → IDLE, never red — the peers may be
        // deliberately off; absence is not a failure.
        tokens.observe_delete(&key("steady"), t0 + 11_000);
        let e = roll_up(&tokens, "x", 30, t0 + 12_000);
        assert_eq!(e.overall_color, "idle");
        assert_eq!(e.sample_count, 0);
    }

    #[test]
    fn dropped_recent_windows_out_old_drops() {
        let mut tokens = Tokens::default();
        let t0 = 1_000_000;
        tokens.observe_put(&key("alive"), t0);
        tokens.observe_put(&key("gone"), t0);
        tokens.observe_delete(&key("gone"), t0 + 1_000);
        // Inside the window: the drop is counted (dropped_recent=1) but does NOT
        // color — still green (≥1 alive). The count is a neutral signal (§3.4).
        let e = roll_up(&tokens, "x", 30, t0 + 10_000);
        match e.detail {
            Some(EnvDetail::Presence(ref d)) => assert_eq!(d.dropped_recent, 1),
            _ => panic!("expected Presence detail"),
        }
        assert_eq!(e.overall_color, "green");
        // Past the window: the drop ages out — dropped_recent returns to 0.
        let e = roll_up(&tokens, "x", 30, t0 + 1_000 + 31_000);
        match e.detail {
            Some(EnvDetail::Presence(ref d)) => {
                assert_eq!(d.dropped_recent, 0);
                assert_eq!(d.alive_total, 1);
            }
            _ => panic!("expected Presence detail"),
        }
        assert_eq!(e.overall_color, "green");
    }

    #[test]
    fn token_transitions_put_delete_put() {
        let mut tokens = Tokens::default();
        let t0 = 1_000_000;
        tokens.observe_put(&key("p"), t0);
        // Re-put while alive changes nothing (since stays).
        tokens.observe_put(&key("p"), t0 + 1_000);
        assert_eq!(tokens.map[&key("p")].since_unix_ms, t0);
        // Drop: increments once, flips state.
        tokens.observe_delete(&key("p"), t0 + 2_000);
        let s = &tokens.map[&key("p")];
        assert!(!s.alive);
        assert_eq!(s.drop_count, 1);
        assert_eq!(s.since_unix_ms, t0 + 2_000);
        // Duplicate delete does not double-count.
        tokens.observe_delete(&key("p"), t0 + 3_000);
        assert_eq!(tokens.map[&key("p")].drop_count, 1);
        // Re-declare: alive again, drop_count still 1, since resets.
        tokens.observe_put(&key("p"), t0 + 4_000);
        let s = &tokens.map[&key("p")];
        assert!(s.alive);
        assert_eq!(s.drop_count, 1);
        assert_eq!(s.since_unix_ms, t0 + 4_000);
    }

    #[test]
    fn idle_when_no_token_ever_seen() {
        let tokens = Tokens::default();
        let e = roll_up(&tokens, "zagros-sandbox", 30, 1_000_000);
        assert_eq!(e.overall_color, "idle");
        assert_eq!(e.sample_count, 0);
        match e.detail {
            Some(EnvDetail::Presence(ref d)) => {
                assert!(d.peers.is_empty());
                assert_eq!(d.alive_total, 0);
                assert_eq!(d.dropped_recent, 0);
            }
            _ => panic!("expected Presence detail"),
        }
    }
}
