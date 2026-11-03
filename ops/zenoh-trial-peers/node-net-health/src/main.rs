// SPDX-License-Identifier: Apache-2.0
//
// goat-node-net-health — the `net` domain of the environment-health surface.
//
// docs/design/node-net-health.md + ADR 1036 (env-health contract). A normal
// mTLS Zenoh client peer (templated on goat-mesh-health):
//   - Reads the classified per-node connection snapshot at a fixed path
//     (written atomically by the node-net-health-snapshot systemd timer —
//     ops/node-net-health/nnh-snapshot.py — bind-mounted read-only). The
//     snapshot already carries the 4-bucket classification (mesh | fabric |
//     baseline | undeclared) against the node-class baseline registry, the
//     per-peer mesh plane (active/stale by handshake age), and the persisted
//     first/last-seen for undeclared rows.
//   - Grades it with the 5-state honesty colors and, on a timer, publishes a
//     per-node reading back on the bus in C0:
//       ops/net/health/<node>   <- EnvHealthV1{domain:"net"}
//   - Subscribes nothing. Persists nothing. Observation, not enforcement
//     (Block 17 egress-proxy owns enforcement).
//
// Honesty:
//   - undeclared_count > 0      → amber ("N undeclared") — never silently green.
//   - every mesh peer stale while connected → amber (enrolled-but-not-exchanging).
//   - snapshot stale or missing → the whole reading publishes `unmetered`.
//   - green only when grounded by a fresh snapshot with zero undeclared.

use anyhow::{Context, Result};
use clap::Parser;
use prost::Message as _;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};
use zenoh::qos::{CongestionControl, Priority};

// ---------------------------------------------------------------------------
// Wire types — keep field tags in lock-step with the canonical schema at
// ops/zenoh-trial-peers/time-health/proto/env_health_v1.proto (arm 14).
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, prost::Message, serde::Serialize)]
struct NodeMeshPeerV1 {
    #[prost(string, tag = "1")]
    peer_fqdn: String,
    #[prost(string, tag = "2")]
    peer_ip: String,
    #[prost(bool, tag = "3")]
    active: bool,
    #[prost(int64, tag = "4")]
    handshake_age_ms: i64,
    #[prost(string, tag = "5")]
    conn_type: String,
    #[prost(uint64, tag = "6")]
    rx_bytes_delta: u64,
    #[prost(uint64, tag = "7")]
    tx_bytes_delta: u64,
    #[prost(double, tag = "8")]
    latency_ms: f64,
}

#[derive(Clone, PartialEq, prost::Message, serde::Serialize)]
struct NodeFabricSessionV1 {
    #[prost(string, tag = "1")]
    zid: String,
    #[prost(string, tag = "2")]
    whatami: String,
    #[prost(uint32, tag = "3")]
    links: u32,
}

#[derive(Clone, PartialEq, prost::Message, serde::Serialize)]
struct NodeConnV1 {
    #[prost(string, tag = "1")]
    bucket: String,
    #[prost(string, tag = "2")]
    proto: String,
    #[prost(string, tag = "3")]
    state: String,
    #[prost(string, tag = "4")]
    local_addr: String,
    #[prost(string, tag = "5")]
    remote_addr: String,
    #[prost(uint32, tag = "6")]
    pid: u32,
    #[prost(string, tag = "7")]
    comm: String,
    #[prost(string, tag = "8")]
    container: String,
    #[prost(string, tag = "9")]
    baseline_rule: String,
    #[prost(int64, tag = "10")]
    first_seen_unix_ms: i64,
    #[prost(int64, tag = "11")]
    last_seen_unix_ms: i64,
}

#[derive(Clone, PartialEq, prost::Message, serde::Serialize)]
struct NetDetailV1 {
    #[prost(message, repeated, tag = "1")]
    mesh_peers: Vec<NodeMeshPeerV1>,
    #[prost(uint32, tag = "2")]
    mesh_active: u32,
    #[prost(uint32, tag = "3")]
    mesh_stale: u32,
    #[prost(message, repeated, tag = "4")]
    fabric_sessions: Vec<NodeFabricSessionV1>,
    #[prost(message, repeated, tag = "5")]
    conns: Vec<NodeConnV1>,
    #[prost(uint32, tag = "6")]
    undeclared_count: u32,
    #[prost(string, tag = "7")]
    node_class: String,
    #[prost(uint32, tag = "8")]
    snapshot_gap_secs: u32,
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
    #[prost(oneof = "EnvDetail", tags = "14")]
    detail: Option<EnvDetail>,
}

#[derive(Clone, PartialEq, prost::Oneof, serde::Serialize)]
enum EnvDetail {
    #[prost(message, tag = "14")]
    Net(NetDetailV1),
}

// ---------------------------------------------------------------------------
// Incoming snapshot — the JSON ops/node-net-health/nnh-snapshot.py writes.
// `#[serde(default)]` keeps a partial snapshot from killing the whole read.
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize, Default)]
struct Snapshot {
    // Staleness is graded on the file mtime (atomic-rename write); the stamp
    // stays in the JSON for humans + the roll-up reader.
    #[serde(default)]
    #[allow(dead_code)]
    generated_unix_ms: i64,
    #[serde(default)]
    node: String,
    #[serde(default)]
    node_class: String,
    #[serde(default)]
    mesh: Option<SnapMesh>,
    #[serde(default)]
    snapshot_gap_secs: u32,
    #[serde(default)]
    conns: Vec<SnapConn>,
    #[serde(default)]
    undeclared_count: u32,
}

#[derive(serde::Deserialize, Default)]
struct SnapMesh {
    #[serde(default)]
    peers: Vec<SnapPeer>,
}

#[derive(serde::Deserialize, Default)]
struct SnapPeer {
    #[serde(default)]
    peer_fqdn: String,
    #[serde(default)]
    peer_ip: String,
    #[serde(default)]
    connected: bool,
    #[serde(default)]
    conn_type: String,
    #[serde(default)]
    handshake_age_ms: i64,
    #[serde(default)]
    active: bool,
    #[serde(default)]
    rx_bytes_delta: u64,
    #[serde(default)]
    tx_bytes_delta: u64,
    #[serde(default)]
    latency_ms: f64,
}

#[derive(serde::Deserialize, Default)]
struct SnapConn {
    #[serde(default)]
    bucket: String,
    #[serde(default)]
    proto: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    local_addr: String,
    #[serde(default)]
    remote_addr: String,
    #[serde(default)]
    pid: u32,
    #[serde(default)]
    comm: String,
    #[serde(default)]
    container: String,
    #[serde(default)]
    baseline_rule: String,
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

/// Build the per-node EnvHealthV1{domain:"net"} from a parsed snapshot.
fn roll_up(snap: &Snapshot, node: &str, window_secs: u32) -> EnvHealthV1 {
    let mesh_peers: Vec<NodeMeshPeerV1> = snap
        .mesh
        .as_ref()
        .map(|m| {
            m.peers
                .iter()
                .map(|p| NodeMeshPeerV1 {
                    peer_fqdn: p.peer_fqdn.clone(),
                    peer_ip: p.peer_ip.clone(),
                    active: p.active,
                    handshake_age_ms: p.handshake_age_ms,
                    conn_type: p.conn_type.clone(),
                    rx_bytes_delta: p.rx_bytes_delta,
                    tx_bytes_delta: p.tx_bytes_delta,
                    latency_ms: p.latency_ms,
                })
                .collect()
        })
        .unwrap_or_default();
    // stale = connected but not exchanging (handshake past the 3x rekey
    // window); a DISCONNECTED peer is neither active nor stale — absence is
    // not illness (ADR 1036 §3.4), per-peer liveness is panoscope's job.
    let mesh_active = mesh_peers.iter().filter(|p| p.active).count() as u32;
    let mesh_stale = snap
        .mesh
        .as_ref()
        .map(|m| m.peers.iter().filter(|p| p.connected && !p.active).count() as u32)
        .unwrap_or(0);
    let mesh_connected = mesh_active + mesh_stale;

    let conns: Vec<NodeConnV1> = snap
        .conns
        .iter()
        .map(|c| NodeConnV1 {
            bucket: c.bucket.clone(),
            proto: c.proto.clone(),
            state: c.state.clone(),
            local_addr: c.local_addr.clone(),
            remote_addr: c.remote_addr.clone(),
            pid: c.pid,
            comm: c.comm.clone(),
            container: c.container.clone(),
            baseline_rule: c.baseline_rule.clone(),
            first_seen_unix_ms: c.first_seen_unix_ms,
            last_seen_unix_ms: c.last_seen_unix_ms,
        })
        .collect();

    // Grading: worst grounded wins, never a false green. Amber reasons:
    // undeclared connections present, or every connected peer gone stale.
    let overall = if snap.undeclared_count > 0 {
        "amber"
    } else if mesh_connected > 0 && mesh_active == 0 {
        "amber"
    } else if conns.is_empty() && mesh_peers.is_empty() {
        "unmetered"
    } else {
        "green"
    };

    EnvHealthV1 {
        domain: "net".to_string(),
        scope: node.to_string(),
        ts_unix_ms: now_unix_ms(),
        overall_color: overall.to_string(),
        sample_count: conns.len() as u64 + mesh_connected as u64,
        window_secs,
        detail: Some(EnvDetail::Net(NetDetailV1 {
            mesh_peers,
            mesh_active,
            mesh_stale,
            fabric_sessions: Vec::new(), // exposed only where a router admin space exists
            conns,
            undeclared_count: snap.undeclared_count,
            node_class: snap.node_class.clone(),
            snapshot_gap_secs: snap.snapshot_gap_secs,
        })),
    }
}

/// The honest "nothing to read" envelope — stale or missing snapshot.
fn unmetered(node: &str, window_secs: u32) -> EnvHealthV1 {
    EnvHealthV1 {
        domain: "net".to_string(),
        scope: node.to_string(),
        ts_unix_ms: now_unix_ms(),
        overall_color: "unmetered".to_string(),
        sample_count: 0,
        window_secs,
        detail: Some(EnvDetail::Net(NetDetailV1::default())),
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "goat-node-net-health",
    about = "Net domain of the environment-health surface — per-node connection inventory roll-up (node-net-health.md/ADR 1036)"
)]
struct Args {
    /// Zenoh router endpoint, e.g. `tls/zagros-sandbox-zenoh-router:7447`.
    #[arg(long, env = "NNH_ENDPOINT")]
    endpoint: String,

    /// publish (default): grade this node's snapshot -> ops/net/health/<node>.
    /// rollup: subscribe ops/net/health/** and write the fleet JSON the
    /// node-list page reads (the passive roll-up reader — brokerless-safe,
    /// no central poller of nodes; nodes still self-publish).
    #[arg(long, env = "NNH_MODE", default_value = "publish")]
    mode: String,

    /// Node identifier — the `<node>` segment of ops/net/health/<node>
    /// (publish mode).
    #[arg(long, env = "NNH_NODE", default_value = "")]
    node: String,

    /// rollup mode: output JSON path (written atomically; served next to the
    /// relay-health dashboard).
    #[arg(long, env = "NNH_ROLLUP_OUT", default_value = "/www/node-net-health.json")]
    rollup_out: String,

    /// Path of the classified snapshot (bind-mounted read-only; written
    /// atomically by the host-side node-net-health-snapshot systemd timer).
    #[arg(long, env = "NNH_SNAPSHOT_FILE", default_value = "/state/snapshot.json")]
    snapshot_file: String,

    /// Leaf X.509 cert (PEM) for mTLS. Required for a `tls/` endpoint.
    #[arg(long, env = "NNH_CERT")]
    cert: Option<String>,
    /// Matching private key (PEM).
    #[arg(long, env = "NNH_KEY_FILE")]
    key: Option<String>,
    /// CA root cert (PEM) used to verify the router.
    #[arg(long, env = "NNH_ROOT_CA")]
    root_ca: Option<String>,

    /// Publish interval (ms). Re-publishes the latest snapshot each beat —
    /// same cadence as the sibling env-health meters so a one-shot
    /// `goat health --domain net` catches a beat; the snapshot cadence
    /// itself is the host timer's (hourly).
    #[arg(long, env = "NNH_INTERVAL_MS", default_value_t = 30_000)]
    interval_ms: u64,

    /// Staleness window (ms): a snapshot older than this publishes `unmetered`
    /// (never a false green). Default = ~2 hourly snapshot intervals + slack.
    #[arg(long, env = "NNH_STALE_AFTER_MS", default_value_t = 7_800_000)]
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

/// rollup mode — the passive fleet reader: subscribe ops/net/health/**,
/// keep each node's latest envelope, write the JSON the node-list page
/// reads (atomic rename). Reveal-only: nodes self-publish; this only
/// listens. The page grades staleness itself from received_at/ts.
async fn rollup(session: &zenoh::Session, out: &str) -> Result<()> {
    #[derive(serde::Serialize)]
    struct NodeEntry {
        received_at_unix_ms: i64,
        env: EnvHealthV1,
    }
    let subscriber = session
        .declare_subscriber("ops/net/health/**")
        .await
        .map_err(|e| anyhow::anyhow!("declare subscriber: {e}"))?;
    info!(out = out, "rollup mode: listening on ops/net/health/**");
    let mut nodes: std::collections::BTreeMap<String, NodeEntry> = Default::default();
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("install SIGTERM handler")?;
    loop {
        tokio::select! {
            s = subscriber.recv_async() => {
                let Ok(sample) = s else { break };
                let payload = sample.payload().to_bytes();
                match EnvHealthV1::decode(payload.as_ref()) {
                    Ok(env) if env.domain == "net" => {
                        let scope = env.scope.clone();
                        nodes.insert(scope.clone(), NodeEntry { received_at_unix_ms: now_unix_ms(), env });
                        let doc = serde_json::json!({
                            "generated_unix_ms": now_unix_ms(),
                            "nodes": nodes,
                        });
                        let tmp = format!("{out}.tmp");
                        if let Err(e) = std::fs::write(&tmp, serde_json::to_vec(&doc)?)
                            .and_then(|_| std::fs::rename(&tmp, out))
                        {
                            warn!(error = %e, "rollup write failed");
                        } else {
                            info!(node = scope.as_str(), nodes = nodes.len(), "rollup updated");
                        }
                    }
                    Ok(_) => {}
                    Err(e) => warn!(error = %e, "undecodable envelope on ops/net/health/**"),
                }
            }
            _ = sigterm.recv() => { info!("SIGTERM received; shutting down"); break; }
            _ = tokio::signal::ctrl_c() => { info!("SIGINT received; shutting down"); break; }
        }
    }
    Ok(())
}

/// Read + parse the snapshot, honestly: stale mtime, missing file, or a parse
/// failure all return None (the caller publishes `unmetered`).
fn read_snapshot(path: &Path, stale_after: Duration) -> Option<Snapshot> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            warn!(error = %e, path = %path.display(), "snapshot missing");
            return None;
        }
    };
    if let Ok(mtime) = meta.modified() {
        let age = SystemTime::now()
            .duration_since(mtime)
            .unwrap_or(Duration::ZERO);
        if age > stale_after {
            warn!(age_secs = age.as_secs(), path = %path.display(), "snapshot stale");
            return None;
        }
    }
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice::<Snapshot>(&bytes) {
        Ok(s) => Some(s),
        Err(e) => {
            warn!(error = %e, "snapshot undecodable");
            None
        }
    }
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
    anyhow::ensure!(!args.node.is_empty(), "--node required in publish mode");
    let health_key = format!("ops/net/health/{}", args.node);
    info!(
        zid = %session.zid(),
        endpoint = args.endpoint.as_str(),
        node = args.node.as_str(),
        snapshot_file = args.snapshot_file.as_str(),
        publishes = health_key.as_str(),
        "session open; grading connection snapshot -> ops/net/health/<node>"
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
                    Some(snap) => roll_up(&snap, &args.node, window_secs),
                    None => unmetered(&args.node, window_secs),
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
                    Ok(()) => info!(key = health_key.as_str(), n = n, overall = overall.as_str(), "published net health"),
                    Err(e) => warn!(error = %e, "publish net health failed"),
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
    fn failed_collection_keeps_evidence_until_reader_marks_it_unmetered() {
        if !Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node-net-health/test_inventory.py").is_file() {
            return; // the inventory script ships in goatcore, not beside this crate
        }
        let root = std::env::temp_dir().join(format!(
            "goat-net-inventory-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node-net-health/test_inventory.py");
        let result = std::process::Command::new("python3")
            .arg(fixture)
            .arg("--publisher-fixture")
            .arg(&root)
            .output()
            .expect("run production collector fixture");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        for scope in ["host", "container"] {
            let path = root.join(format!("retained-{scope}.json"));
            let snap = read_snapshot(&path, Duration::from_secs(60)).expect("last good snapshot");
            assert_eq!(roll_up(&snap, "fixture", 60).overall_color, "amber");
            assert_eq!(snap.undeclared_count, 1);
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(UNIX_EPOCH + Duration::from_secs(1))
                .unwrap();
            let reading = match read_snapshot(&path, Duration::from_secs(60)) {
                Some(snap) => roll_up(&snap, "fixture", 60),
                None => unmetered("fixture", 60),
            };
            assert_eq!(reading.overall_color, "unmetered");
            assert_eq!(reading.sample_count, 0);
        }
        let empty = read_snapshot(&root.join("successful-empty.json"), Duration::from_secs(60))
            .expect("successful empty scan");
        assert_eq!(roll_up(&empty, "fixture", 60).overall_color, "green");
        std::fs::remove_dir_all(root).unwrap();
    }

    // Shape-faithful to a live nnh-snapshot.py run on the ZAGROS Thor
    // (names/addresses genericized).
    const FIXTURE: &str = r#"{
        "generated_unix_ms": 1784020000000,
        "node": "mgmt-example-01",
        "node_class": "mgmt-host",
        "mesh": {"fqdn":"mgmt-example-01.netbird.selfhosted","peers":[
            {"peer_fqdn":"peer-active.netbird.selfhosted","peer_ip":"100.127.0.2",
             "connected":true,"conn_type":"P2P","handshake_age_ms":45000,
             "active":true,"rx_bytes_delta":1200,"tx_bytes_delta":900,"latency_ms":42.5},
            {"peer_fqdn":"peer-stale.netbird.selfhosted","peer_ip":"100.127.0.3",
             "connected":true,"conn_type":"Relayed","handshake_age_ms":999999,
             "active":false,"rx_bytes_delta":0,"tx_bytes_delta":0,"latency_ms":0},
            {"peer_fqdn":"peer-off.netbird.selfhosted","peer_ip":"100.127.0.4",
             "connected":false,"conn_type":"","handshake_age_ms":-1,
             "active":false,"rx_bytes_delta":0,"tx_bytes_delta":0,"latency_ms":0}
        ]},
        "snapshot_gap_secs": 3600,
        "conns": [
            {"bucket":"mesh","proto":"udp","state":"estab","local_addr":"192.0.2.10:51820",
             "remote_addr":"198.51.100.7:51820","pid":900,"comm":"netbird","container":"",
             "baseline_rule":"","first_seen_unix_ms":0,"last_seen_unix_ms":0},
            {"bucket":"baseline","proto":"tcp","state":"listen","local_addr":"0.0.0.0:22",
             "remote_addr":"","pid":1000,"comm":"sshd","container":"",
             "baseline_rule":"sshd-listen","first_seen_unix_ms":0,"last_seen_unix_ms":0},
            {"bucket":"undeclared","proto":"tcp","state":"estab","local_addr":"192.0.2.10:42550",
             "remote_addr":"203.0.113.9:443","pid":4242,"comm":"curl","container":"",
             "baseline_rule":"","first_seen_unix_ms":1784019990000,"last_seen_unix_ms":1784020000000}
        ],
        "undeclared_count": 1
    }"#;

    #[test]
    fn parses_live_shaped_snapshot() {
        let s: Snapshot = serde_json::from_str(FIXTURE).expect("parse");
        assert_eq!(s.node, "mgmt-example-01");
        assert_eq!(s.node_class, "mgmt-host");
        assert_eq!(s.mesh.as_ref().unwrap().peers.len(), 3);
        assert_eq!(s.conns.len(), 3);
        assert_eq!(s.undeclared_count, 1);
        assert_eq!(s.snapshot_gap_secs, 3600);
    }

    #[test]
    fn undeclared_grades_amber_never_silent_green() {
        let s: Snapshot = serde_json::from_str(FIXTURE).expect("parse");
        let e = roll_up(&s, "mgmt-example-01", 900);
        assert_eq!(e.domain, "net");
        assert_eq!(e.scope, "mgmt-example-01");
        assert_eq!(e.overall_color, "amber"); // 1 undeclared -> amber
        match e.detail {
            Some(EnvDetail::Net(ref d)) => {
                assert_eq!(d.undeclared_count, 1);
                assert_eq!(d.mesh_active, 1);
                assert_eq!(d.mesh_stale, 1); // connected-but-stale only; disconnected excluded
                assert_eq!(d.node_class, "mgmt-host");
                let und = d.conns.iter().find(|c| c.bucket == "undeclared").unwrap();
                assert_eq!(und.comm, "curl");
                assert!(und.first_seen_unix_ms > 0);
            }
            _ => panic!("expected Net detail"),
        }
        // Round-trips through protobuf.
        let bytes = e.encode_to_vec();
        let back = EnvHealthV1::decode(bytes.as_slice()).expect("decode");
        assert_eq!(back.overall_color, "amber");
        assert_eq!(back.domain, "net");
    }

    #[test]
    fn clean_snapshot_grades_green() {
        let mut s: Snapshot = serde_json::from_str(FIXTURE).expect("parse");
        s.undeclared_count = 0;
        s.conns.retain(|c| c.bucket != "undeclared");
        let e = roll_up(&s, "n", 900);
        assert_eq!(e.overall_color, "green");
    }

    #[test]
    fn all_connected_stale_grades_amber() {
        let mut s: Snapshot = serde_json::from_str(FIXTURE).expect("parse");
        s.undeclared_count = 0;
        s.conns.retain(|c| c.bucket != "undeclared");
        if let Some(m) = s.mesh.as_mut() {
            for p in m.peers.iter_mut() {
                if p.connected {
                    p.active = false;
                }
            }
        }
        let e = roll_up(&s, "n", 900);
        assert_eq!(e.overall_color, "amber"); // enrolled-but-not-exchanging fleet
    }

    #[test]
    fn empty_snapshot_reads_unmetered() {
        let s: Snapshot = serde_json::from_str(
            r#"{"generated_unix_ms":1,"node":"n","node_class":"x","mesh":null,
                "snapshot_gap_secs":0,"conns":[],"undeclared_count":0}"#,
        )
        .unwrap();
        let e = roll_up(&s, "n", 900);
        assert_eq!(e.overall_color, "unmetered");
        let e2 = unmetered("n", 900);
        assert_eq!(e2.overall_color, "unmetered");
        assert_eq!(e2.sample_count, 0);
    }
}
