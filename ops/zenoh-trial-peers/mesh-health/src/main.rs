// SPDX-License-Identifier: Apache-2.0
//
// goat-mesh-health — the mesh domain of the environment-health surface.
//
// ADR 1042 (mesh-link health domain) + ADR 1036 (env-health contract). A normal
// mTLS Zenoh client peer (templated on goat-time-health):
//   - Reads the host-side `netbird status --json` snapshot at a fixed path
//     (written atomically by the mesh-health-snapshot systemd timer the
//     mesh-health Ansible role installs; bind-mounted read-only). PASSIVE —
//     netbird already measures per-peer latency and direct-vs-relayed for its
//     own connection management; this meter injects no probes (no ICMP).
//   - Grades each WireGuard mesh link with the 5-state honesty colors and, on
//     a timer, publishes a per-site reading back on the bus in C0:
//       ops/mesh/health/<site>   <- EnvHealthV1{domain:"mesh"}
//   - Subscribes nothing. Persists nothing.
//
// Honesty — HEALTH graded against a PATH-AWARE expectation (docs/design/
// environment-health-surface.md §3.4 + §6, ADR 1042):
//   - connected               → green, escalated to amber/red ONLY by a MEASURED
//     latency past the threshold (netbird reports latency=0 when it has no
//     reading — an unmeasured latency never grades a link hot). Path (relayed vs
//     direct) does NOT color the link directly — it picks the YARDSTICK: a
//     relayed link rides a relay hop (often WAN/satellite) and is expected to be
//     slower, so its latency is judged against a looser relayed baseline; a
//     direct link is held to a tighter one. So "slow because relayed over a
//     distant bearer" reads calm, but the same latency on a link that should be
//     fast reads hot. `direct` also stays on the wire as a neutral badge (goat
//     top's PATH column). This fixed both the false "MESH amber" from the mere
//     fact of relaying AND the false "MESH red" from normal relayed latency
//     tripping a LAN-tight threshold.
//   - disconnected            → graded against the peer's DECLARED CLASS
//     (peer-liveness-expectations.md). ABSENCE IS NOT ILLNESS (§3.4 tenet 2)
//     still holds for any peer that may legitimately be off: `attended`,
//     `ephemeral` and `unclassified` peers stay idle however long they are
//     gone. But an `always-up` peer's absence is amber from the FIRST tick,
//     escalating to red with duration — never green while a peer we declared
//     must be up is missing. A long-absent sensor node reading calm is the
//     failure this closes, and a grace period would re-create it: on a
//     meter restart absence duration reads 0, so "amber only after 10 min"
//     publishes green with a node gone. The don't-page-on-a-blip tier lives in the ALERT rule.
//   - Connecting              → NOT a color of its own. From this vantage
//     netbird reports an unreachable peer as `Connecting` whether it is wedged
//     in the F-286 half-state or simply switched off (a powered-down laptop reads `Connecting`). It is
//     REPORTED for always-up peers, where the remedy differs (repair name
//     resolution, do not restart), and never colored for anyone else.
//   - snapshot stale or missing → the whole reading publishes `unmetered` with
//     zero links. Absence of signal is never a green.
//   - overall_color = worst grounded link (red>amber>green); idle when links
//     exist but none is grounded; unmetered when there is nothing to read.
//
// Vantage (ADR 1042 v1 limit): this is the HUB vantage — the links the vantage
// host's own netbird sees. Partner<->partner pairwise links are out of scope
// (that is snitch territory; full-matrix needs a publisher on every peer).

use anyhow::{Context, Result};
use clap::Parser;
use prost::Message as _;
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};
use zenoh::qos::{CongestionControl, Priority};

// ---------------------------------------------------------------------------
// Wire types — keep field tags in lock-step with the canonical schema at
// ops/zenoh-trial-peers/time-health/proto/env_health_v1.proto.
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, prost::Message)]
struct MeshLinkHealthV1 {
    #[prost(string, tag = "1")]
    peer_fqdn: String,
    #[prost(string, tag = "2")]
    peer_ip: String,
    #[prost(string, tag = "3")]
    color: String,
    #[prost(bool, tag = "4")]
    connected: bool,
    #[prost(bool, tag = "5")]
    direct: bool,
    #[prost(string, tag = "6")]
    conn_type: String,
    #[prost(double, tag = "7")]
    latency_ms: f64,
    #[prost(string, tag = "8")]
    relay_address: String,
    #[prost(int64, tag = "9")]
    handshake_age_ms: i64,
    #[prost(uint64, tag = "10")]
    transition_count: u64,
    /// Declared expectation class — never inferred from behaviour.
    #[prost(string, tag = "11")]
    peer_class: String,
    /// How long this peer has been absent (ms); -1 while connected.
    #[prost(int64, tag = "12")]
    absent_ms: i64,
    /// Raw netbird status ("Connected"|"Disconnected"|"Idle"|"Connecting").
    #[prost(string, tag = "13")]
    status: String,
    /// ICE candidate types of the selected pair ("host"|"srflx"|"relay";
    /// empty = netbird reported none). host/host is the LAN path.
    #[prost(string, tag = "14")]
    ice_local: String,
    #[prost(string, tag = "15")]
    ice_remote: String,
    /// This peer is declared to share the vantage's LAN (issue #240).
    #[prost(bool, tag = "16")]
    lan_declared: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
struct MeshDetailV1 {
    #[prost(message, repeated, tag = "1")]
    links: Vec<MeshLinkHealthV1>,
    #[prost(string, tag = "2")]
    vantage_host: String,
    #[prost(uint32, tag = "3")]
    peers_total: u32,
    #[prost(uint32, tag = "4")]
    peers_connected: u32,
    /// THE product: `always-up` peers currently absent. Everything else here
    /// is context (peer-liveness-expectations.md §Shape).
    #[prost(string, repeated, tag = "5")]
    always_up_absent: Vec<String>,
    /// Peers stuck in the F-286 half-state: enrolled, powered, `Connecting`
    /// indefinitely, and unable to self-clear. Distinct from absent because the
    /// remedy differs — repair name resolution, do not restart.
    #[prost(string, repeated, tag = "6")]
    connecting_stuck: Vec<String>,
    #[prost(uint32, tag = "7")]
    always_up_total: u32,
    #[prost(uint32, tag = "8")]
    attended_total: u32,
    #[prost(uint32, tag = "9")]
    ephemeral_total: u32,
    /// Peers with no declared class. Counted and surfaced so the declaration
    /// gap is visible; never alerted on (we do not invent an expectation).
    #[prost(uint32, tag = "10")]
    unclassified_total: u32,
    /// Peers declared to share this vantage's LAN (issue #240 observability).
    #[prost(uint32, tag = "11")]
    lan_declared_total: u32,
    /// Declared LAN peers currently connected on a selected pair that is NOT
    /// host/host — traffic to a LAN neighbour is transiting the WAN/relay.
    /// Positive evidence only: a disconnected peer, or one whose snapshot
    /// carries no ICE data, is never counted (a fabricated hairpin is as bad
    /// as a fabricated green). Path is posture, not health (§3.4): this names
    /// and counts, it does not colour.
    #[prost(string, repeated, tag = "12")]
    lan_hairpinning: Vec<String>,
    /// Declared LAN peers the snapshot does not contain at all: a typo in the
    /// declaration, a peer deleted from management, or one outside the
    /// vantage's ACL scope. Each still counts in `lan_declared_total`, so
    /// without this list the declared count and the hairpin count disagree
    /// with nothing naming why. Posture, like `lan_hairpinning`: named, not
    /// coloured.
    #[prost(string, repeated, tag = "13")]
    lan_unseen: Vec<String>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct FabricDetailV1 {}

#[derive(Clone, PartialEq, prost::Message)]
struct TimeDetailV1 {}

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
    #[prost(oneof = "EnvDetail", tags = "10, 11, 12")]
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
}

// ---------------------------------------------------------------------------
// Incoming snapshot — the subset of `netbird status --json` the meter reads
// (field names verified against netbird 0.72.2 live output + the snitch
// parser at ops/snitch/internal/types/types.go). Extra fields are ignored;
// `#[serde(default)]` keeps a partial snapshot from killing the whole read.
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct NetbirdStatus {
    #[serde(default)]
    fqdn: String,
    #[serde(default)]
    peers: NetbirdPeers,
}

#[derive(serde::Deserialize, Default)]
struct NetbirdPeers {
    #[serde(default)]
    total: u32,
    #[serde(default)]
    connected: u32,
    #[serde(default)]
    details: Vec<NetbirdPeerDetail>,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct NetbirdPeerDetail {
    #[serde(default)]
    fqdn: String,
    #[serde(default)]
    netbird_ip: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    connection_type: String, // "P2P" | "Relayed"
    #[serde(default)]
    relay_address: String,
    #[serde(default)]
    last_wireguard_handshake: String,
    #[serde(default)]
    latency: i64, // nanoseconds; 0 = netbird has no measurement
    /// Which ICE candidate types the selected pair is built from
    /// ("host"|"srflx"|"relay"; empty when netbird reports none, e.g. while
    /// disconnected or on snapshots from older daemons). `host`/`host` on a
    /// same-LAN pair means the LAN path; anything else on a declared LAN peer
    /// is the #240 hairpin.
    #[serde(default)]
    ice_candidate_type: NetbirdIceCandidate,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct NetbirdIceCandidate {
    #[serde(default)]
    local: String,
    #[serde(default)]
    remote: String,
}

// ---------------------------------------------------------------------------
// Declared peer expectations (peer-liveness-expectations.md).
//
// The classification is DECLARATIVE and reviewed, never inferred. Inferring
// "this peer is usually up so it should be up" learns the outage as normal — a
// node that is down a long time becomes its own baseline and stays invisible.
// Operators author the map as YAML in group_vars; Ansible
// renders it to JSON alongside the netbird snapshot and bind-mounts it read-only.
// ---------------------------------------------------------------------------

const CLASS_ALWAYS_UP: &str = "always-up";
const CLASS_ATTENDED: &str = "attended";
const CLASS_EPHEMERAL: &str = "ephemeral";
/// No declaration exists for this peer. Counted and surfaced so the gap is
/// visible; never alerted on — absence only means something against a declared
/// expectation, and we do not invent one.
const CLASS_UNCLASSIFIED: &str = "unclassified";

#[derive(serde::Deserialize, Default)]
struct PeerClasses {
    /// Exact FQDN -> class.
    #[serde(default)]
    peers: HashMap<String, String>,
    /// Suffix patterns ("*.sensors.example.net" -> class), longest match wins.
    /// Exact entries always beat patterns.
    #[serde(default)]
    patterns: HashMap<String, String>,
    /// Exact FQDNs declared to share this vantage's LAN (issue #240). Declared,
    /// never inferred: F-206 proved a shared RFC1918 subnet is not evidence of
    /// a shared LAN, so locality is an operator statement like everything else
    /// in this file. A connected LAN peer whose selected ICE pair is not
    /// host/host is counted and named as hairpinning. The vantage's own FQDN
    /// may appear (group_vars render one list per host group); it never shows
    /// up in its own peer list, so the entry is inert. Empty list = feature
    /// inert.
    #[serde(default)]
    lan_peers: Vec<String>,
}

impl PeerClasses {
    fn load(path: &Path) -> Option<Self> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, path = %path.display(), "peer-class map unreadable");
                return None;
            }
        };
        match serde_json::from_slice::<PeerClasses>(&bytes) {
            Ok(c) => Some(c),
            Err(e) => {
                warn!(error = %e, "peer-class map undecodable");
                None
            }
        }
    }

    /// Resolve a peer's declared class. Unknown peers are `unclassified` —
    /// fail-safe: an undeclared peer never pages anyone.
    fn classify(&self, fqdn: &str) -> &str {
        if let Some(c) = self.peers.get(fqdn) {
            return c;
        }
        let mut best: Option<(usize, &str)> = None;
        for (pat, class) in &self.patterns {
            let suffix = pat.strip_prefix('*').unwrap_or(pat);
            if fqdn.ends_with(suffix) {
                let better = best.map(|(len, _)| suffix.len() > len).unwrap_or(true);
                if better {
                    best = Some((suffix.len(), class));
                }
            }
        }
        best.map(|(_, c)| c).unwrap_or(CLASS_UNCLASSIFIED)
    }
}

// ---------------------------------------------------------------------------
// Management peer registry (optional).
//
// The vantage's local `netbird status` answers "can I reach this peer". The
// management registry answers "does this peer exist, and when did the control
// plane last hear from it" — a different question, from a vantage that survives
// the peer's absence in a way the local daemon's opinion does not.
//
// Two things only the registry can settle:
//   - a TRUE `last_seen`, so absence duration is not merely "since this meter
//     started, floored by a handshake stamp";
//   - whether a declared peer that is missing from the roster was DELETED (the
//     declaration is stale) or merely fell out of the vantage's ACL scope
//     (quietly unmonitored). Those look identical locally, which is why
//     roster-absence grades amber rather than red without it.
//
// Fetched host-side by the snapshot timer and dropped next to the netbird
// snapshot, so the container stays passive, file-driven, and tokenless.
// Absent or stale file => everything below simply does not apply, and the
// meter behaves exactly as it did before.
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize, Default)]
struct RegistryPeer {
    #[serde(default)]
    dns_label: String,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    connected: bool,
    /// RFC3339. netbird omits/zeroes this for a peer that has never connected.
    #[serde(default)]
    last_seen: String,
}

#[derive(Default)]
struct PeerRegistry {
    /// dns_label -> peer. The local snapshot's FQDN is `<dns_label>.<zone>`,
    /// so the label is the stable join key across both vantages.
    by_label: HashMap<String, RegistryPeer>,
    fetched: bool,
}

impl PeerRegistry {
    fn load(path: &Path, stale_after: Duration) -> Self {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => return Self::default(), // not configured / not yet written
        };
        if let Ok(mtime) = meta.modified() {
            if SystemTime::now().duration_since(mtime).unwrap_or(Duration::ZERO) > stale_after {
                warn!(path = %path.display(), "peer registry stale — ignoring rather than trusting");
                return Self::default();
            }
        }
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "peer registry unreadable");
                return Self::default();
            }
        };
        match serde_json::from_slice::<Vec<RegistryPeer>>(&bytes) {
            Ok(peers) => {
                let mut by_label = HashMap::new();
                for p in peers {
                    let key = if !p.dns_label.is_empty() {
                        p.dns_label.clone()
                    } else {
                        p.hostname.clone()
                    };
                    if !key.is_empty() {
                        by_label.insert(key, p);
                    }
                }
                Self { by_label, fetched: true }
            }
            Err(e) => {
                warn!(error = %e, "peer registry undecodable");
                Self::default()
            }
        }
    }

    /// The label half of an FQDN: `node-a.netbird.selfhosted` -> `node-a`.
    fn label_of(fqdn: &str) -> &str {
        fqdn.split('.').next().unwrap_or(fqdn)
    }

    fn get(&self, fqdn: &str) -> Option<&RegistryPeer> {
        self.by_label.get(Self::label_of(fqdn))
    }

    /// Is this declared peer known to the control plane at all?
    /// `None` = no registry, so the question cannot be answered — which is NOT
    /// the same as "no", and must not be rendered as one.
    fn knows(&self, fqdn: &str) -> Option<bool> {
        if !self.fetched {
            return None;
        }
        Some(self.by_label.contains_key(Self::label_of(fqdn)))
    }
}

/// Age of an RFC3339 `last_seen` in ms; -1 when absent, zero-valued or unparseable.
fn last_seen_age_ms(stamp: &str, now: SystemTime) -> i64 {
    if stamp.is_empty() || stamp.starts_with("0001-01-01") {
        return -1; // netbird's zero value for "never connected"
    }
    match humantime::parse_rfc3339(stamp) {
        Ok(t) => now.duration_since(t).map(|d| d.as_millis() as i64).unwrap_or(-1),
        Err(_) => -1,
    }
}

/// Tracks how long each peer has been absent, across ticks.
///
/// A meter restart must not reset a long-running absence back to zero — that
/// would silence exactly the outage this exists to catch — so the in-process
/// clock is floored by the peer's WireGuard handshake age, which survives us.
#[derive(Default)]
struct Liveness {
    absent_since: HashMap<String, SystemTime>,
}

impl Liveness {
    /// Returns absence duration in ms, or -1 while connected.
    fn observe(&mut self, fqdn: &str, connected: bool, handshake_age_ms: i64, now: SystemTime) -> i64 {
        if connected {
            self.absent_since.remove(fqdn);
            return -1;
        }
        let since = *self
            .absent_since
            .entry(fqdn.to_string())
            .or_insert(now);
        let tracked = now
            .duration_since(since)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        // Handshake age is corroborating evidence the peer was gone before we
        // started watching. Take the longer of the two — never the shorter.
        tracked.max(handshake_age_ms)
    }
}

/// The 5-state honesty color for one link — HEALTH, graded on connectivity +
/// *measured* latency, with the threshold chosen by PATH (environment-health-
/// surface.md §3.4). Posture (direct vs relayed) does NOT color the link
/// directly — it sets the EXPECTATION the measurement is judged against: a
/// relayed link is expected to be slower (relay hop, often satellite/WAN), so
/// its latency is graded against a looser relayed baseline, while a direct link
/// is held to a tighter one. So "slow because relayed over a distant bearer"
/// reads green/amber (normal for that path), but the SAME latency on a link
/// that should be fast reads hot. The measurement drives the color; posture
/// only picks the yardstick. `direct` stays on the wire as a neutral badge
/// (goat top's PATH column). An unmeasured latency (0) never grades a link hot.
///
/// ABSENCE is graded against the peer's DECLARED CLASS, not against the bare
/// fact of being disconnected (peer-liveness-expectations.md). "Absence is not
/// illness" (§3.4 tenet 2) remains true for a peer that may legitimately be
/// off — `attended`, `ephemeral`, and `unclassified` peers stay `idle` however
/// long they are gone. But an `always-up` peer's absence IS a finding: that is
/// the whole point of declaring the expectation.
///
/// `Connecting` sustained past its threshold is its own amber — the F-286
/// half-state, where a node is enrolled, powered, and permanently unable to
/// rejoin. It is called out separately from absence because the remedy differs:
/// repair name resolution, do not restart.
struct Thresholds {
    direct_amber_ms: f64,
    direct_red_ms: f64,
    relayed_amber_ms: f64,
    relayed_red_ms: f64,
    absent_red_ms: i64,
    connecting_amber_ms: i64,
}

fn link_color(
    connected: bool,
    direct: bool,
    latency_ms: f64,
    class: &str,
    absent_ms: i64,
    status: &str,
    t: &Thresholds,
) -> &'static str {
    let _ = status; // posture only; see connecting_stuck in roll_up
    if !connected {
        // NOTE: `Connecting` is NOT graded on its own here, and deliberately so.
        // From the hub vantage, netbird reports a peer it cannot reach as
        // `Connecting` whether that peer is wedged in the F-286 half-state or
        // simply switched off — measured, where 7 of 14
        // peers sat `Connecting` and every one was an ordinary powered-down
        // laptop. Colouring on that would have paged for the normal resting
        // state of half the fleet. The vantage cannot see "powered and trying",
        // so absence is graded by DECLARED CLASS alone, and the
        // `Connecting`-vs-`Disconnected` distinction is reported (never
        // coloured) for always-up peers, where the remedy differs.
        if class == CLASS_ALWAYS_UP {
            if absent_ms >= t.absent_red_ms {
                return "red";
            }
            // Amber from the FIRST tick, not after a grace period.
            // The alternative is a false green:
            // a declared always-up peer was correctly NAMED in always_up_absent
            // while overall_color still read `green`, because the meter had just
            // restarted (in-process absence 0) and the peer had no WireGuard
            // handshake stamp to floor it — so a dashboard reading the colour
            // alone saw "healthy" with a node missing. Duration cannot be a
            // precondition for saying something is absent when we can see that
            // it IS absent; it only decides amber vs red. The 10-minute
            // don't-page-on-a-blip tier belongs in the ALERT rule, over this
            // colour plus absent_ms, not in the colour itself.
            return "amber";
        }
        return "idle"; // deliberately-off peers must not alarm
    }
    let (amber_ms, red_ms) = if direct {
        (t.direct_amber_ms, t.direct_red_ms)
    } else {
        (t.relayed_amber_ms, t.relayed_red_ms)
    };
    if latency_ms > 0.0 {
        if latency_ms >= red_ms {
            return "red";
        }
        if latency_ms >= amber_ms {
            return "amber";
        }
    }
    "green" // connected + latency in-bound for its path (or unmeasured)
}

/// Worst *grounded* color across links: red > amber > green; idle when links
/// exist but none is grounded; unmetered when there is nothing to read.
fn overall_color(link_colors: &[&str]) -> &'static str {
    if link_colors.is_empty() {
        return "unmetered";
    }
    if link_colors.contains(&"red") {
        "red"
    } else if link_colors.contains(&"amber") {
        "amber"
    } else if link_colors.contains(&"green") {
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

/// Age of an RFC3339 handshake stamp in ms; -1 when absent/unparseable.
fn handshake_age_ms(stamp: &str, now: SystemTime) -> i64 {
    if stamp.is_empty() {
        return -1;
    }
    match humantime::parse_rfc3339(stamp) {
        Ok(t) => now
            .duration_since(t)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(-1),
        Err(_) => -1,
    }
}

/// Strip a CIDR suffix from a netbird IP ("100.127.0.7/16" -> "100.127.0.7").
fn strip_cidr(ip: &str) -> &str {
    ip.split('/').next().unwrap_or(ip)
}

/// Per-peer direct/relayed transition tracking across ticks (flap signal).
#[derive(Default)]
struct Transitions {
    last: HashMap<String, String>,
    counts: HashMap<String, u64>,
}

impl Transitions {
    /// Record this tick's conn_type for a peer; returns the running flip count.
    fn observe(&mut self, fqdn: &str, conn_type: &str) -> u64 {
        if conn_type.is_empty() {
            // Disconnected — keep the last known path type so a reconnect on a
            // different path still counts as a transition.
            return self.counts.get(fqdn).copied().unwrap_or(0);
        }
        if let Some(prev) = self.last.get(fqdn) {
            if prev != conn_type {
                *self.counts.entry(fqdn.to_string()).or_insert(0) += 1;
            }
        }
        self.last.insert(fqdn.to_string(), conn_type.to_string());
        self.counts.get(fqdn).copied().unwrap_or(0)
    }
}

/// Build the per-site EnvHealthV1{domain:"mesh"} from a parsed snapshot.
#[allow(clippy::too_many_arguments)]
fn roll_up(
    status: &NetbirdStatus,
    transitions: &mut Transitions,
    liveness: &mut Liveness,
    classes: &PeerClasses,
    registry: &PeerRegistry,
    site: &str,
    window_secs: u32,
    t: &Thresholds,
    now: SystemTime,
) -> EnvHealthV1 {
    let mut links: Vec<MeshLinkHealthV1> = Vec::with_capacity(status.peers.details.len());
    let (mut always_up_total, mut attended_total) = (0u32, 0u32);
    let (mut ephemeral_total, mut unclassified_total) = (0u32, 0u32);
    let mut always_up_absent: Vec<String> = Vec::new();
    let mut connecting_stuck: Vec<String> = Vec::new();
    let lan_declared: std::collections::HashSet<&str> =
        classes.lan_peers.iter().map(String::as_str).collect();
    // The vantage's own FQDN never appears in its peer list, so a self-entry
    // (unavoidable when one list is rendered per host group) must not inflate
    // the declared total.
    let lan_declared_total =
        lan_declared.iter().filter(|f| **f != status.fqdn.as_str()).count() as u32;
    let mut lan_hairpinning: Vec<String> = Vec::new();

    for p in &status.peers.details {
        let connected = p.status.eq_ignore_ascii_case("connected");
        let direct = p.connection_type.eq_ignore_ascii_case("p2p");
        let latency_ms = p.latency as f64 / 1.0e6;
        let conn_type = if connected { p.connection_type.clone() } else { String::new() };
        let transition_count = transitions.observe(&p.fqdn, &conn_type);
        let hs_age = handshake_age_ms(&p.last_wireguard_handshake, now);
        let class = classes.classify(&p.fqdn).to_string();
        // The registry's last_seen is the control plane's own record and beats
        // both the handshake stamp and this meter's uptime — it is the only one
        // of the three that survives both a meter restart and a peer that never
        // completed a handshake. Taken as a FLOOR, never a ceiling: whichever
        // source says the peer has been gone longest is the one to believe.
        let reg_age = registry
            .get(&p.fqdn)
            .map(|r| last_seen_age_ms(&r.last_seen, now))
            .unwrap_or(-1);
        let absent_ms = liveness.observe(&p.fqdn, connected, hs_age.max(reg_age), now);

        match class.as_str() {
            CLASS_ALWAYS_UP => always_up_total += 1,
            CLASS_ATTENDED => attended_total += 1,
            CLASS_EPHEMERAL => ephemeral_total += 1,
            _ => unclassified_total += 1,
        }
        // The declared-absent list is the product; it is built from the
        // declaration, NOT from the color, so it stays correct even while a
        // freshly-absent peer is still inside its alert threshold.
        if !connected && class == CLASS_ALWAYS_UP {
            always_up_absent.push(p.fqdn.clone());
        }
        // Only meaningful for a peer we EXPECT to be up: for anything else,
        // `Connecting` is just how the vantage renders "switched off".
        if !connected
            && class == CLASS_ALWAYS_UP
            && p.status.eq_ignore_ascii_case("connecting")
            && absent_ms >= t.connecting_amber_ms
        {
            connecting_stuck.push(p.fqdn.clone());
        }

        // Hairpin detection needs POSITIVE evidence on all three legs: the
        // peer is declared same-LAN, it is connected, and netbird reported the
        // selected pair's candidate types with at least one leg not `host`.
        // Empty ICE data (disconnected, or a pre-ICE snapshot) counts nothing.
        // A prflx/host pair on the LAN still counts: the ICE candidate type
        // names how the address was learned, not the path the packets take,
        // and only host/host is the direct LAN path. Anything else on
        // a declared neighbour is read as hairpinning through the WAN.
        let is_lan = lan_declared.contains(p.fqdn.as_str());
        let ice_l = p.ice_candidate_type.local.as_str();
        let ice_r = p.ice_candidate_type.remote.as_str();
        if is_lan
            && connected
            && !ice_l.is_empty()
            && !ice_r.is_empty()
            && !(ice_l.eq_ignore_ascii_case("host") && ice_r.eq_ignore_ascii_case("host"))
        {
            lan_hairpinning.push(p.fqdn.clone());
        }

        links.push(MeshLinkHealthV1 {
            peer_fqdn: p.fqdn.clone(),
            peer_ip: strip_cidr(&p.netbird_ip).to_string(),
            color: link_color(connected, direct, latency_ms, &class, absent_ms, &p.status, t)
                .to_string(),
            connected,
            direct,
            conn_type,
            latency_ms,
            relay_address: p.relay_address.clone(),
            handshake_age_ms: hs_age,
            transition_count,
            peer_class: class,
            absent_ms,
            status: p.status.clone(),
            ice_local: p.ice_candidate_type.local.clone(),
            ice_remote: p.ice_candidate_type.remote.clone(),
            lan_declared: is_lan,
        });
    }
    // Declared but ABSENT FROM THE ROSTER ENTIRELY.
    //
    // Everything above grades the peers the snapshot contains. A peer that is
    // merely down still appears there (`Disconnected`/`Connecting`) and grades
    // correctly — but one deleted from management, or dropped out of the
    // vantage's ACL scope, leaves the list altogether and would never be
    // compared against its declaration. It would vanish from the roll-up in
    // silence, which is this surface's own founding failure one level up:
    // absence of the RECORD reading as absence of a problem.
    //
    // Only exact `peers` entries can be checked: a `patterns` entry has no
    // finite membership, so nothing can be asserted about members never seen.
    let seen: std::collections::HashSet<&str> =
        status.peers.details.iter().map(|p| p.fqdn.as_str()).collect();
    for (fqdn, class) in &classes.peers {
        if class != CLASS_ALWAYS_UP || seen.contains(fqdn.as_str()) {
            continue;
        }
        always_up_total += 1;
        always_up_absent.push(fqdn.clone());
        // With a registry we can finally separate the two cases that look
        // identical from the local vantage. DELETED is the harder finding: the
        // control plane has no record at all, so the peer is not merely off —
        // it cannot come back without being re-enrolled, and the declaration
        // naming it is now fiction. Descoped means it exists and we simply
        // cannot see it, which is a visibility problem, not an outage.
        let (status, color) = match registry.knows(fqdn) {
            Some(false) => ("not-in-registry", "red"),
            Some(true) => ("out-of-vantage-scope", "amber"),
            // No registry: the question is unanswerable, and amber says
            // "someone look" without claiming which of the two it is.
            None => ("not-in-roster", "amber"),
        };
        let reg_age = registry
            .get(fqdn)
            .map(|r| last_seen_age_ms(&r.last_seen, now))
            .unwrap_or(-1);
        links.push(MeshLinkHealthV1 {
            peer_fqdn: fqdn.clone(),
            peer_ip: String::new(),
            color: color.to_string(),
            connected: false,
            direct: false,
            conn_type: String::new(),
            latency_ms: 0.0,
            relay_address: String::new(),
            handshake_age_ms: -1,
            transition_count: 0,
            peer_class: class.clone(),
            // -1 stays "unknown"; the registry supplies a real duration when it
            // has one, and a fabricated zero is never acceptable here.
            absent_ms: reg_age,
            status: status.to_string(),
            ice_local: String::new(),
            ice_remote: String::new(),
            lan_declared: lan_declared.contains(fqdn.as_str()),
        });
    }

    // The same roster gap for the LAN declaration, which the loop above never
    // reaches: `lan_peers` is a separate list, so an entry that is not also an
    // always-up `peers` entry would count toward `lan_declared_total` and be
    // compared against nothing.
    let mut lan_unseen: Vec<String> = lan_declared
        .iter()
        .filter(|f| **f != status.fqdn.as_str() && !seen.contains(**f))
        .map(|f| f.to_string())
        .collect();

    links.sort_by(|a, b| a.peer_fqdn.cmp(&b.peer_fqdn)); // deterministic ordering
    always_up_absent.sort();
    connecting_stuck.sort();
    lan_hairpinning.sort();
    lan_unseen.sort();
    let colors: Vec<&str> = links.iter().map(|l| l.color.as_str()).collect();
    let grounded = links.iter().filter(|l| l.connected).count() as u64;
    EnvHealthV1 {
        domain: "mesh".to_string(),
        scope: site.to_string(),
        ts_unix_ms: now_unix_ms(),
        overall_color: overall_color(&colors).to_string(),
        sample_count: grounded,
        window_secs,
        detail: Some(EnvDetail::Mesh(MeshDetailV1 {
            links,
            vantage_host: status.fqdn.clone(),
            peers_total: status.peers.total,
            peers_connected: status.peers.connected,
            always_up_absent,
            connecting_stuck,
            always_up_total,
            attended_total,
            ephemeral_total,
            unclassified_total,
            lan_declared_total,
            lan_hairpinning,
            lan_unseen,
        })),
    }
}

// ---------------------------------------------------------------------------
// Prometheus textfile export.
//
// The meter already holds everything an alert needs, so it emits the textfile
// itself rather than standing up a second service with its own cert, ACL
// subject and container. node_exporter's textfile collector scrapes the
// directory; the write is tmp+rename so a scrape never sees a half-written file.
//
// Honesty rules carried onto the metrics, because an alert reads THESE and not
// the envelope: `unmetered` publishes goat_mesh_metered=0 and NO per-peer
// series, so a rule cannot mistake "we cannot see" for "nothing is absent".
// goat_mesh_last_reading_timestamp_seconds always advances, so staleness of the
// meter itself is detectable separately from the fleet being healthy.
// ---------------------------------------------------------------------------

fn render_textfile(reading: &EnvHealthV1, site: &str) -> String {
    let mut s = String::new();
    let m = match reading.detail {
        Some(EnvDetail::Mesh(ref m)) => m,
        _ => return s,
    };
    let metered = reading.overall_color != "unmetered";

    s.push_str("# HELP goat_mesh_metered Whether this reading is grounded in a usable snapshot.\n");
    s.push_str("# TYPE goat_mesh_metered gauge\n");
    s.push_str(&format!(
        "goat_mesh_metered{{site=\"{site}\"}} {}\n",
        if metered { 1 } else { 0 }
    ));

    s.push_str("# HELP goat_mesh_last_reading_timestamp_seconds When the meter last produced a reading.\n");
    s.push_str("# TYPE goat_mesh_last_reading_timestamp_seconds gauge\n");
    s.push_str(&format!(
        "goat_mesh_last_reading_timestamp_seconds{{site=\"{site}\"}} {}\n",
        reading.ts_unix_ms / 1000
    ));

    // Severity as a number so a rule can threshold it: green 0, idle 1,
    // amber 2, red 3, unmetered -1 (explicitly NOT 0 — unknown is not healthy).
    let sev = match reading.overall_color.as_str() {
        "green" => 0,
        "idle" => 1,
        "amber" => 2,
        "red" => 3,
        _ => -1,
    };
    s.push_str("# HELP goat_mesh_overall_severity 0 green, 1 idle, 2 amber, 3 red, -1 unmetered.\n");
    s.push_str("# TYPE goat_mesh_overall_severity gauge\n");
    s.push_str(&format!(
        "goat_mesh_overall_severity{{site=\"{site}\"}} {sev}\n"
    ));

    if !metered {
        // Deliberately no per-peer series and no absent count: emitting 0 here
        // is the F-287 counter-that-read-zero shape, and an alert would read it
        // as "all clear" during a total loss of visibility.
        return s;
    }

    s.push_str("# HELP goat_mesh_always_up_absent Count of declared always-up peers currently absent.\n");
    s.push_str("# TYPE goat_mesh_always_up_absent gauge\n");
    s.push_str(&format!(
        "goat_mesh_always_up_absent{{site=\"{site}\"}} {}\n",
        m.always_up_absent.len()
    ));

    s.push_str("# HELP goat_mesh_declared_peers Declared peers by class.\n");
    s.push_str("# TYPE goat_mesh_declared_peers gauge\n");
    for (class, n) in [
        (CLASS_ALWAYS_UP, m.always_up_total),
        (CLASS_ATTENDED, m.attended_total),
        (CLASS_EPHEMERAL, m.ephemeral_total),
        (CLASS_UNCLASSIFIED, m.unclassified_total),
    ] {
        s.push_str(&format!(
            "goat_mesh_declared_peers{{site=\"{site}\",class=\"{class}\"}} {n}\n"
        ));
    }

    s.push_str("# HELP goat_mesh_peer_connected Per-peer connectivity, labelled with its DECLARED class.\n");
    s.push_str("# TYPE goat_mesh_peer_connected gauge\n");
    for l in &m.links {
        s.push_str(&format!(
            "goat_mesh_peer_connected{{site=\"{site}\",peer=\"{}\",class=\"{}\"}} {}\n",
            l.peer_fqdn,
            l.peer_class,
            if l.connected { 1 } else { 0 }
        ));
    }

    s.push_str("# HELP goat_mesh_peer_absent_seconds How long a peer has been absent; absent while connected.\n");
    s.push_str("# TYPE goat_mesh_peer_absent_seconds gauge\n");
    for l in &m.links {
        if l.absent_ms >= 0 {
            s.push_str(&format!(
                "goat_mesh_peer_absent_seconds{{site=\"{site}\",peer=\"{}\",class=\"{}\"}} {}\n",
                l.peer_fqdn,
                l.peer_class,
                l.absent_ms / 1000
            ));
        }
    }

    s.push_str("# HELP goat_mesh_peer_connecting_stuck Declared always-up peer sustained in `Connecting`.\n");
    s.push_str("# TYPE goat_mesh_peer_connecting_stuck gauge\n");
    for p in &m.connecting_stuck {
        s.push_str(&format!(
            "goat_mesh_peer_connecting_stuck{{site=\"{site}\",peer=\"{p}\"}} 1\n"
        ));
    }

    // Emitted only when LAN peers are declared: on a site with no declaration
    // the series is absent, so a rule cannot read "not declared" as "not
    // hairpinning" (the F-287 counter-that-read-zero shape).
    if m.lan_declared_total > 0 {
        s.push_str("# HELP goat_mesh_lan_peers_declared Peers declared to share this vantage's LAN.\n");
        s.push_str("# TYPE goat_mesh_lan_peers_declared gauge\n");
        s.push_str(&format!(
            "goat_mesh_lan_peers_declared{{site=\"{site}\"}} {}\n",
            m.lan_declared_total
        ));
        s.push_str("# HELP goat_mesh_lan_peers_hairpinning Declared LAN peers connected on a non-host/host ICE pair (issue #240). Sustained > 0 is the ADR 0306 revisit trigger.\n");
        s.push_str("# TYPE goat_mesh_lan_peers_hairpinning gauge\n");
        s.push_str(&format!(
            "goat_mesh_lan_peers_hairpinning{{site=\"{site}\"}} {}\n",
            m.lan_hairpinning.len()
        ));
        for p in &m.lan_hairpinning {
            s.push_str(&format!(
                "goat_mesh_lan_peer_hairpin{{site=\"{site}\",peer=\"{p}\"}} 1\n"
            ));
        }
        s.push_str("# HELP goat_mesh_lan_peers_unseen Declared LAN peers missing from the vantage roster (typo, deleted, or out of scope).\n");
        s.push_str("# TYPE goat_mesh_lan_peers_unseen gauge\n");
        s.push_str(&format!(
            "goat_mesh_lan_peers_unseen{{site=\"{site}\"}} {}\n",
            m.lan_unseen.len()
        ));
        for p in &m.lan_unseen {
            s.push_str(&format!(
                "goat_mesh_lan_peer_unseen{{site=\"{site}\",peer=\"{p}\"}} 1\n"
            ));
        }
    }
    s
}

/// Write the textfile atomically — a scrape must never see a partial file.
fn write_textfile(path: &Path, body: &str) {
    let tmp = path.with_extension("prom.tmp");
    if let Err(e) = std::fs::write(&tmp, body) {
        warn!(error = %e, path = %tmp.display(), "textfile write failed");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        warn!(error = %e, "textfile rename failed");
    }
}

/// The honest "nothing to read" envelope — stale or missing snapshot.
fn unmetered(site: &str, window_secs: u32, vantage_host: &str) -> EnvHealthV1 {
    EnvHealthV1 {
        domain: "mesh".to_string(),
        scope: site.to_string(),
        ts_unix_ms: now_unix_ms(),
        overall_color: "unmetered".to_string(),
        sample_count: 0,
        window_secs,
        detail: Some(EnvDetail::Mesh(MeshDetailV1 {
            links: Vec::new(),
            vantage_host: vantage_host.to_string(),
            peers_total: 0,
            peers_connected: 0,
            // Deliberately empty, not zero-as-fact: with no snapshot we cannot
            // know whether an always-up peer is absent. `unmetered` says we
            // don't know; an empty list next to it must never be read as
            // "nothing is absent".
            always_up_absent: Vec::new(),
            connecting_stuck: Vec::new(),
            always_up_total: 0,
            attended_total: 0,
            ephemeral_total: 0,
            unclassified_total: 0,
            // Same rule as always_up_absent: unmetered means unknown, so no
            // declared total and no hairpin claims (the textfile gates its lan
            // series on lan_declared_total > 0, which also keeps them out of
            // an unmetered reading).
            lan_declared_total: 0,
            lan_hairpinning: Vec::new(),
            lan_unseen: Vec::new(),
        })),
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "goat-mesh-health",
    about = "Mesh domain of the environment-health surface — per-site WireGuard link-health roll-up (ADR 1042/1036)"
)]
struct Args {
    /// Zenoh router endpoint, e.g. `tls/goat-zenoh-router:7447`.
    #[arg(long, env = "MESH_HEALTH_ENDPOINT")]
    endpoint: String,

    /// Site identifier — the `<site>` segment of ops/mesh/health/<site>.
    #[arg(long, env = "MESH_HEALTH_SITE")]
    site: String,

    /// Path of the netbird status snapshot (bind-mounted read-only; written
    /// atomically by the host-side mesh-health-snapshot systemd timer).
    #[arg(long, env = "MESH_HEALTH_STATUS_FILE", default_value = "/state/netbird-status.json")]
    status_file: String,

    /// Leaf X.509 cert (PEM) for mTLS. Required for a `tls/` endpoint.
    #[arg(long, env = "MESH_HEALTH_CERT")]
    cert: Option<String>,
    /// Matching private key (PEM).
    #[arg(long, env = "MESH_HEALTH_KEY_FILE")]
    key: Option<String>,
    /// CA root cert (PEM) used to verify the router.
    #[arg(long, env = "MESH_HEALTH_ROOT_CA")]
    root_ca: Option<String>,

    /// Publish interval (ms). The published window_secs reflects it.
    #[arg(long, env = "MESH_HEALTH_INTERVAL_MS", default_value_t = 30_000)]
    interval_ms: u64,

    /// Staleness window (ms): a snapshot older than this publishes `unmetered`
    /// (never a false green). Default = ~4 snapshot-timer intervals.
    #[arg(long, env = "MESH_HEALTH_STALE_AFTER_MS", default_value_t = 90_000)]
    stale_after_ms: u64,

    /// DIRECT-link measured latency at/above which a link grades amber (ms).
    /// A direct P2P link should be fast, so this is the tight yardstick.
    #[arg(long, env = "MESH_HEALTH_LATENCY_AMBER_MS", default_value_t = 250.0)]
    latency_amber_ms: f64,

    /// DIRECT-link measured latency at/above which a link grades red (ms).
    #[arg(long, env = "MESH_HEALTH_LATENCY_RED_MS", default_value_t = 600.0)]
    latency_red_ms: f64,

    /// RELAYED-link measured latency at/above which a link grades amber (ms).
    /// A relayed link rides a relay hop (often WAN/satellite) and is EXPECTED to
    /// be slower, so it is judged against a looser baseline — posture sets the
    /// expectation, not the color (env-health-surface.md §3.4).
    #[arg(long, env = "MESH_HEALTH_LATENCY_RELAYED_AMBER_MS", default_value_t = 1000.0)]
    latency_relayed_amber_ms: f64,

    /// RELAYED-link measured latency at/above which a link grades red (ms).
    #[arg(long, env = "MESH_HEALTH_LATENCY_RELAYED_RED_MS", default_value_t = 2500.0)]
    latency_relayed_red_ms: f64,

    /// Declared peer-expectation map (JSON), bind-mounted read-only. Rendered
    /// by the mesh-health role from the operator-authored YAML in group_vars.
    /// Absent or unreadable: every peer is `unclassified` and nothing alerts —
    /// the surface degrades to what it did before, never to a false alarm.
    #[arg(long, env = "MESH_HEALTH_PEER_CLASSES")]
    peer_classes: Option<String>,

    /// An `always-up` peer absent at/beyond this grades red (ms).
    #[arg(long, env = "MESH_HEALTH_ABSENT_RED_MS", default_value_t = 3_600_000)]
    absent_red_ms: i64,

    /// Sustained `Connecting` at/beyond this grades amber — the F-286
    /// half-state, which a restart-based watchdog cannot clear (ms).
    #[arg(long, env = "MESH_HEALTH_CONNECTING_AMBER_MS", default_value_t = 300_000)]
    connecting_amber_ms: i64,

    /// Management peer-registry snapshot (JSON array from `GET /api/peers`),
    /// written host-side by the snapshot timer and bind-mounted read-only.
    /// Unset = the meter behaves exactly as before; it never calls the API
    /// itself, so no token ever enters the container.
    #[arg(long, env = "MESH_HEALTH_REGISTRY_FILE")]
    registry_file: Option<String>,

    /// Prometheus textfile to write each tick (node_exporter textfile
    /// collector). Unset = no export, which is the prior behaviour. The meter
    /// emits this itself rather than standing up a second service, since it
    /// already holds every value an alert needs.
    #[arg(long, env = "MESH_HEALTH_TEXTFILE")]
    textfile: Option<String>,
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

/// Read + parse the snapshot, honestly: stale mtime, missing file, or a parse
/// failure all return None (the caller publishes `unmetered`).
fn read_snapshot(path: &Path, stale_after: Duration) -> Option<NetbirdStatus> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            warn!(error = %e, path = %path.display(), "snapshot missing");
            return None;
        }
    };
    match meta.modified() {
        Ok(mtime) => {
            let age = SystemTime::now()
                .duration_since(mtime)
                .unwrap_or(Duration::ZERO);
            if age > stale_after {
                warn!(age_secs = age.as_secs(), path = %path.display(), "snapshot stale");
                return None;
            }
        }
        Err(e) => {
            warn!(error = %e, "snapshot mtime unreadable");
            return None;
        }
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "snapshot unreadable");
            return None;
        }
    };
    match serde_json::from_slice::<NetbirdStatus>(&bytes) {
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
    let health_key = format!("ops/mesh/health/{}", args.site);
    info!(
        zid = %session.zid(),
        endpoint = args.endpoint.as_str(),
        site = args.site.as_str(),
        status_file = args.status_file.as_str(),
        publishes = health_key.as_str(),
        "session open; grading netbird snapshot -> ops/mesh/health/<site>"
    );

    // Declared expectations are loaded once: the map is reviewed config, and
    // the role restarts the meter when it re-renders. An absent map is not an
    // error — every peer reads `unclassified` and nothing alerts.
    let classes = match args.peer_classes.as_ref() {
        Some(p) => PeerClasses::load(Path::new(p)).unwrap_or_default(),
        None => PeerClasses::default(),
    };
    info!(
        declared_peers = classes.peers.len(),
        declared_patterns = classes.patterns.len(),
        "peer-expectation map loaded"
    );

    let thresholds = Thresholds {
        direct_amber_ms: args.latency_amber_ms,
        direct_red_ms: args.latency_red_ms,
        relayed_amber_ms: args.latency_relayed_amber_ms,
        relayed_red_ms: args.latency_relayed_red_ms,
        absent_red_ms: args.absent_red_ms,
        connecting_amber_ms: args.connecting_amber_ms,
    };

    let interval = Duration::from_millis(args.interval_ms);
    let stale_after = Duration::from_millis(args.stale_after_ms);
    let status_path = std::path::PathBuf::from(&args.status_file);
    let mut transitions = Transitions::default();
    let mut liveness = Liveness::default();
    let registry_path = args.registry_file.as_ref().map(std::path::PathBuf::from);
    if let Some(ref r) = registry_path {
        info!(path = %r.display(), "management peer registry enabled (true last_seen + deleted-vs-descoped)");
    }
    let textfile_path = args.textfile.as_ref().map(std::path::PathBuf::from);
    if let Some(ref tf) = textfile_path {
        info!(path = %tf.display(), "prometheus textfile export enabled");
    }
    let mut last_vantage = String::new();
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("install SIGTERM handler")?;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let window_secs = (interval.as_secs() as u32).max(1);
                // Re-read per tick: the host timer refreshes it on the snapshot
                // cadence, and a registry that has gone stale must stop being
                // trusted rather than keep answering from a frozen copy.
                let registry = match registry_path {
                    Some(ref p) => PeerRegistry::load(p, stale_after),
                    None => PeerRegistry::default(),
                };
                let reading = match read_snapshot(&status_path, stale_after) {
                    Some(status) => {
                        if !status.fqdn.is_empty() {
                            last_vantage = status.fqdn.clone();
                        }
                        roll_up(
                            &status,
                            &mut transitions,
                            &mut liveness,
                            &classes,
                            &registry,
                            &args.site,
                            window_secs,
                            &thresholds,
                            SystemTime::now(),
                        )
                    }
                    None => unmetered(&args.site, window_secs, &last_vantage),
                };
                let n = reading.sample_count;
                let overall = reading.overall_color.clone();
                // Name the absent always-up peers in the log line: this is the
                // product, and it must be legible without a bus subscriber.
                let absent = match reading.detail {
                    Some(EnvDetail::Mesh(ref m)) => m.always_up_absent.join(","),
                    _ => String::new(),
                };
                if !absent.is_empty() {
                    warn!(absent = absent.as_str(), "always-up peers absent");
                }
                // Export BEFORE publishing: if the router is unreachable the
                // publish fails, and that is exactly when an operator most
                // needs the local metrics to still be fresh.
                if let Some(ref tf) = textfile_path {
                    write_textfile(tf, &render_textfile(&reading, &args.site));
                }
                let payload = reading.encode_to_vec();
                match session
                    .put(&health_key, payload)
                    .congestion_control(CongestionControl::Block)
                    .priority(Priority::InteractiveHigh)
                    .await
                {
                    Ok(()) => info!(key = health_key.as_str(), links = n, overall = overall.as_str(), "published mesh health"),
                    Err(e) => warn!(error = %e, "publish mesh health failed"),
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

    // Shape-faithful to live netbird 0.72.2 output (names genericized); the
    // iceCandidateType block matches live 0.75.x output (fleet pin 0.75.1).
    const FIXTURE: &str = r#"{
        "cliVersion":"0.72.2","daemonVersion":"0.72.2","daemonStatus":"Connected",
        "fqdn":"vantage-host.netbird.selfhosted","netbirdIp":"100.127.0.1/16",
        "management":{"url":"https://mgmt.example.net:443","connected":true},
        "peers":{"total":4,"connected":2,"details":[
            {"fqdn":"peer-direct.netbird.selfhosted","netbirdIp":"100.127.0.2",
             "status":"Connected","connectionType":"P2P","relayAddress":"",
             "iceCandidateType":{"local":"host","remote":"host"},
             "lastWireguardHandshake":"2026-06-12T18:04:37Z",
             "transferReceived":1,"transferSent":2,"latency":72025304},
            {"fqdn":"peer-relayed.netbird.selfhosted","netbirdIp":"100.127.0.3",
             "status":"Connected","connectionType":"Relayed",
             "relayAddress":"rels://relay-x.example.net:33073",
             "iceCandidateType":{"local":"relay","remote":"relay"},
             "lastWireguardHandshake":"2026-06-12T18:04:37Z","latency":0},
            {"fqdn":"peer-offline.netbird.selfhosted","netbirdIp":"100.127.0.4",
             "status":"Disconnected","connectionType":"","relayAddress":"",
             "lastWireguardHandshake":"","latency":0},
            {"fqdn":"peer-idle.netbird.selfhosted","netbirdIp":"100.127.0.5/16",
             "status":"Idle","connectionType":"","relayAddress":"",
             "lastWireguardHandshake":"","latency":0}
        ]}}"#;

    fn thresholds() -> Thresholds {
        Thresholds {
            direct_amber_ms: 250.0,
            direct_red_ms: 600.0,
            relayed_amber_ms: 1000.0,
            relayed_red_ms: 2500.0,
            absent_red_ms: 3_600_000,
            connecting_amber_ms: 300_000,
        }
    }

    #[test]
    fn link_color_grades_latency_against_path_expectation() {
        // Thresholds: direct 250/600, relayed 1000/2500.
        let t = thresholds();
        let c = |conn, direct, lat| {
            link_color(conn, direct, lat, CLASS_UNCLASSIFIED, -1, "Connected", &t)
        };

        // DIRECT link: held to the tight yardstick.
        assert_eq!(c(true, true, 72.0), "green");
        assert_eq!(c(true, true, 300.0), "amber");
        assert_eq!(c(true, true, 900.0), "red"); // slow on a link that should be fast

        // RELAYED link: judged against the looser baseline (expected slower) —
        // the SAME latencies read calmer because posture sets the expectation.
        assert_eq!(c(true, false, 300.0), "green"); // normal for a relay
        assert_eq!(c(true, false, 900.0), "green"); // a slow relayed path → green
        assert_eq!(c(true, false, 1500.0), "amber"); // getting slow even for a relay
        assert_eq!(c(true, false, 3000.0), "red"); // broken even for a relay

        // Unmeasured latency (0) never grades hot; disconnected is idle (absence
        // is not illness — §3.4 tenet 2), never red/amber.
        assert_eq!(c(true, true, 0.0), "green");
        assert_eq!(c(true, false, 0.0), "green");
        assert_eq!(c(false, false, 0.0), "idle");
    }

    #[test]
    fn overall_is_worst_grounded_never_false_green() {
        assert_eq!(overall_color(&[]), "unmetered");
        assert_eq!(overall_color(&["idle", "idle"]), "idle");
        assert_eq!(overall_color(&["green", "idle"]), "green");
        assert_eq!(overall_color(&["green", "amber"]), "amber");
        assert_eq!(overall_color(&["green", "amber", "red"]), "red");
    }

    #[test]
    fn parses_live_shaped_status() {
        let s: NetbirdStatus = serde_json::from_str(FIXTURE).expect("parse");
        assert_eq!(s.fqdn, "vantage-host.netbird.selfhosted");
        assert_eq!(s.peers.total, 4);
        assert_eq!(s.peers.connected, 2);
        assert_eq!(s.peers.details.len(), 4);
        assert_eq!(s.peers.details[0].latency, 72025304);
        assert_eq!(s.peers.details[1].relay_address, "rels://relay-x.example.net:33073");
        // ICE candidate types parse when present and default to empty when the
        // key is absent (older daemons; disconnected peers) — an old snapshot
        // must keep parsing whole, never fail the whole roster.
        assert_eq!(s.peers.details[0].ice_candidate_type.local, "host");
        assert_eq!(s.peers.details[1].ice_candidate_type.remote, "relay");
        assert_eq!(s.peers.details[2].ice_candidate_type.local, "");
        assert_eq!(s.peers.details[2].ice_candidate_type.remote, "");
    }

    #[test]
    fn rollup_builds_mesh_envelope() {
        let s: NetbirdStatus = serde_json::from_str(FIXTURE).expect("parse");
        let mut tr = Transitions::default();
        let mut lv = Liveness::default();
        let classes = PeerClasses::default();
        let now = humantime::parse_rfc3339("2026-06-12T18:05:37Z").unwrap();
        let e = roll_up(&s, &mut tr, &mut lv, &classes, &PeerRegistry::default(), "zagros-sandbox", 30, &thresholds(), now);
        assert_eq!(e.domain, "mesh");
        assert_eq!(e.scope, "zagros-sandbox");
        // Both connected links are green (direct @72ms in-bound; relayed with
        // unmeasured latency) → overall green. Path is posture, not health
        // (§3.4), so the relayed link no longer drags this to amber. n =
        // connected links only.
        assert_eq!(e.overall_color, "green");
        assert_eq!(e.sample_count, 2);
        match e.detail {
            Some(EnvDetail::Mesh(ref m)) => {
                assert_eq!(m.links.len(), 4);
                assert_eq!(m.vantage_host, "vantage-host.netbird.selfhosted");
                assert_eq!(m.peers_total, 4);
                assert_eq!(m.peers_connected, 2);
                let direct = m.links.iter().find(|l| l.peer_fqdn.starts_with("peer-direct")).unwrap();
                assert_eq!(direct.color, "green");
                assert!(direct.direct);
                assert!((direct.latency_ms - 72.025304).abs() < 1e-6);
                assert_eq!(direct.handshake_age_ms, 60_000); // 18:05:37 - 18:04:37
                let relayed = m.links.iter().find(|l| l.peer_fqdn.starts_with("peer-relayed")).unwrap();
                // Relayed is GREEN now — path is posture, not health (§3.4); its
                // latency is unmeasured/in-bound. `direct=false` stays on the
                // wire as the posture badge, just not a color input.
                assert_eq!(relayed.color, "green");
                assert!(!relayed.direct);
                assert_eq!(relayed.latency_ms, 0.0);
                let offline = m.links.iter().find(|l| l.peer_fqdn.starts_with("peer-offline")).unwrap();
                assert_eq!(offline.color, "idle");
                assert!(!offline.connected);
                assert_eq!(offline.handshake_age_ms, -1);
                // CIDR suffix stripped.
                let idle = m.links.iter().find(|l| l.peer_fqdn.starts_with("peer-idle")).unwrap();
                assert_eq!(idle.peer_ip, "100.127.0.5");
            }
            _ => panic!("expected Mesh detail"),
        }
        // Round-trips through protobuf.
        let bytes = e.encode_to_vec();
        let back = EnvHealthV1::decode(bytes.as_slice()).expect("decode");
        assert_eq!(back.domain, "mesh");
        assert_eq!(back.overall_color, "green");
    }

    #[test]
    fn transitions_count_path_flips() {
        let mut tr = Transitions::default();
        assert_eq!(tr.observe("p", "P2P"), 0); // first sighting, no flip
        assert_eq!(tr.observe("p", "P2P"), 0);
        assert_eq!(tr.observe("p", "Relayed"), 1); // direct -> relayed
        assert_eq!(tr.observe("p", ""), 1); // disconnect keeps the count + last path
        assert_eq!(tr.observe("p", "P2P"), 2); // reconnect on the other path counts
    }

    #[test]
    fn unmetered_when_nothing_to_read() {
        let e = unmetered("zagros-sandbox", 30, "vantage-host");
        assert_eq!(e.overall_color, "unmetered");
        assert_eq!(e.sample_count, 0);
        // Zero enrolled peers also reads honest (no links → unmetered).
        let s: NetbirdStatus = serde_json::from_str(
            r#"{"fqdn":"v","peers":{"total":0,"connected":0,"details":[]}}"#,
        )
        .unwrap();
        let mut tr = Transitions::default();
        let mut lv = Liveness::default();
        let classes = PeerClasses::default();
        let e = roll_up(&s, &mut tr, &mut lv, &classes, &PeerRegistry::default(), "x", 30, &thresholds(), SystemTime::now());
        assert_eq!(e.overall_color, "unmetered");
    }

    // -- declared expectations -------------------------------------------------

    fn classes_fixture() -> PeerClasses {
        serde_json::from_str(
            r#"{
                "peers": {
                    "peer-offline.netbird.selfhosted": "always-up",
                    "peer-idle.netbird.selfhosted": "ephemeral"
                },
                "patterns": { "*.sensors.example.net": "always-up" }
            }"#,
        )
        .expect("parse class map")
    }

    #[test]
    fn classify_prefers_exact_then_longest_pattern_then_unclassified() {
        let c = classes_fixture();
        assert_eq!(c.classify("peer-offline.netbird.selfhosted"), CLASS_ALWAYS_UP);
        assert_eq!(c.classify("peer-idle.netbird.selfhosted"), CLASS_EPHEMERAL);
        assert_eq!(c.classify("svn-001.sensors.example.net"), CLASS_ALWAYS_UP);
        // Undeclared peers never acquire an expectation by accident.
        assert_eq!(c.classify("laptop.netbird.selfhosted"), CLASS_UNCLASSIFIED);
        // An empty map classifies nothing — the degraded, silent default.
        assert_eq!(PeerClasses::default().classify("anything"), CLASS_UNCLASSIFIED);
    }

    #[test]
    fn absence_alarms_only_for_declared_always_up_peers() {
        let t = thresholds();
        let hour = 3_600_000;
        let c = |class, absent| link_color(false, false, 0.0, class, absent, "Disconnected", &t);

        // always-up: absent is a finding IMMEDIATELY — never green while a peer
        // we declared must be up is missing — escalating to red with duration.
        assert_eq!(c(CLASS_ALWAYS_UP, 0), "amber"); // duration unknown/just seen
        assert_eq!(c(CLASS_ALWAYS_UP, 60_000), "amber");
        assert_eq!(c(CLASS_ALWAYS_UP, hour), "red");
        // A long absence must not read calm.
        assert_eq!(c(CLASS_ALWAYS_UP, 23 * hour), "red");

        // Everything else stays idle however long it is gone — absence is not
        // illness without a declared expectation.
        for class in [CLASS_ATTENDED, CLASS_EPHEMERAL, CLASS_UNCLASSIFIED] {
            assert_eq!(c(class, 23 * hour), "idle", "class {class} must not alarm");
        }
    }

    #[test]
    fn connecting_never_colors_an_undeclared_peer() {
        // A `Connecting` peer is often an ordinary powered-down laptop. The hub vantage
        // cannot distinguish "wedged in the F-286 half-state" from "switched
        // off", so `Connecting` must not colour on its own — otherwise the
        // normal resting state of half the fleet pages.
        let t = thresholds();
        let c = |class, absent| link_color(false, false, 0.0, class, absent, "Connecting", &t);
        let day = 24 * 3_600_000;
        for class in [CLASS_UNCLASSIFIED, CLASS_ATTENDED, CLASS_EPHEMERAL] {
            assert_eq!(c(class, day), "idle", "class {class} must not alarm on Connecting");
        }
        // A peer we DO expect up is graded by its absence, as with any absence.
        assert_eq!(c(CLASS_ALWAYS_UP, 60_000), "amber");
        assert_eq!(c(CLASS_ALWAYS_UP, day), "red");
    }

    #[test]
    fn connecting_stuck_lists_only_declared_always_up_peers() {
        let s: NetbirdStatus = serde_json::from_str(
            r#"{"fqdn":"v","peers":{"total":2,"connected":0,"details":[
                {"fqdn":"sensor.sensors.example.net","status":"Connecting",
                 "netbirdIp":"100.127.0.9","lastWireguardHandshake":"2026-06-12T18:04:37Z","latency":0},
                {"fqdn":"laptop.netbird.selfhosted","status":"Connecting",
                 "netbirdIp":"100.127.0.8","lastWireguardHandshake":"2026-06-12T18:04:37Z","latency":0}
            ]}}"#,
        )
        .unwrap();
        let (mut tr, mut lv) = (Transitions::default(), Liveness::default());
        // One hour after the last handshake — the age floor puts both peers well
        // past the connecting threshold without pre-seeding the tracker.
        let now = humantime::parse_rfc3339("2026-06-12T19:04:37Z").unwrap();
        let e = roll_up(&s, &mut tr, &mut lv, &classes_fixture(), &PeerRegistry::default(), "x", 30, &thresholds(), now);
        match e.detail {
            Some(EnvDetail::Mesh(ref m)) => {
                // The undeclared laptop is absent-and-Connecting too, and stays
                // out of both lists.
                assert_eq!(m.connecting_stuck, vec!["sensor.sensors.example.net"]);
                // always_up_absent also carries peer-offline: classes_fixture()
                // declares it always-up by EXACT name and this mini-roster does
                // not contain it, so the roster check correctly reports a peer
                // the mesh has never heard of. connecting_stuck stays scoped to
                // peers actually seen in `Connecting`.
                assert!(m.always_up_absent.contains(&"sensor.sensors.example.net".to_string()));
                assert!(m.always_up_absent.contains(&"peer-offline.netbird.selfhosted".to_string()));
            }
            _ => panic!("expected Mesh detail"),
        }
    }

    #[test]
    fn liveness_never_shortens_an_absence_across_a_meter_restart() {
        let mut lv = Liveness::default();
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        // Connected peers report -1 and hold no absence state.
        assert_eq!(lv.observe("p", true, -1, t0), -1);
        // First sighting of an absence with no handshake evidence starts at 0…
        assert_eq!(lv.observe("p", false, -1, t0), 0);
        // …and accrues with wall time.
        assert_eq!(lv.observe("p", false, -1, t0 + Duration::from_secs(60)), 60_000);
        // A peer already long gone when the meter starts must not read as
        // freshly absent — handshake age floors the in-process clock.
        let mut fresh = Liveness::default();
        assert_eq!(fresh.observe("q", false, 23 * 3_600_000, t0), 23 * 3_600_000);
        // Reconnecting clears the absence.
        assert_eq!(lv.observe("p", true, -1, t0 + Duration::from_secs(90)), -1);
        assert_eq!(lv.observe("p", false, -1, t0 + Duration::from_secs(90)), 0);
    }

    #[test]
    fn rollup_names_absent_always_up_peers_and_counts_classes() {
        let s: NetbirdStatus = serde_json::from_str(FIXTURE).expect("parse");
        let mut tr = Transitions::default();
        let mut lv = Liveness::default();
        let now = humantime::parse_rfc3339("2026-06-12T18:05:37Z").unwrap();
        let e = roll_up(
            &s,
            &mut tr,
            &mut lv,
            &classes_fixture(),
            &PeerRegistry::default(),
            "zagros-sandbox",
            30,
            &thresholds(),
            now,
        );
        match e.detail {
            Some(EnvDetail::Mesh(ref m)) => {
                // peer-offline is declared always-up and is disconnected, so it
                // is named — the product of this surface.
                assert_eq!(m.always_up_absent, vec!["peer-offline.netbird.selfhosted"]);
                assert_eq!(m.always_up_total, 1);
                assert_eq!(m.ephemeral_total, 1); // peer-idle
                assert_eq!(m.unclassified_total, 2); // the two connected peers
                assert_eq!(m.attended_total, 0);
                // Declared class rides on each link so an operator sees WHY a
                // peer matters without knowing the roster.
                let offline = m
                    .links
                    .iter()
                    .find(|l| l.peer_fqdn.starts_with("peer-offline"))
                    .unwrap();
                assert_eq!(offline.peer_class, CLASS_ALWAYS_UP);
                assert_eq!(offline.status, "Disconnected");
                let idle = m.links.iter().find(|l| l.peer_fqdn.starts_with("peer-idle")).unwrap();
                assert_eq!(idle.peer_class, CLASS_EPHEMERAL);
                assert_eq!(idle.color, "idle"); // ephemeral absence never alarms
            }
            _ => panic!("expected Mesh detail"),
        }
    }

    #[test]
    fn declared_peer_missing_from_the_roster_is_a_finding_not_silence() {
        // The fixture roster contains peer-offline and peer-idle but NOT
        // svn-001.sensors.example.net, which classes_fixture() declares
        // always-up via its pattern... so use an EXACT declaration, which is
        // the only thing that can be checked (a pattern has no membership).
        let classes: PeerClasses = serde_json::from_str(
            r#"{"peers":{"ghost.netbird.selfhosted":"always-up",
                          "peer-offline.netbird.selfhosted":"always-up"},
                "patterns":{}}"#,
        )
        .unwrap();
        let s: NetbirdStatus = serde_json::from_str(FIXTURE).expect("parse");
        let (mut tr, mut lv) = (Transitions::default(), Liveness::default());
        let now = humantime::parse_rfc3339("2026-06-12T18:05:37Z").unwrap();
        let e = roll_up(&s, &mut tr, &mut lv, &classes, &PeerRegistry::default(), "x", 30, &thresholds(), now);

        match e.detail {
            Some(EnvDetail::Mesh(ref m)) => {
                // The ghost is named alongside the ordinarily-absent peer.
                assert!(m.always_up_absent.contains(&"ghost.netbird.selfhosted".to_string()),
                        "a declared peer the mesh has never heard of must be NAMED");
                assert_eq!(m.always_up_total, 2, "both declarations counted");
                let ghost = m.links.iter().find(|l| l.peer_fqdn.starts_with("ghost")).unwrap();
                assert_eq!(ghost.status, "not-in-roster");
                assert_eq!(ghost.color, "amber");
                assert!(!ghost.connected);
                // Duration is genuinely unknown and must not be fabricated.
                assert_eq!(ghost.absent_ms, -1);
            }
            _ => panic!("expected Mesh detail"),
        }
        // And it drives the envelope off green — silence was the bug.
        assert_ne!(e.overall_color, "green");
    }

    #[test]
    fn roster_check_ignores_patterns_and_non_always_up() {
        // A pattern entry has no finite membership, and a declared `ephemeral`
        // peer being unknown to the mesh is not a finding.
        let classes: PeerClasses = serde_json::from_str(
            r#"{"peers":{"gone-laptop.netbird.selfhosted":"ephemeral"},
                "patterns":{"*.sensors.example.net":"always-up"}}"#,
        )
        .unwrap();
        let s: NetbirdStatus = serde_json::from_str(FIXTURE).expect("parse");
        let (mut tr, mut lv) = (Transitions::default(), Liveness::default());
        let e = roll_up(&s, &mut tr, &mut lv, &classes, &PeerRegistry::default(), "x", 30, &thresholds(), SystemTime::now());
        match e.detail {
            Some(EnvDetail::Mesh(ref m)) => {
                assert!(m.always_up_absent.is_empty(), "no phantom findings");
                assert!(!m.links.iter().any(|l| l.status == "not-in-roster"));
            }
            _ => panic!("expected Mesh detail"),
        }
    }

    fn registry_from(json: &str) -> PeerRegistry {
        let peers: Vec<RegistryPeer> = serde_json::from_str(json).unwrap();
        let mut by_label = HashMap::new();
        for p in peers {
            by_label.insert(p.dns_label.clone(), p);
        }
        PeerRegistry { by_label, fetched: true }
    }

    #[test]
    fn registry_separates_deleted_from_merely_out_of_scope() {
        // The whole reason for reading the control plane. Locally these look
        // identical — the peer simply is not in `netbird status`.
        let classes: PeerClasses = serde_json::from_str(
            r#"{"peers":{"deleted.netbird.selfhosted":"always-up",
                          "descoped.netbird.selfhosted":"always-up"},
                "patterns":{}}"#,
        )
        .unwrap();
        // The registry knows `descoped` and has never heard of `deleted`.
        let reg = registry_from(
            r#"[{"dns_label":"descoped","hostname":"descoped","connected":false,
                 "last_seen":"2026-06-12T12:00:00Z"}]"#,
        );
        let s: NetbirdStatus =
            serde_json::from_str(r#"{"fqdn":"v","peers":{"total":0,"connected":0,"details":[]}}"#)
                .unwrap();
        let (mut tr, mut lv) = (Transitions::default(), Liveness::default());
        let now = humantime::parse_rfc3339("2026-06-12T18:00:00Z").unwrap();
        let e = roll_up(&s, &mut tr, &mut lv, &classes, &reg, "x", 30, &thresholds(), now);

        match e.detail {
            Some(EnvDetail::Mesh(ref m)) => {
                let del = m.links.iter().find(|l| l.peer_fqdn.starts_with("deleted")).unwrap();
                let des = m.links.iter().find(|l| l.peer_fqdn.starts_with("descoped")).unwrap();
                // Deleted is the harder finding: it cannot come back without
                // re-enrolment, and the declaration naming it is now fiction.
                assert_eq!(del.status, "not-in-registry");
                assert_eq!(del.color, "red");
                // Descoped exists; we just cannot see it. Visibility problem.
                assert_eq!(des.status, "out-of-vantage-scope");
                assert_eq!(des.color, "amber");
                // And the registry supplies a real duration for the one it knows.
                assert_eq!(des.absent_ms, 6 * 3_600_000);
                assert_eq!(del.absent_ms, -1, "unknown stays unknown");
            }
            _ => panic!("expected Mesh detail"),
        }
        assert_eq!(e.overall_color, "red");
    }

    #[test]
    fn without_a_registry_the_question_is_unanswerable_not_answered_no() {
        // Absent registry must NOT be read as "the control plane has no record".
        let classes: PeerClasses = serde_json::from_str(
            r#"{"peers":{"ghost.netbird.selfhosted":"always-up"},"patterns":{}}"#,
        )
        .unwrap();
        let s: NetbirdStatus =
            serde_json::from_str(r#"{"fqdn":"v","peers":{"total":0,"connected":0,"details":[]}}"#)
                .unwrap();
        let (mut tr, mut lv) = (Transitions::default(), Liveness::default());
        let e = roll_up(&s, &mut tr, &mut lv, &classes, &PeerRegistry::default(), "x", 30,
                        &thresholds(), SystemTime::now());
        match e.detail {
            Some(EnvDetail::Mesh(ref m)) => {
                let g = m.links.iter().find(|l| l.peer_fqdn.starts_with("ghost")).unwrap();
                assert_eq!(g.status, "not-in-roster");
                assert_eq!(g.color, "amber", "unanswerable must not escalate to red");
            }
            _ => panic!("expected Mesh detail"),
        }
        assert_eq!(PeerRegistry::default().knows("anything"), None);
    }

    #[test]
    fn registry_last_seen_is_a_floor_never_a_ceiling() {
        // Whichever source says the peer has been gone LONGEST is believed:
        // a fresh handshake stamp must not shorten a long registry absence, and
        // a registry that has never seen the peer must not shorten a real
        // handshake age.
        let now = humantime::parse_rfc3339("2026-06-12T18:00:00Z").unwrap();
        assert_eq!(last_seen_age_ms("2026-06-12T12:00:00Z", now), 6 * 3_600_000);
        // netbird's zero value for "never connected" is not a 2000-year absence.
        assert_eq!(last_seen_age_ms("0001-01-01T00:00:00Z", now), -1);
        assert_eq!(last_seen_age_ms("", now), -1);
        assert_eq!(last_seen_age_ms("not-a-date", now), -1);
        // The join key is the label half of the FQDN.
        assert_eq!(PeerRegistry::label_of("node-a.netbird.selfhosted"), "node-a");
        assert_eq!(PeerRegistry::label_of("bare"), "bare");
    }

    /// Emits a REAL encoded EnvHealthV1 for goat-map's Python decoder tests.
    /// Ignored by default; `cargo test emit_wire_fixture -- --ignored` regenerates it.
    /// Hand-built fixtures would only prove the Python decoder agrees with itself;
    /// this proves it agrees with the encoder that actually publishes.
    #[test]
    #[ignore]
    fn emit_wire_fixture() {
        use std::io::Write;
        let s: NetbirdStatus = serde_json::from_str(FIXTURE).unwrap();
        let (mut tr, mut lv) = (Transitions::default(), Liveness::default());
        let now = humantime::parse_rfc3339("2026-06-12T18:05:37Z").unwrap();
        let e = roll_up(&s, &mut tr, &mut lv, &classes_fixture(), &PeerRegistry::default(),
                        "zagros-prod", 30, &thresholds(), now);
        let bytes = e.encode_to_vec();
        let path = std::env::var("FIXTURE_OUT")
            .unwrap_or_else(|_| "/tmp/envhealth_mesh.bin".to_string());
        std::fs::File::create(&path).unwrap().write_all(&bytes).unwrap();
        eprintln!("wrote {} bytes to {}", bytes.len(), path);
    }

    #[test]
    fn textfile_never_reports_all_clear_while_unmetered() {
        // The alerting counterpart of the counter-that-read-zero: with no
        // usable snapshot the export must carry NO absent count and NO per-peer
        // series, or a rule reads total blindness as "nothing is absent".
        let e = unmetered("zagros-prod", 30, "vantage");
        let out = render_textfile(&e, "zagros-prod");
        assert!(out.contains("goat_mesh_metered{site=\"zagros-prod\"} 0"));
        assert!(out.contains("goat_mesh_overall_severity{site=\"zagros-prod\"} -1"),
                "unmetered must be -1, never 0 — unknown is not healthy");
        assert!(!out.contains("goat_mesh_always_up_absent"),
                "no absent count while unmetered");
        assert!(!out.contains("goat_mesh_peer_connected"),
                "no per-peer series while unmetered");
        // Liveness of the meter itself stays observable.
        assert!(out.contains("goat_mesh_last_reading_timestamp_seconds"));
    }

    #[test]
    fn textfile_exports_declared_class_and_absence() {
        let s: NetbirdStatus = serde_json::from_str(FIXTURE).expect("parse");
        let (mut tr, mut lv) = (Transitions::default(), Liveness::default());
        let now = humantime::parse_rfc3339("2026-06-12T18:05:37Z").unwrap();
        let e = roll_up(&s, &mut tr, &mut lv, &classes_fixture(), &PeerRegistry::default(), "zagros-prod", 30, &thresholds(), now);
        let out = render_textfile(&e, "zagros-prod");

        assert!(out.contains("goat_mesh_metered{site=\"zagros-prod\"} 1"));
        // peer-offline is declared always-up and absent → counted and named.
        assert!(out.contains("goat_mesh_always_up_absent{site=\"zagros-prod\"} 1"));
        assert!(out.contains(
            "goat_mesh_peer_connected{site=\"zagros-prod\",peer=\"peer-offline.netbird.selfhosted\",class=\"always-up\"} 0"
        ));
        // The declared class rides on the metric, so a rule can page on
        // always-up alone without re-deriving the roster.
        assert!(out.contains("class=\"ephemeral\""));
        assert!(out.contains("goat_mesh_declared_peers{site=\"zagros-prod\",class=\"always-up\"} 1"));
        // Absence seconds present for absent peers, omitted for connected ones.
        assert!(out.contains("goat_mesh_peer_absent_seconds{site=\"zagros-prod\",peer=\"peer-offline.netbird.selfhosted\""));
        assert!(!out.contains("goat_mesh_peer_absent_seconds{site=\"zagros-prod\",peer=\"peer-direct.netbird.selfhosted\""));
        // An absent always-up peer drives severity to at least amber.
        assert!(out.contains("goat_mesh_overall_severity{site=\"zagros-prod\"} 2")
             || out.contains("goat_mesh_overall_severity{site=\"zagros-prod\"} 3"));
        // Every emitted line is valid exposition (name{labels} value).
        for line in out.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
            assert!(line.matches(' ').count() >= 1 && line.starts_with("goat_mesh_"),
                    "malformed exposition line: {line}");
        }
    }

    #[test]
    fn lan_hairpin_counts_positive_evidence_only() {
        // Issue #240: every leg needs positive evidence — declared, connected,
        // AND netbird reported the selected pair's candidate types. A missing
        // ICE block or a disconnected peer counts NOTHING (a fabricated
        // hairpin is as bad as a fabricated green).
        let s: NetbirdStatus = serde_json::from_str(
            r#"{"fqdn":"node-1.netbird.selfhosted","peers":{"total":5,"connected":4,"details":[
                {"fqdn":"node-2.netbird.selfhosted","netbirdIp":"100.127.0.2","status":"Connected",
                 "connectionType":"P2P","iceCandidateType":{"local":"host","remote":"host"},
                 "lastWireguardHandshake":"2026-06-12T18:04:37Z","latency":1000000},
                {"fqdn":"node-3.netbird.selfhosted","netbirdIp":"100.127.0.3","status":"Connected",
                 "connectionType":"P2P","iceCandidateType":{"local":"srflx","remote":"host"},
                 "lastWireguardHandshake":"2026-06-12T18:04:37Z","latency":9000000},
                {"fqdn":"node-4.netbird.selfhosted","netbirdIp":"100.127.0.4","status":"Connected",
                 "connectionType":"P2P",
                 "lastWireguardHandshake":"2026-06-12T18:04:37Z","latency":1000000},
                {"fqdn":"node-5.netbird.selfhosted","netbirdIp":"100.127.0.5","status":"Disconnected",
                 "connectionType":"","lastWireguardHandshake":"","latency":0},
                {"fqdn":"stranger.netbird.selfhosted","netbirdIp":"100.127.0.6","status":"Connected",
                 "connectionType":"Relayed","iceCandidateType":{"local":"relay","remote":"relay"},
                 "lastWireguardHandshake":"2026-06-12T18:04:37Z","latency":0}
            ]}}"#,
        )
        .expect("parse");
        // The vantage's own FQDN rides in the declared list (one list per host
        // group) and must stay inert.
        let classes: PeerClasses = serde_json::from_str(
            r#"{"peers":{},"patterns":{},"lan_peers":[
                "node-1.netbird.selfhosted","node-2.netbird.selfhosted",
                "node-3.netbird.selfhosted","node-4.netbird.selfhosted",
                "node-5.netbird.selfhosted"]}"#,
        )
        .expect("parse classes");
        let (mut tr, mut lv) = (Transitions::default(), Liveness::default());
        let now = humantime::parse_rfc3339("2026-06-12T18:05:37Z").unwrap();
        let e = roll_up(&s, &mut tr, &mut lv, &classes, &PeerRegistry::default(), "zagros-prod", 30,
                        &thresholds(), now);
        match e.detail {
            Some(EnvDetail::Mesh(ref m)) => {
                assert_eq!(m.lan_declared_total, 4, "self-entry excluded");
                // Every declared neighbour is in the snapshot, and the
                // vantage's own entry is never reported as unseen.
                assert!(m.lan_unseen.is_empty());
                // Only node-3: on-LAN by declaration, connected, and on a
                // srflx leg. host/host (b), no-ICE-data (d), disconnected (e)
                // and the undeclared relayed stranger all count nothing.
                assert_eq!(m.lan_hairpinning, vec!["node-3.netbird.selfhosted".to_string()]);
                let c = m.links.iter().find(|l| l.peer_fqdn.starts_with("node-3")).unwrap();
                assert!(c.lan_declared);
                assert_eq!(c.ice_local, "srflx");
                assert_eq!(c.ice_remote, "host");
                // Path is posture, not health (§3.4): hairpinning names, it
                // does not colour — the link itself stays green.
                assert_eq!(c.color, "green");
                let d = m.links.iter().find(|l| l.peer_fqdn.starts_with("node-4")).unwrap();
                assert!(d.lan_declared);
                assert_eq!(d.ice_local, "", "absent ICE stays empty, never invented");
            }
            _ => panic!("expected Mesh detail"),
        }
        // Round-trips through protobuf with the new tags.
        let bytes = e.encode_to_vec();
        let back = EnvHealthV1::decode(bytes.as_slice()).expect("decode");
        match back.detail {
            Some(EnvDetail::Mesh(ref m)) => {
                assert_eq!(m.lan_declared_total, 4);
                assert_eq!(m.lan_hairpinning.len(), 1);
            }
            _ => panic!("expected Mesh detail after decode"),
        }
    }

    #[test]
    fn declared_lan_peer_missing_from_the_roster_is_named_not_silent() {
        // A `lan_peers` entry that is not also an always-up `peers` entry used
        // to count toward lan_declared_total while nothing checked it against
        // the roster: a typo'd FQDN inflated the declared count and was
        // reported on no path. It is now named in lan_unseen.
        let s: NetbirdStatus = serde_json::from_str(FIXTURE).expect("parse");
        let classes: PeerClasses = serde_json::from_str(
            r#"{"peers":{},"patterns":{},"lan_peers":[
                "peer-relayed.netbird.selfhosted",
                "peer-relayd.netbird.selfhosted"]}"#,
        )
        .expect("parse classes");
        let (mut tr, mut lv) = (Transitions::default(), Liveness::default());
        let now = humantime::parse_rfc3339("2026-06-12T18:05:37Z").unwrap();
        let e = roll_up(&s, &mut tr, &mut lv, &classes, &PeerRegistry::default(), "zagros-prod", 30,
                        &thresholds(), now);
        match e.detail {
            Some(EnvDetail::Mesh(ref m)) => {
                assert_eq!(m.lan_declared_total, 2);
                assert_eq!(m.lan_unseen, vec!["peer-relayd.netbird.selfhosted".to_string()]);
                // Posture, not health: an unseen LAN entry adds no link and no
                // absence, because the LAN list declares locality, not uptime.
                assert!(m.always_up_absent.is_empty());
                assert!(!m.links.iter().any(|l| l.peer_fqdn.starts_with("peer-relayd")));
            }
            _ => panic!("expected Mesh detail"),
        }
        let back = EnvHealthV1::decode(e.encode_to_vec().as_slice()).expect("decode");
        match back.detail {
            Some(EnvDetail::Mesh(ref m)) => assert_eq!(m.lan_unseen.len(), 1),
            _ => panic!("expected Mesh detail after decode"),
        }
        let out = render_textfile(&e, "zagros-prod");
        assert!(out.contains("goat_mesh_lan_peers_unseen{site=\"zagros-prod\"} 1"));
        assert!(out.contains(
            "goat_mesh_lan_peer_unseen{site=\"zagros-prod\",peer=\"peer-relayd.netbird.selfhosted\"} 1"
        ));
    }

    #[test]
    fn textfile_lan_series_gated_on_declaration() {
        // No declaration -> NO lan series at all: a rule must not read "not
        // declared" as "not hairpinning" (the F-287 counter-that-read-zero
        // shape). Declared -> count plus named per-peer series.
        let s: NetbirdStatus = serde_json::from_str(FIXTURE).expect("parse");
        let (mut tr, mut lv) = (Transitions::default(), Liveness::default());
        let now = humantime::parse_rfc3339("2026-06-12T18:05:37Z").unwrap();
        let undeclared = roll_up(&s, &mut tr, &mut lv, &classes_fixture(), &PeerRegistry::default(),
                                 "zagros-prod", 30, &thresholds(), now);
        let out = render_textfile(&undeclared, "zagros-prod");
        assert!(!out.contains("goat_mesh_lan_"), "no lan series without a declaration");

        let classes: PeerClasses = serde_json::from_str(
            r#"{"peers":{},"patterns":{},"lan_peers":["peer-relayed.netbird.selfhosted"]}"#,
        )
        .unwrap();
        let (mut tr, mut lv) = (Transitions::default(), Liveness::default());
        let declared = roll_up(&s, &mut tr, &mut lv, &classes, &PeerRegistry::default(),
                               "zagros-prod", 30, &thresholds(), now);
        let out = render_textfile(&declared, "zagros-prod");
        assert!(out.contains("goat_mesh_lan_peers_declared{site=\"zagros-prod\"} 1"));
        // peer-relayed is declared same-LAN and sits on relay/relay -> hairpin.
        assert!(out.contains("goat_mesh_lan_peers_hairpinning{site=\"zagros-prod\"} 1"));
        assert!(out.contains(
            "goat_mesh_lan_peer_hairpin{site=\"zagros-prod\",peer=\"peer-relayed.netbird.selfhosted\"} 1"
        ));
    }

    #[test]
    fn unmetered_absent_list_is_unknown_not_empty_fact() {
        // An empty always_up_absent beside `unmetered` must never be read as
        // "nothing is absent" — that is the F-287 counter-that-read-zero shape.
        let e = unmetered("zagros-sandbox", 30, "vantage-host");
        assert_eq!(e.overall_color, "unmetered");
        match e.detail {
            Some(EnvDetail::Mesh(ref m)) => {
                assert!(m.always_up_absent.is_empty());
                assert_eq!(m.always_up_total, 0);
            }
            _ => panic!("expected Mesh detail"),
        }
    }
}

