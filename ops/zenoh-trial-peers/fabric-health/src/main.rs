// SPDX-License-Identifier: Apache-2.0
//
// goat-fabric-health — fabric health + per-producer footprint meter.
//
// ADR 1034 / docs/design/fabric-health-surface.md. The "Reveal" pillar of
// the QoS design (ADR 1033), built first.
//
// Shape (templated on goat-audit-subscriber, minus persistence):
//   - Opens an mTLS Zenoh session as a client peer.
//   - Subscribes `**` and meters every sample by traffic class + producer
//     into a rolling accumulator. Writes nothing durable.
//   - On a timer, computes per-class load vs operator-set capacity targets
//     (the knee) and per-producer footprint, and publishes them BACK onto
//     the bus in the control class (C0, Block + high priority) so the health
//     signal is never starved by the load it reports on:
//       ops/fabric/health/<site>     -> FabricHealthV1
//       ops/fabric/footprint/<peer>  -> ProducerFootprintV1
//
// Protobuf wire types are hand-written prost-derive structs matching
// proto/fabric_health_v1.proto (the canonical schema) — no protoc/build.rs.

use anyhow::{Context, Result};
use clap::Parser;
use prost::Message as _;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, warn};
use zenoh::qos::{CongestionControl, Priority};

// ---------------------------------------------------------------------------
// Wire types — keep field tags in lock-step with proto/fabric_health_v1.proto.
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, prost::Message)]
struct ClassHealthV1 {
    #[prost(string, tag = "1")]
    class: String,
    #[prost(double, tag = "2")]
    sample_rate: f64,
    #[prost(double, tag = "3")]
    byte_rate: f64,
    #[prost(double, tag = "4")]
    target_sample_rate: f64,
    #[prost(double, tag = "5")]
    target_byte_rate: f64,
    #[prost(double, tag = "6")]
    headroom_pct: f64,
    #[prost(string, tag = "7")]
    color: String,
    #[prost(uint64, tag = "8")]
    sample_count: u64,
}

#[derive(Clone, PartialEq, prost::Message)]
struct FabricHealthV1 {
    #[prost(string, tag = "1")]
    site: String,
    #[prost(int64, tag = "2")]
    ts_unix_ms: i64,
    #[prost(message, repeated, tag = "3")]
    classes: Vec<ClassHealthV1>,
    #[prost(string, tag = "4")]
    overall_color: String,
    #[prost(uint32, tag = "5")]
    active_producers: u32,
    #[prost(uint32, tag = "6")]
    window_secs: u32,
}

#[derive(Clone, PartialEq, prost::Message)]
struct ClassShareV1 {
    #[prost(string, tag = "1")]
    class: String,
    #[prost(double, tag = "2")]
    sample_rate: f64,
    #[prost(double, tag = "3")]
    byte_rate: f64,
}

#[derive(Clone, PartialEq, prost::Message)]
struct ProducerFootprintV1 {
    #[prost(string, tag = "1")]
    peer: String,
    #[prost(int64, tag = "2")]
    ts_unix_ms: i64,
    #[prost(double, tag = "3")]
    sample_rate: f64,
    #[prost(double, tag = "4")]
    byte_rate: f64,
    #[prost(message, repeated, tag = "5")]
    shares: Vec<ClassShareV1>,
    #[prost(uint32, tag = "6")]
    rank: u32,
    #[prost(bool, tag = "7")]
    near_limit: bool,
}

// ---------------------------------------------------------------------------

const CLASS_NAMES: [&str; 4] = ["C0", "C1", "C2", "C3"];
/// A producer is flagged near_limit only if its dominant class is amber/red
/// AND it ranks in the top-N by byte-rate. Advisory until caps exist.
const NEAR_LIMIT_TOP_N: u32 = 3;
/// Below this observed rate, a class carries too little traffic to confirm
/// health from — it reads `idle` (UNCONFIRMED), never a false `green`
/// (ADR 1034 measurement-trust). Window-independent (a rate), and the same
/// quiet condition that the future idle-probe fires on. msgs/s.
const IDLE_RATE_FLOOR: f64 = 1.0;

#[derive(Parser, Debug)]
#[command(
    name = "goat-fabric-health",
    about = "Fabric health + per-producer footprint meter (ADR 1034)."
)]
struct Args {
    /// Zenoh router endpoint, e.g. `tls/goat-zenoh-router:7447`.
    #[arg(long, env = "FABRIC_HEALTH_ENDPOINT")]
    endpoint: String,

    /// Site identifier — the `<site>` segment of ops/fabric/health/<site>.
    #[arg(long, env = "FABRIC_HEALTH_SITE")]
    site: String,

    /// Leaf X.509 cert (PEM) for mTLS. Required for a `tls/` endpoint.
    #[arg(long, env = "FABRIC_HEALTH_CERT")]
    cert: Option<String>,
    /// Matching private key (PEM).
    #[arg(long, env = "FABRIC_HEALTH_KEY_FILE")]
    key: Option<String>,
    /// CA root cert (PEM) used to verify the router.
    #[arg(long, env = "FABRIC_HEALTH_ROOT_CA")]
    root_ca: Option<String>,

    /// Publish interval (ms). Rates are computed over the interval just
    /// elapsed; the published `window_secs` reflects it.
    #[arg(long, env = "FABRIC_HEALTH_INTERVAL_MS", default_value_t = 5000)]
    interval_ms: u64,

    /// Per-class capacity targets, msgs/s, comma-separated C0,C1,C2,C3
    /// (the knee — operator-set from ADR 1012 measurements). 0 = unset
    /// (that class reads `unmetered` with headroom -1 — never green — since
    /// load without a capacity reference is not evidence of health).
    #[arg(
        long,
        env = "FABRIC_HEALTH_TARGET_MSGS",
        default_value = "5000,20000,50000,200000"
    )]
    target_msgs: String,

    /// Per-class capacity targets, bytes/s, comma-separated C0,C1,C2,C3.
    #[arg(
        long,
        env = "FABRIC_HEALTH_TARGET_BYTES",
        default_value = "1000000,20000000,100000000,500000000"
    )]
    target_bytes: String,
}

/// (count, bytes) accumulated since the last publish, keyed by (class_idx, producer).
type Accum = HashMap<(u8, String), (u64, u64)>;
/// Per-class tally (index 0..3 = C0..C3).
type ClassArr = [u64; 4];
/// One producer's per-class (count, bytes) tallies.
type ProducerTally = (ClassArr, ClassArr);

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let target_msgs = parse_targets(&args.target_msgs).context("--target-msgs")?;
    let target_bytes = parse_targets(&args.target_bytes).context("--target-bytes")?;

    let session = Arc::new(open_session(&args).await.context("open zenoh session")?);
    info!(zid = %session.zid(), endpoint = args.endpoint.as_str(), site = args.site.as_str(), "session open");

    let accum: Arc<Mutex<Accum>> = Arc::new(Mutex::new(HashMap::new()));

    // Publisher timer task — snapshot+reset the accumulator each interval,
    // compute, publish health + footprints.
    let pub_session = session.clone();
    let pub_accum = accum.clone();
    let site = args.site.clone();
    let interval = Duration::from_millis(args.interval_ms);
    let pub_handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await; // first tick is immediate; skip it (empty window)
        loop {
            ticker.tick().await;
            let snapshot: Accum = {
                let mut g = pub_accum.lock().await;
                std::mem::take(&mut *g)
            };
            if let Err(e) = publish_round(
                &pub_session,
                &site,
                &snapshot,
                interval.as_secs_f64(),
                &target_msgs,
                &target_bytes,
            )
            .await
            {
                warn!(error = %e, "publish round failed");
            }
        }
    });

    // Subscriber loop — meter every sample. Inlined so we don't have to name
    // the subscriber's concrete handler type.
    let subscriber = session
        .declare_subscriber("**")
        .await
        .map_err(|e| anyhow::anyhow!("declare_subscriber(**) failed: {e}"))?;
    info!("metering ** — fabric-health publishing every {} ms", args.interval_ms);

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("shutdown signal received");
                break;
            }
            recv = subscriber.recv_async() => {
                let sample = match recv {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(error = %e, "subscriber recv error; meter loop ending");
                        break;
                    }
                };
                let key = sample.key_expr().as_str().to_string();
                let len = sample.payload().to_bytes().len() as u64;
                let (class_idx, producer) = classify(&key);
                let mut g = accum.lock().await;
                let e = g.entry((class_idx, producer)).or_insert((0, 0));
                e.0 += 1;
                e.1 += len;
            }
        }
    }

    pub_handle.abort();
    Ok(())
}

/// Map a key-expression to (class index 0..3, producer identity). Classes
/// per ADR 1033; producer is the key's identity segment (first segment, or
/// second for the `volatile/<peer>/…` class). On the trial fabric the first
/// segment is the vendor; canonical production prefixes map to their class.
fn classify(key: &str) -> (u8, String) {
    let segs: Vec<&str> = key.split('/').collect();
    let p0 = segs.first().copied().unwrap_or("");
    let class: u8 = if key.starts_with("ops/fabric/") || matches!(p0, "cmd" | "task" | "ack") {
        0 // C0 control / kill-chain + the health signal itself
    } else if matches!(p0, "data" | "release") {
        1 // C1 mission
    } else if p0 == "volatile" {
        3 // C3 bulk / scavenger (the drop-first spillway)
    } else {
        // C2 situational: agents/telemetry/health + <vendor>/** trial topics +
        // ops/services/config/attest (anything not explicitly higher).
        2
    };
    let producer = if p0 == "volatile" {
        segs.get(1).copied().unwrap_or("unknown")
    } else if p0.is_empty() {
        "unknown"
    } else {
        p0
    };
    (class, producer.to_string())
}

/// Honest per-class assessment (ADR 1034 measurement-trust). Returns the
/// 5-state color + headroom; never a false green. Rate below the idle floor
/// -> `idle` (UNCONFIRMED, headroom -1); no capacity target -> `unmetered`
/// (load shown, no reference, -1); otherwise grounded -> green/amber/red +
/// real headroom 0..1. Color + headroom are driven by the WORST of the
/// msgs/s and bytes/s ratios.
fn assess_class(
    sample_rate: f64,
    byte_rate: f64,
    target_msgs: f64,
    target_bytes: f64,
) -> (&'static str, f64) {
    if sample_rate < IDLE_RATE_FLOOR {
        return ("idle", -1.0);
    }
    if target_msgs <= 0.0 && target_bytes <= 0.0 {
        return ("unmetered", -1.0);
    }
    let rm = if target_msgs > 0.0 { sample_rate / target_msgs } else { 0.0 };
    let rb = if target_bytes > 0.0 { byte_rate / target_bytes } else { 0.0 };
    let r = rm.max(rb);
    let color = if r > 0.9 {
        "red"
    } else if r >= 0.7 {
        "amber"
    } else {
        "green"
    };
    (color, (1.0 - r).max(0.0))
}

/// Overall dipstick = the worst *grounded* pressure across classes. If no
/// class is grounded (all idle/unmetered), overall is `idle` — the fabric is
/// quiet and health is UNCONFIRMED, never falsely `green`.
fn overall_color(colors: &[String]) -> String {
    let mut any_grounded = false;
    let mut worst = 0u8; // 0 green, 1 amber, 2 red
    for c in colors {
        match c.as_str() {
            "green" => {
                any_grounded = true;
            }
            "amber" => {
                any_grounded = true;
                worst = worst.max(1);
            }
            "red" => {
                any_grounded = true;
                worst = worst.max(2);
            }
            _ => {} // idle / unmetered — not grounded, contribute nothing
        }
    }
    if !any_grounded {
        "idle".to_string()
    } else {
        match worst {
            2 => "red",
            1 => "amber",
            _ => "green",
        }
        .to_string()
    }
}

async fn publish_round(
    session: &zenoh::Session,
    site: &str,
    snapshot: &Accum,
    elapsed_s: f64,
    target_msgs: &[f64; 4],
    target_bytes: &[f64; 4],
) -> Result<()> {
    let ts = ts_unix_ms();
    let elapsed_s = if elapsed_s <= 0.0 { 1.0 } else { elapsed_s };

    // --- per-class aggregation ---
    let mut class_count: ClassArr = [0; 4];
    let mut class_bytes: ClassArr = [0; 4];
    // --- per-producer aggregation: producer -> (per-class count, per-class bytes) ---
    let mut producers: HashMap<&str, ProducerTally> = HashMap::new();
    for ((cidx, producer), (count, bytes)) in snapshot {
        let c = *cidx as usize;
        if c < 4 {
            class_count[c] += count;
            class_bytes[c] += bytes;
            let entry = producers.entry(producer.as_str()).or_insert(([0; 4], [0; 4]));
            entry.0[c] += count;
            entry.1[c] += bytes;
        }
    }

    let mut classes = Vec::with_capacity(4);
    for c in 0..4usize {
        let sample_rate = class_count[c] as f64 / elapsed_s;
        let byte_rate = class_bytes[c] as f64 / elapsed_s;
        let (color, headroom) =
            assess_class(sample_rate, byte_rate, target_msgs[c], target_bytes[c]);
        classes.push(ClassHealthV1 {
            class: CLASS_NAMES[c].to_string(),
            sample_rate,
            byte_rate,
            target_sample_rate: target_msgs[c],
            target_byte_rate: target_bytes[c],
            headroom_pct: headroom,
            color: color.to_string(),
            sample_count: class_count[c],
        });
    }
    let overall = overall_color(&classes.iter().map(|c| c.color.clone()).collect::<Vec<_>>());
    // Snapshot the assessed colors before `classes` moves into the health
    // message — used for the per-producer near_limit flag below.
    let class_colors: Vec<String> = classes.iter().map(|c| c.color.clone()).collect();

    let health = FabricHealthV1 {
        site: site.to_string(),
        ts_unix_ms: ts,
        classes,
        overall_color: overall,
        active_producers: producers.len() as u32,
        window_secs: elapsed_s.round() as u32,
    };
    let key = format!("ops/fabric/health/{site}");
    publish_c0(session, &key, health.encode_to_vec()).await?;

    // --- per-producer footprints, ranked by total byte-rate ---
    let mut ranked: Vec<(&str, f64, f64, ClassArr, ClassArr)> = producers
        .iter()
        .map(|(p, (cc, cb))| {
            let total_b: u64 = cb.iter().sum();
            let total_c: u64 = cc.iter().sum();
            (*p, total_c as f64 / elapsed_s, total_b as f64 / elapsed_s, *cc, *cb)
        })
        .collect();
    ranked.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    for (rank0, (peer, srate, brate, cc, cb)) in ranked.iter().enumerate() {
        let rank = (rank0 + 1) as u32;
        let mut shares = Vec::new();
        let mut dominant_class = 0usize;
        let mut dominant_bytes = 0u64;
        for c in 0..4usize {
            if cc[c] == 0 && cb[c] == 0 {
                continue;
            }
            if cb[c] > dominant_bytes {
                dominant_bytes = cb[c];
                dominant_class = c;
            }
            shares.push(ClassShareV1 {
                class: CLASS_NAMES[c].to_string(),
                sample_rate: cc[c] as f64 / elapsed_s,
                byte_rate: cb[c] as f64 / elapsed_s,
            });
        }
        let dom = class_colors[dominant_class].as_str();
        let near_limit = rank <= NEAR_LIMIT_TOP_N && (dom == "amber" || dom == "red");
        let fp = ProducerFootprintV1 {
            peer: (*peer).to_string(),
            ts_unix_ms: ts,
            sample_rate: *srate,
            byte_rate: *brate,
            shares,
            rank,
            near_limit,
        };
        let key = format!("ops/fabric/footprint/{peer}");
        publish_c0(session, &key, fp.encode_to_vec()).await?;
    }
    Ok(())
}

/// Publish in the control class: Block congestion-control + high priority so
/// the health signal is never starved by the load it reports on.
async fn publish_c0(session: &zenoh::Session, key: &str, payload: Vec<u8>) -> Result<()> {
    session
        .put(key, payload)
        .congestion_control(CongestionControl::Block)
        .priority(Priority::InteractiveHigh)
        .await
        .map_err(|e| anyhow::anyhow!("put({key}) failed: {e}"))
}

fn parse_targets(s: &str) -> Result<[f64; 4]> {
    let parts: Vec<f64> = s
        .split(',')
        .map(|p| p.trim().parse::<f64>())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("parse targets {s:?}: {e}"))?;
    if parts.len() != 4 {
        anyhow::bail!("expected 4 comma-separated targets (C0,C1,C2,C3), got {}", parts.len());
    }
    Ok([parts[0], parts[1], parts[2], parts[3]])
}

fn ts_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{assess_class, classify, overall_color, parse_targets, IDLE_RATE_FLOOR};

    fn colors(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // -----------------------------------------------------------------------
    // THE invariant (ADR 1034): nothing-heard is never green. A health surface
    // with no evidence must report UNCONFIRMED (`idle`), not `green`.
    // -----------------------------------------------------------------------

    #[test]
    fn overall_empty_input_is_idle_never_green() {
        // Zero classes reported at all — no evidence — must not fall through
        // to green.
        assert_eq!(overall_color(&[]), "idle");
    }

    #[test]
    fn overall_all_idle_is_idle_never_green() {
        // The whole fabric quiet: every class below the idle floor.
        assert_eq!(overall_color(&colors(&["idle", "idle", "idle", "idle"])), "idle");
    }

    #[test]
    fn overall_all_unmetered_is_idle_never_green() {
        // Traffic flowing but no capacity reference anywhere — load without a
        // knee is not evidence of health.
        assert_eq!(
            overall_color(&colors(&["unmetered", "unmetered", "unmetered", "unmetered"])),
            "idle"
        );
    }

    #[test]
    fn overall_mixed_idle_unmetered_is_idle_never_green() {
        assert_eq!(
            overall_color(&colors(&["idle", "unmetered", "idle", "unmetered"])),
            "idle"
        );
    }

    #[test]
    fn overall_unrecognized_color_is_not_grounded() {
        // Fail-closed: a garbled/unknown state string must never count as
        // evidence of health. Alone it resolves to idle, not green.
        assert_eq!(overall_color(&colors(&["chartreuse"])), "idle");
        assert_eq!(overall_color(&colors(&[""])), "idle");
    }

    // -----------------------------------------------------------------------
    // Precedence: red dominates amber dominates green, across grounded classes.
    // -----------------------------------------------------------------------

    #[test]
    fn overall_single_red_dominates() {
        assert_eq!(overall_color(&colors(&["green", "green", "red", "green"])), "red");
    }

    #[test]
    fn overall_red_dominates_amber() {
        assert_eq!(overall_color(&colors(&["amber", "red", "green", "idle"])), "red");
    }

    #[test]
    fn overall_amber_dominates_green() {
        assert_eq!(overall_color(&colors(&["green", "amber", "green", "green"])), "amber");
    }

    #[test]
    fn overall_red_not_diluted_by_quiet_classes() {
        // One grounded red among otherwise idle/unmetered classes is still red.
        assert_eq!(overall_color(&colors(&["idle", "unmetered", "red", "idle"])), "red");
    }

    #[test]
    fn overall_amber_among_quiet_classes_is_amber() {
        assert_eq!(overall_color(&colors(&["idle", "amber", "idle", "unmetered"])), "amber");
    }

    // -----------------------------------------------------------------------
    // The one true green: green requires at least one grounded green class and
    // no grounded class worse than green.
    // -----------------------------------------------------------------------

    #[test]
    fn overall_all_green_with_evidence_is_green() {
        assert_eq!(overall_color(&colors(&["green", "green", "green", "green"])), "green");
    }

    #[test]
    fn overall_grounded_green_with_quiet_classes_is_green() {
        // Deliberate per the design (fabric-health-surface.md): overall is the
        // worst *grounded* pressure; classes with no traffic contribute
        // nothing. One measured-green class + three quiet ones reads green —
        // the per-class `idle` states still carry the UNCONFIRMED signal.
        assert_eq!(overall_color(&colors(&["green", "idle", "idle", "unmetered"])), "green");
    }

    // -----------------------------------------------------------------------
    // assess_class: the 5-state per-class grading that feeds overall_color.
    // -----------------------------------------------------------------------

    #[test]
    fn assess_below_idle_floor_is_idle_with_sentinel_headroom() {
        // No traffic at all.
        let (color, headroom) = assess_class(0.0, 0.0, 5000.0, 1_000_000.0);
        assert_eq!(color, "idle");
        assert_eq!(headroom, -1.0);
        // Just below the floor — still UNCONFIRMED, even with targets set.
        let (color, _) = assess_class(IDLE_RATE_FLOOR - 0.01, 1e9, 5000.0, 1_000_000.0);
        assert_eq!(color, "idle");
    }

    #[test]
    fn assess_no_targets_is_unmetered_not_green() {
        // Real traffic, no capacity reference: load shown, health unconfirmed.
        let (color, headroom) = assess_class(100.0, 100_000.0, 0.0, 0.0);
        assert_eq!(color, "unmetered");
        assert_eq!(headroom, -1.0);
    }

    #[test]
    fn assess_grounded_green_with_real_headroom() {
        // 100 msgs/s against a 5000 msgs/s knee -> ratio 0.02, green.
        let (color, headroom) = assess_class(100.0, 10_000.0, 5000.0, 1_000_000.0);
        assert_eq!(color, "green");
        assert!((headroom - 0.98).abs() < 1e-9);
    }

    #[test]
    fn assess_amber_band_boundaries() {
        // Exactly 0.7 of the knee -> amber (>= 0.7).
        let (color, _) = assess_class(3500.0, 0.0, 5000.0, 1_000_000.0);
        assert_eq!(color, "amber");
        // Exactly 0.9 -> still amber (red is strictly > 0.9).
        let (color, _) = assess_class(4500.0, 0.0, 5000.0, 1_000_000.0);
        assert_eq!(color, "amber");
    }

    #[test]
    fn assess_red_above_ninety_pct_with_floored_headroom() {
        let (color, headroom) = assess_class(6000.0, 0.0, 5000.0, 1_000_000.0);
        assert_eq!(color, "red");
        // Over the knee: headroom clamps at 0, never negative for grounded.
        assert_eq!(headroom, 0.0);
    }

    #[test]
    fn assess_worst_of_msgs_and_bytes_drives_color() {
        // msgs ratio green (0.02) but bytes ratio red (0.95): worst wins.
        let (color, _) = assess_class(100.0, 950_000.0, 5000.0, 1_000_000.0);
        assert_eq!(color, "red");
    }

    #[test]
    fn assess_single_sided_target_still_grounds() {
        // Only a bytes knee set: msgs side contributes 0, bytes side grades.
        let (color, _) = assess_class(100.0, 950_000.0, 0.0, 1_000_000.0);
        assert_eq!(color, "red");
        let (color, _) = assess_class(100.0, 10_000.0, 0.0, 1_000_000.0);
        assert_eq!(color, "green");
    }

    // -----------------------------------------------------------------------
    // Supporting pure functions.
    // -----------------------------------------------------------------------

    #[test]
    fn classify_maps_classes_and_producers() {
        assert_eq!(classify("ops/fabric/health/site1"), (0, "ops".to_string()));
        assert_eq!(classify("cmd/fire/x"), (0, "cmd".to_string()));
        assert_eq!(classify("data/track/1"), (1, "data".to_string()));
        assert_eq!(classify("release/set/2"), (1, "release".to_string()));
        assert_eq!(classify("volatile/peer-a/blob"), (3, "peer-a".to_string()));
        assert_eq!(classify("vendorx/telemetry"), (2, "vendorx".to_string()));
        assert_eq!(classify(""), (2, "unknown".to_string()));
    }

    #[test]
    fn parse_targets_happy_and_sad() {
        assert_eq!(parse_targets("1,2,3,4").unwrap(), [1.0, 2.0, 3.0, 4.0]);
        assert!(parse_targets("1,2,3").is_err());
        assert!(parse_targets("a,b,c,d").is_err());
    }
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
