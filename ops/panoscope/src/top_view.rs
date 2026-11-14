// SPDX-License-Identifier: Apache-2.0
//
// Web-native `goat top` view model (/api/top) — ADR 1046 rendered in the
// browser, viewer-scoped from the first commit (ADR 1073/1074).
//
// Mirrors crates/goat-cli/src/tui/top.rs::view_model: the same honesty
// discipline, computed once server-side so every client renders the same
// truth — absent → "unmetered", stale → "idle" + age, `n` on every
// reading, no envelope → `—` (never a faked green). Data source is the
// firehose sample buffer: the `ops/**` health + footprint beats already
// flow through panoscope's `**` subscriber, so "current state" is the
// newest ring entry per key (arrival-stamped — the honest staleness
// clock; we do not trust a beat's own timestamp for liveness).
//
// SCOPING (the reason this module exists at all): a footprint's carrier
// key is floor-visible `ops/fabric/footprint/<peer>`, but its content
// names an app producer. Rows therefore gate per PRODUCER through
// `ViewerScope::allows_producer_row` — the same rule the snapshot's node
// filter enforces. Aggregates that name no identity (fabric class loads,
// weather chips, link/presence counts) stay visible to every tier, the
// same class as the deliberate `/api/stats` exception. Totals are
// computed over the VISIBLE rows only, so a scoped response never
// implies the hidden fleet.
//
// The peer domain (goatcore ADR 1232) names machines and their owners: a
// per-peer row carries a Zenoh ID and an attributed name. Those rows are
// shown at the Full tier only; every other tier sees the per-site grade
// and a count of the rows withheld, never the identities.
//
// The trust-refusal reading (a client refuses the router's certificate) is
// site-level and identity-free: the trust verdict, its reason and the TLS
// alert counts per name show at every tier. Its candidate rows (declared
// peers with no session while refusals arrive) carry a Zenoh ID or a source
// address, so they follow the identity rule: Full tier only. A candidate is
// worded as a candidate, never as a machine at fault, and is never red. A
// trust reading that is not measured is said so and never reads green.

use serde::Serialize;

use crate::health_wire::{
    fmt_age, short_name, EnvDetail, EnvHealthV1, FabricHealthV1, PeerFaultDetailV1, PeerFaultV1,
    ProducerFootprintV1, FOOTPRINT_PREFIX, HEALTH_PREFIX, MESH_HEALTH_PREFIX,
    PEER_FAULT_CLOCK_AHEAD, PEER_FAULT_TRUST_ROOT_REFUSED_CANDIDATE, PEER_HEALTH_PREFIX,
    PRESENCE_HEALTH_PREFIX, TIME_HEALTH_PREFIX,
};
use crate::sample_buffer::SampleBuffer;
use crate::viewer::ViewerScope;
use panoscope_core::alias::AliasRegistry;
use panoscope_core::envelope::EnvelopeRegistry;
use panoscope_core::Topology;
use prost::Message as _;

/// A beat is stale once its last arrival is older than this multiple of
/// its own publish window (same constant as the TUI).
const STALE_FACTOR: u64 = 3;
/// Fallback window (secs) when a beat carries no `window_secs`.
const DEFAULT_WINDOW_SECS: u64 = 5;
/// The env-health domains in canonical strip order.
const DOMAINS: [&str; 5] = ["mesh", "fabric", "time", "presence", "peer"];

/// One decoded reading + the arrival timestamp of the sample it rode.
pub struct Beat<T> {
    pub msg: T,
    pub seen_ms: u64,
}

/// The decoded current-state inputs, latest-wins per key.
#[derive(Default)]
pub struct TopSources {
    pub fabric: Option<Beat<FabricHealthV1>>,
    pub mesh: Option<Beat<EnvHealthV1>>,
    pub time: Option<Beat<EnvHealthV1>>,
    pub presence: Option<Beat<EnvHealthV1>>,
    /// The peer domain, one beat per site (latest per key), sorted by site.
    pub peer: Vec<Beat<EnvHealthV1>>,
    pub footprints: Vec<Beat<ProducerFootprintV1>>,
}

/// Decode the current health/footprint state out of the firehose sample
/// buffer. Undecodable payloads (wrong format, truncated past the ring's
/// per-entry cap) are skipped — a beat we can't prove is a beat we don't
/// assert.
pub fn collect_sources(buf: &SampleBuffer) -> TopSources {
    let mut src = TopSources::default();

    for e in buf.latest_by_prefix(HEALTH_PREFIX) {
        let Some(raw) = &e.raw_bytes else { continue };
        if let Ok(h) = FabricHealthV1::decode(raw.as_slice()) {
            if src.fabric.as_ref().map_or(true, |b| e.ts_ms >= b.seen_ms) {
                src.fabric = Some(Beat {
                    msg: h,
                    seen_ms: e.ts_ms,
                });
            }
        }
    }
    for e in buf.latest_by_prefix(FOOTPRINT_PREFIX) {
        let Some(raw) = &e.raw_bytes else { continue };
        if let Ok(fp) = ProducerFootprintV1::decode(raw.as_slice()) {
            if !fp.peer.is_empty() {
                src.footprints.push(Beat {
                    msg: fp,
                    seen_ms: e.ts_ms,
                });
            }
        }
    }
    for (prefix, slot) in [
        (MESH_HEALTH_PREFIX, &mut src.mesh),
        (TIME_HEALTH_PREFIX, &mut src.time),
        (PRESENCE_HEALTH_PREFIX, &mut src.presence),
    ] {
        for e in buf.latest_by_prefix(prefix) {
            let Some(raw) = &e.raw_bytes else { continue };
            if let Ok(env) = EnvHealthV1::decode(raw.as_slice()) {
                if slot.as_ref().map_or(true, |b| e.ts_ms >= b.seen_ms) {
                    *slot = Some(Beat {
                        msg: env,
                        seen_ms: e.ts_ms,
                    });
                }
            }
        }
    }
    for e in buf.latest_by_prefix(PEER_HEALTH_PREFIX) {
        let Some(raw) = &e.raw_bytes else { continue };
        if let Ok(env) = EnvHealthV1::decode(raw.as_slice()) {
            if env.domain == "peer" {
                src.peer.push(Beat {
                    msg: env,
                    seen_ms: e.ts_ms,
                });
            }
        }
    }
    src.peer.sort_by(|a, b| a.msg.scope.cmp(&b.msg.scope));
    src
}

// --- the serialized view model ------------------------------------------------

#[derive(Serialize)]
pub struct ChipView {
    pub domain: String,
    /// Honesty state string: a grounded color ("green"/"amber"/"red"),
    /// or "idle" (stale-collapsed) / "unmetered" (no beat ever seen).
    pub state: String,
    pub sample_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale_age: Option<String>,
}

#[derive(Serialize)]
pub struct ClassRow {
    pub class: String,
    /// Display-only legend name (control/mission/situational/bulk); ""
    /// for a class code the legend doesn't know (rendered bare).
    pub label: String,
    pub sample_rate: f64,
    pub target_sample_rate: f64,
    pub ratio: f64,
    pub color: String,
}

/// The fabric class-load pane: rows, or an honest absent/stale note.
#[derive(Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum ClassPane {
    Absent,
    Stale { age: String },
    Rows { rows: Vec<ClassRow> },
}

#[derive(Serialize)]
pub struct ProducerRow {
    /// 1-based display position under the msgs/s sort (clients may re-sort).
    pub pos: usize,
    pub anchor: String,
    /// Alias-rendered display name (ADR 1039) — falls back to the
    /// truncated UUID, never errors.
    pub display: String,
    pub sample_rate: f64,
    pub byte_rate: f64,
    /// Compact class-share string, e.g. "C2 80% · C0 20%"; "—" when the
    /// footprint carried no shares.
    pub shares: String,
    pub near_limit: bool,
    /// Envelope band label (ADR 1045): OVER/HIGH/ok/LOW, or "—" when no
    /// envelope is declared (honest no-verdict).
    pub band_label: String,
    /// Band honesty color; "" for the no-envelope "—".
    pub band_color: String,
    /// Mesh path ("direct P2P" / "relayed via <relay>" / "offline") or
    /// "—" when no mesh link matches — never a guessed join.
    pub path: String,
    pub latency: String,
}

#[derive(Serialize)]
pub struct Totals {
    /// Producer rows visible AT THIS VIEWER'S SCOPE (not the fleet count —
    /// the fleet aggregate is `fabric_active_producers`, which the meter
    /// itself publishes as an identity-free aggregate).
    pub visible: usize,
    pub total_msgs: f64,
    pub total_bytes: f64,
    pub direct: usize,
    pub relayed: usize,
    pub alive: u32,
    pub dropped: u32,
}

/// One peer-fault row (Full tier only): who, how far ahead, how often; or
/// a trust-refusal candidate: who holds no session while refusals arrive.
#[derive(Serialize)]
pub struct PeerRow {
    /// The attributed name when the reader had one, else the Zenoh ID, else
    /// the source address (a candidate declared by address has no Zenoh ID).
    pub label: String,
    pub zid: String,
    /// "clock_ahead" / "trust_candidate" / "unknown".
    pub kind: &'static str,
    /// A trust-refusal candidate: not confirmed, not a machine at fault.
    pub candidate: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_address: Option<String>,
    /// Candidates: how long the reader has seen the peer hold no session,
    /// e.g. "4m ago" (since the reader started; a lower bound).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub absent_since: Option<String>,
    /// What the pane prints in the grade column: the grade, or
    /// "candidate (not confirmed)" for a candidate.
    pub grade_label: String,
    pub offset_seconds: f64,
    pub rejections_per_minute: f64,
    /// "yes" / "no" / "unknown" (the reader had no sessions file).
    pub direct_session: &'static str,
    /// The row's grade from the reader: peer_at_fault, local_clock_suspect,
    /// quiet, cleared, unmeasured, candidate.
    pub grade: String,
    /// The row's color; a candidate is amber whatever the reader sent.
    pub color: String,
}

/// One TLS alert name the router received in the window (identity-free).
#[derive(Serialize)]
pub struct TlsAlertRow {
    pub name: String,
    pub count: u64,
    pub per_minute: f64,
    /// In the trust-refusal set; other names are counted, grade nothing.
    pub trust_refusal: bool,
}

/// The trust-refusal line of one site (identity-free, every tier).
#[derive(Serialize)]
pub struct TrustLine {
    /// The reader's trust verdict: clean / unattributed / candidates /
    /// unmeasured, or "stale" when the beat is stale.
    pub verdict: String,
    /// What the pane prints: "not measured" for unmeasured, never "clean".
    pub label: String,
    /// green / amber / unmetered / idle. Unmeasured is unmetered, never green.
    pub state: &'static str,
    /// The trust part of the reader's reason.
    pub reason: String,
    pub alerts: Vec<TlsAlertRow>,
    /// Declared peers holding no session; None = cannot be judged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_peers_absent: Option<u32>,
}

/// One site's peer-domain reading.
#[derive(Serialize)]
pub struct PeerSiteRow {
    pub site: String,
    /// Honesty state as for the chips: the reading's color, or "idle"
    /// when the beat is stale.
    pub state: String,
    /// The reader's verdict (peer_at_fault, local_clock_suspect, recovering,
    /// clean, unmeasured), or "stale" when the beat is stale.
    pub verdict: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale_age: Option<String>,
    /// The peering this reader serves; "" when it serves no single peering.
    pub peering: String,
    pub peers: Vec<PeerRow>,
    /// Peer rows withheld at this viewer's scope (identities).
    pub peers_withheld: usize,
    /// Of those, trust-refusal candidates.
    pub candidates_withheld: usize,
    /// The trust-refusal line; None from a reader that predates it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust: Option<TrustLine>,
}

#[derive(Serialize)]
pub struct TopView {
    /// The viewer's evaluated tier: "full" / "data-all" / "scoped" / "floor".
    pub scope_tier: &'static str,
    /// True when producer rows may have been withheld by the viewer's
    /// scope — the frontend renders a "scoped by your role" note so a
    /// filtered table is never mistaken for the whole fabric (the same
    /// honesty rule the TUI applies to its `/` filter).
    pub restricted: bool,
    pub chips: Vec<ChipView>,
    pub classes: ClassPane,
    /// Peer domain, one row per site; empty when no site has published.
    pub peer_sites: Vec<PeerSiteRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    /// The meter-declared fleet producer count (identity-free aggregate
    /// off the fabric weather beat); `null` when no beat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fabric_active_producers: Option<u32>,
    pub producers: Vec<ProducerRow>,
    pub totals: Totals,
    pub generated_at_ms: u64,
}

pub fn scope_tier(scope: &ViewerScope) -> &'static str {
    match scope {
        ViewerScope::Full => "full",
        ViewerScope::DataAll => "data-all",
        ViewerScope::Scoped { anchors, .. } if anchors.is_empty() => "floor",
        ViewerScope::Scoped { .. } => "scoped",
    }
}

/// Project the decoded sources into the scoped view model. Pure over its
/// inputs (clock passed in) so the honesty rules are unit-testable.
pub fn build_top_view(
    scope: &ViewerScope,
    src: &TopSources,
    topology: &Topology,
    aliases: &AliasRegistry,
    envelopes: &EnvelopeRegistry,
    now_ms: u64,
) -> TopView {
    let stale = |seen_ms: u64, window_secs: u32| -> bool {
        let w = (window_secs as u64).max(1);
        now_ms.saturating_sub(seen_ms) >= w * STALE_FACTOR * 1000
    };

    // Weather chips, canonical order. Fabric wraps its own shape.
    let chips = DOMAINS
        .iter()
        .map(|domain| {
            let (color, n, window, seen) = match *domain {
                "fabric" => match &src.fabric {
                    None => (None, 0, 0, 0),
                    Some(b) => (
                        Some(b.msg.overall_color.clone()),
                        b.msg.classes.iter().map(|c| c.sample_count).sum(),
                        b.msg.window_secs,
                        b.seen_ms,
                    ),
                },
                "peer" => peer_chip_inputs(&src.peer, &stale),
                _ => {
                    let slot = match *domain {
                        "mesh" => &src.mesh,
                        "time" => &src.time,
                        _ => &src.presence,
                    };
                    match slot {
                        None => (None, 0, 0, 0),
                        Some(b) => (
                            Some(b.msg.overall_color.clone()),
                            b.msg.sample_count,
                            b.msg.window_secs,
                            b.seen_ms,
                        ),
                    }
                }
            };
            match color {
                None => ChipView {
                    domain: domain.to_string(),
                    state: "unmetered".to_string(),
                    sample_count: 0,
                    stale_age: None,
                },
                Some(color) => {
                    let is_stale = stale(seen, window);
                    ChipView {
                        domain: domain.to_string(),
                        state: if is_stale { "idle".to_string() } else { color },
                        sample_count: n,
                        stale_age: is_stale.then(|| fmt_age(now_ms.saturating_sub(seen) as i64)),
                    }
                }
            }
        })
        .collect();

    // Fabric class-load pane.
    let classes = match &src.fabric {
        None => ClassPane::Absent,
        Some(b) if stale(b.seen_ms, b.msg.window_secs) => ClassPane::Stale {
            age: fmt_age(now_ms.saturating_sub(b.seen_ms) as i64),
        },
        Some(b) => ClassPane::Rows {
            rows: b
                .msg
                .classes
                .iter()
                .map(|c| ClassRow {
                    class: c.class.clone(),
                    label: class_label(&c.class).to_string(),
                    sample_rate: c.sample_rate,
                    target_sample_rate: c.target_sample_rate,
                    ratio: if c.target_sample_rate > 0.0 {
                        (c.sample_rate / c.target_sample_rate).clamp(0.0, 1.0)
                    } else {
                        0.0
                    },
                    color: c.color.clone(),
                })
                .collect(),
        },
    };

    // Peer domain: one row per site; identities at the Full tier only.
    let show_peers = matches!(scope, ViewerScope::Full);
    let peer_sites = src
        .peer
        .iter()
        .map(|b| peer_site_row(b, show_peers, stale(b.seen_ms, b.msg.window_secs), now_ms))
        .collect();

    // Producer rows: expire (footprints age out, never linger false-live),
    // SCOPE-GATE per producer, then sort by msgs/s desc.
    let fp_window_secs = src
        .fabric
        .as_ref()
        .map(|b| b.msg.window_secs as u64)
        .filter(|w| *w > 0)
        .unwrap_or(DEFAULT_WINDOW_SECS)
        .max(DEFAULT_WINDOW_SECS);
    let mut live: Vec<&Beat<ProducerFootprintV1>> = src
        .footprints
        .iter()
        .filter(|b| now_ms.saturating_sub(b.seen_ms) < fp_window_secs * STALE_FACTOR * 1000)
        .filter(|b| scope.allows_producer_row(&b.msg.peer, topology))
        .collect();
    live.sort_by(|a, b| {
        b.msg
            .sample_rate
            .partial_cmp(&a.msg.sample_rate)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.msg.peer.cmp(&b.msg.peer))
    });

    let mesh_env = src.mesh.as_ref().map(|b| &b.msg);
    let producers: Vec<ProducerRow> = live
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let fp = &b.msg;
            let (path, latency) = mesh_join(fp, aliases, mesh_env);
            let (band_label, band_color) =
                match envelopes.band(&fp.peer, fp.sample_rate, fp.byte_rate) {
                    Some(band) => (band.label().to_string(), band.color().to_string()),
                    None => ("—".to_string(), String::new()),
                };
            ProducerRow {
                pos: i + 1,
                anchor: fp.peer.clone(),
                display: aliases.render(&fp.peer),
                sample_rate: fp.sample_rate,
                byte_rate: fp.byte_rate,
                shares: shares_label(fp),
                near_limit: fp.near_limit,
                band_label,
                band_color,
                path,
                latency,
            }
        })
        .collect();

    // Identity-free aggregates (all tiers): link + presence counts.
    let (direct, relayed) = match mesh_env.and_then(|e| e.detail.as_ref()) {
        Some(EnvDetail::Mesh(d)) => (
            d.links.iter().filter(|l| l.connected && l.direct).count(),
            d.links.iter().filter(|l| l.connected && !l.direct).count(),
        ),
        _ => (0, 0),
    };
    let (alive, dropped) = match src.presence.as_ref().and_then(|b| b.msg.detail.as_ref()) {
        Some(EnvDetail::Presence(d)) => (d.alive_total, d.dropped_recent),
        _ => (0, 0),
    };

    let totals = Totals {
        visible: producers.len(),
        total_msgs: live.iter().map(|b| b.msg.sample_rate).sum(),
        total_bytes: live.iter().map(|b| b.msg.byte_rate).sum(),
        direct,
        relayed,
        alive,
        dropped,
    };

    TopView {
        scope_tier: scope_tier(scope),
        restricted: !matches!(scope, ViewerScope::Full | ViewerScope::DataAll),
        chips,
        classes,
        peer_sites,
        site: src.fabric.as_ref().map(|b| b.msg.site.clone()),
        fabric_active_producers: src.fabric.as_ref().map(|b| b.msg.active_producers),
        producers,
        totals,
        generated_at_ms: now_ms,
    }
}

/// Chip inputs for the peer domain, which has one beat per site: the worst
/// color among the fresh sites, n summed over them. When every site is
/// stale the newest stale beat drives the chip, so it collapses to idle with
/// that beat's age.
fn peer_chip_inputs(
    sites: &[Beat<EnvHealthV1>],
    stale: &dyn Fn(u64, u32) -> bool,
) -> (Option<String>, u64, u32, u64) {
    let fresh: Vec<&Beat<EnvHealthV1>> = sites
        .iter()
        .filter(|b| !stale(b.seen_ms, b.msg.window_secs))
        .collect();
    if fresh.is_empty() {
        return match sites.iter().max_by_key(|b| b.seen_ms) {
            None => (None, 0, 0, 0),
            Some(b) => (
                Some(peer_color(&b.msg)),
                b.msg.sample_count,
                b.msg.window_secs,
                b.seen_ms,
            ),
        };
    }
    let worst = fresh
        .iter()
        .max_by_key(|b| color_rank(&peer_color(&b.msg)))
        .expect("fresh is non-empty");
    (
        Some(peer_color(&worst.msg)),
        fresh.iter().map(|b| b.msg.sample_count).sum(),
        worst.msg.window_secs,
        worst.seen_ms,
    )
}

/// Severity order for picking the worst site: a grounded fault outranks a
/// grounded clean reading, and any grounded reading outranks idle/unmetered.
fn color_rank(color: &str) -> u8 {
    match color {
        "red" => 4,
        "amber" => 3,
        "green" => 2,
        "idle" => 1,
        _ => 0, // unmetered / unknown
    }
}

fn peer_detail(msg: &EnvHealthV1) -> Option<&PeerFaultDetailV1> {
    match &msg.detail {
        Some(EnvDetail::Peer(d)) => Some(d),
        _ => None,
    }
}

/// The reading's color, held to the honesty rule: a reading whose trust part
/// is not measured never shows green, whatever the envelope says.
fn peer_color(msg: &EnvHealthV1) -> String {
    let trust_unmeasured = peer_detail(msg).is_some_and(|d| d.trust_verdict == "unmeasured");
    if trust_unmeasured && msg.overall_color == "green" {
        "unmetered".to_string()
    } else {
        msg.overall_color.clone()
    }
}

/// The trust part of the reader's combined reason. The reader writes the
/// trust reason alone when its verdict is a trust word, and after "; also: "
/// when the clock verdict leads.
fn trust_reason(d: &PeerFaultDetailV1) -> String {
    if d.verdict.starts_with("trust_") {
        return d.verdict_reason.clone();
    }
    if let Some((_, tail)) = d.verdict_reason.split_once("; also: ") {
        return tail.to_string();
    }
    match d.trust_verdict.as_str() {
        "clean" => "no trust-refusal alert in the window".to_string(),
        // Unmeasured because the whole reading is: the clock reason says why.
        "unmeasured" => d.verdict_reason.clone(),
        _ => String::new(),
    }
}

fn trust_line(d: &PeerFaultDetailV1, is_stale: bool) -> Option<TrustLine> {
    if d.trust_verdict.is_empty() {
        return None;
    }
    let candidates = d
        .peers
        .iter()
        .filter(|p| p.kind == PEER_FAULT_TRUST_ROOT_REFUSED_CANDIDATE)
        .count();
    let (label, state) = match d.trust_verdict.as_str() {
        "clean" => ("no client refuses the router's certificate".to_string(), "green"),
        "unattributed" => (
            "a client refuses the router's certificate; the router's log cannot name it"
                .to_string(),
            "amber",
        ),
        "candidates" => (
            format!(
                "a client refuses the router's certificate; {candidates} declared peer(s) without a session are candidates, not confirmed"
            ),
            "amber",
        ),
        "unmeasured" => ("not measured".to_string(), "unmetered"),
        other => (format!("{other} (unknown trust verdict)"), "unmetered"),
    };
    Some(TrustLine {
        verdict: if is_stale {
            "stale".to_string()
        } else {
            d.trust_verdict.clone()
        },
        label,
        state: if is_stale { "idle" } else { state },
        reason: trust_reason(d),
        alerts: d
            .tls_alerts
            .iter()
            .map(|a| TlsAlertRow {
                name: a.name.clone(),
                count: a.count_in_window,
                per_minute: a.per_minute,
                trust_refusal: a.trust_refusal,
            })
            .collect(),
        declared_peers_absent: d.declared_peers_absent,
    })
}

fn peer_site_row(
    b: &Beat<EnvHealthV1>,
    show_peers: bool,
    is_stale: bool,
    now_ms: u64,
) -> PeerSiteRow {
    let detail = peer_detail(&b.msg);
    let peers: Vec<PeerRow> = detail
        .map(|d| d.peers.iter().map(|p| peer_row(p, now_ms)).collect())
        .unwrap_or_default();
    let withheld = if show_peers { 0 } else { peers.len() };
    let candidates_withheld = if show_peers {
        0
    } else {
        peers.iter().filter(|p| p.candidate).count()
    };
    PeerSiteRow {
        site: b.msg.scope.clone(),
        state: if is_stale {
            "idle".to_string()
        } else {
            peer_color(&b.msg)
        },
        verdict: if is_stale {
            "stale".to_string()
        } else {
            detail.map(|d| d.verdict.clone()).unwrap_or_default()
        },
        reason: detail.map(|d| d.verdict_reason.clone()).unwrap_or_default(),
        stale_age: is_stale.then(|| fmt_age(now_ms.saturating_sub(b.seen_ms) as i64)),
        peering: detail.map(|d| d.peering.clone()).unwrap_or_default(),
        peers: if show_peers { peers } else { Vec::new() },
        peers_withheld: withheld,
        candidates_withheld,
        trust: detail.and_then(|d| trust_line(d, is_stale)),
    }
}

fn peer_row(p: &PeerFaultV1, now_ms: u64) -> PeerRow {
    let candidate = p.kind == PEER_FAULT_TRUST_ROOT_REFUSED_CANDIDATE;
    let source_address = p.source_address.clone().filter(|a| !a.is_empty());
    let label = p
        .attribution_name
        .clone()
        .filter(|n| !n.is_empty())
        .or_else(|| (!p.zid.is_empty()).then(|| p.zid.clone()))
        .or_else(|| source_address.clone())
        .unwrap_or_else(|| "—".to_string());
    PeerRow {
        label,
        zid: p.zid.clone(),
        kind: match p.kind {
            PEER_FAULT_CLOCK_AHEAD => "clock_ahead",
            PEER_FAULT_TRUST_ROOT_REFUSED_CANDIDATE => "trust_candidate",
            _ => "unknown",
        },
        candidate,
        source_address,
        absent_since: p
            .absent_since_unix_ms
            .filter(|_| candidate)
            .map(|t| fmt_age(now_ms as i64 - t)),
        grade_label: if candidate {
            "candidate (not confirmed)".to_string()
        } else {
            p.grade.clone()
        },
        offset_seconds: p.offset_seconds,
        rejections_per_minute: p.rejections_per_minute,
        direct_session: match p.direct_session {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        },
        grade: p.grade.clone(),
        color: if candidate {
            "amber".to_string()
        } else {
            p.color.clone()
        },
    }
}

/// Compact per-class share string, e.g. "C2 80% · C0 20%" (top-3 by share).
/// Public alias for the who-view's footprint join.
pub fn shares_label_of(fp: &ProducerFootprintV1) -> String {
    shares_label(fp)
}

fn shares_label(fp: &ProducerFootprintV1) -> String {
    let total: f64 = fp.shares.iter().map(|s| s.sample_rate).sum();
    if total <= 0.0 || fp.shares.is_empty() {
        return "—".to_string();
    }
    let mut parts: Vec<(String, f64)> = fp
        .shares
        .iter()
        .map(|s| (s.class.clone(), s.sample_rate / total))
        .collect();
    parts.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    parts
        .iter()
        .take(3)
        .map(|(c, frac)| format!("{c} {:.0}%", frac * 100.0))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Best-effort join of a footprint to its mesh link (path + latency).
/// HONESTY (ADR 1044 §4.3): footprint `peer` is a slot anchor; a mesh link
/// keys on a netbird fqdn — different namespaces. Match only when the
/// alias or raw anchor equals the link's short name; otherwise "—".
/// A wrong join is worse than an honest blank.
fn mesh_join(
    fp: &ProducerFootprintV1,
    aliases: &AliasRegistry,
    mesh: Option<&EnvHealthV1>,
) -> (String, String) {
    let Some(env) = mesh else {
        return ("—".to_string(), "—".to_string());
    };
    let Some(EnvDetail::Mesh(detail)) = &env.detail else {
        return ("—".to_string(), "—".to_string());
    };
    let alias = aliases.alias_of(&fp.peer);
    let matched = detail.links.iter().find(|l| {
        let sn = short_name(&l.peer_fqdn);
        sn == fp.peer.as_str() || alias.as_deref() == Some(sn)
    });
    match matched {
        None => ("—".to_string(), "—".to_string()),
        Some(l) if !l.connected => ("offline".to_string(), "—".to_string()),
        Some(l) => {
            let path = if l.direct {
                "direct P2P".to_string()
            } else {
                format!("relayed {}", relay_label(&l.relay_address))
            };
            let latency = if l.latency_ms > 0.0 {
                format!("{:.0} ms", l.latency_ms)
            } else {
                "unmeasured".to_string()
            };
            (path, latency)
        }
    }
}

/// Relay short label: "via relay-dub" from "rels://relay-dub.example.net:33073".
fn relay_label(addr: &str) -> String {
    let host = addr
        .trim_start_matches("rels://")
        .trim_start_matches("rel://");
    match host.split(['.', ':']).next() {
        Some(h) if !h.is_empty() => format!("via {h}"),
        _ => "via relay".to_string(),
    }
}

/// Display-only class legend (wire stays C0..C3; unknown codes render bare).
fn class_label(class: &str) -> &'static str {
    match class {
        "C0" => "control",
        "C1" => "mission",
        "C2" => "situational",
        "C3" => "bulk",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health_wire::{
        ClassHealthV1, ClassShareV1, MeshDetailV1, MeshLinkHealthV1, PeerFaultDetailV1, TlsAlertV1,
    };
    use panoscope_core::portal::EnvelopeRow;
    use std::collections::HashSet;

    fn scoped(anchors: &[&str], include_notional: bool) -> ViewerScope {
        ViewerScope::Scoped {
            anchors: anchors
                .iter()
                .map(|s| s.to_string())
                .collect::<HashSet<_>>(),
            include_notional,
        }
    }

    fn fp(peer: &str, msgs: f64) -> Beat<ProducerFootprintV1> {
        Beat {
            msg: ProducerFootprintV1 {
                peer: peer.into(),
                ts_unix_ms: 0,
                sample_rate: msgs,
                byte_rate: msgs * 100.0,
                shares: vec![ClassShareV1 {
                    class: "C2".into(),
                    sample_rate: msgs,
                    byte_rate: msgs * 100.0,
                }],
                rank: 1,
                near_limit: false,
            },
            seen_ms: 10_000,
        }
    }

    fn fabric(window: u32, seen_ms: u64) -> Beat<FabricHealthV1> {
        Beat {
            msg: FabricHealthV1 {
                site: "site-a".into(),
                ts_unix_ms: 0,
                classes: vec![ClassHealthV1 {
                    class: "C2".into(),
                    sample_rate: 10.0,
                    byte_rate: 1000.0,
                    target_sample_rate: 100.0,
                    target_byte_rate: 0.0,
                    headroom_pct: 90.0,
                    color: "green".into(),
                    sample_count: 42,
                }],
                overall_color: "green".into(),
                active_producers: 7,
                window_secs: window,
            },
            seen_ms,
        }
    }

    fn empty_topology() -> Topology {
        Topology::default()
    }

    fn synth_topology(anchor: &str) -> Topology {
        let mut t = Topology::default();
        t.record_publish_declaration(anchor.to_string(), format!("{anchor}/t/v1"), 1);
        if let Some(p) = t.peers.get_mut(anchor) {
            p.synthetic = Some(true);
        }
        t
    }

    #[test]
    fn full_scope_sees_all_rows_and_fleet_totals() {
        let src = TopSources {
            fabric: Some(fabric(5, 10_000)),
            footprints: vec![fp("aaa111", 30.0), fp("bbb222", 10.0)],
            ..Default::default()
        };
        let v = build_top_view(
            &ViewerScope::Full,
            &src,
            &empty_topology(),
            &AliasRegistry::default(),
            &EnvelopeRegistry::empty(),
            11_000,
        );
        assert_eq!(v.scope_tier, "full");
        assert!(!v.restricted);
        assert_eq!(v.producers.len(), 2);
        // Sorted by msgs/s desc, 1-based positions.
        assert_eq!(v.producers[0].anchor, "aaa111");
        assert_eq!(v.producers[0].pos, 1);
        assert_eq!(v.totals.visible, 2);
        assert_eq!(v.fabric_active_producers, Some(7));
    }

    #[test]
    fn floor_scope_hides_producer_rows_but_keeps_aggregates() {
        let src = TopSources {
            fabric: Some(fabric(5, 10_000)),
            footprints: vec![fp("aaa111", 30.0)],
            ..Default::default()
        };
        let v = build_top_view(
            &scoped(&[], false),
            &src,
            &empty_topology(),
            &AliasRegistry::default(),
            &EnvelopeRegistry::empty(),
            11_000,
        );
        assert_eq!(v.scope_tier, "floor");
        assert!(v.restricted, "floor viewers must see the scoped-view note");
        assert!(
            v.producers.is_empty(),
            "app producer identities are scoped information"
        );
        // Identity-free aggregates stay: class rows + meter's own count.
        assert!(matches!(v.classes, ClassPane::Rows { .. }));
        assert_eq!(v.fabric_active_producers, Some(7));
        // Totals cover the visible (empty) set — they never imply the
        // hidden fleet.
        assert_eq!(v.totals.visible, 0);
        assert_eq!(v.totals.total_msgs, 0.0);
    }

    #[test]
    fn scoped_viewer_sees_granted_anchor_only() {
        let src = TopSources {
            fabric: Some(fabric(5, 10_000)),
            footprints: vec![fp("aaa111", 30.0), fp("bbb222", 10.0)],
            ..Default::default()
        };
        let v = build_top_view(
            &scoped(&["aaa111"], false),
            &src,
            &empty_topology(),
            &AliasRegistry::default(),
            &EnvelopeRegistry::empty(),
            11_000,
        );
        assert_eq!(v.scope_tier, "scoped");
        assert_eq!(v.producers.len(), 1);
        assert_eq!(v.producers[0].anchor, "aaa111");
        assert_eq!(v.totals.total_msgs, 30.0, "totals cover visible rows only");
    }

    #[test]
    fn notional_grant_reveals_synthetic_producer_rows() {
        let src = TopSources {
            fabric: Some(fabric(5, 10_000)),
            footprints: vec![fp("tarsier-alpha", 5.0), fp("real-org", 8.0)],
            ..Default::default()
        };
        let topo = synth_topology("tarsier-alpha");
        let with = build_top_view(
            &scoped(&[], true),
            &src,
            &topo,
            &AliasRegistry::default(),
            &EnvelopeRegistry::empty(),
            11_000,
        );
        assert_eq!(with.producers.len(), 1);
        assert_eq!(with.producers[0].anchor, "tarsier-alpha");
        let without = build_top_view(
            &scoped(&[], false),
            &src,
            &topo,
            &AliasRegistry::default(),
            &EnvelopeRegistry::empty(),
            11_000,
        );
        assert!(without.producers.is_empty());
    }

    #[test]
    fn stale_fabric_collapses_to_idle_and_hides_bars() {
        // Beat window 5s, seen at t=0, now t=20s → past 3× window.
        let src = TopSources {
            fabric: Some(fabric(5, 0)),
            ..Default::default()
        };
        let v = build_top_view(
            &ViewerScope::Full,
            &src,
            &empty_topology(),
            &AliasRegistry::default(),
            &EnvelopeRegistry::empty(),
            20_000,
        );
        let fab = v.chips.iter().find(|c| c.domain == "fabric").unwrap();
        assert_eq!(
            fab.state, "idle",
            "stale beat never re-asserts its live color"
        );
        assert!(fab.stale_age.is_some());
        assert!(
            matches!(v.classes, ClassPane::Stale { .. }),
            "bars hidden, not faked"
        );
    }

    #[test]
    fn absent_domain_reads_unmetered() {
        let v = build_top_view(
            &ViewerScope::Full,
            &TopSources::default(),
            &empty_topology(),
            &AliasRegistry::default(),
            &EnvelopeRegistry::empty(),
            1_000,
        );
        assert!(v
            .chips
            .iter()
            .all(|c| c.state == "unmetered" && c.sample_count == 0));
        assert!(matches!(v.classes, ClassPane::Absent));
    }

    #[test]
    fn expired_footprint_ages_out_of_the_table() {
        // window 5s → expiry 15s; footprint seen at t=10s, now t=26s.
        let src = TopSources {
            fabric: Some(fabric(5, 26_000)),
            footprints: vec![fp("aaa111", 30.0)],
            ..Default::default()
        };
        let v = build_top_view(
            &ViewerScope::Full,
            &src,
            &empty_topology(),
            &AliasRegistry::default(),
            &EnvelopeRegistry::empty(),
            26_000,
        );
        assert!(
            v.producers.is_empty(),
            "quiet producers drop, never false-live"
        );
    }

    #[test]
    fn no_envelope_renders_honest_dash_and_declared_envelope_bands() {
        let src = TopSources {
            fabric: Some(fabric(5, 10_000)),
            footprints: vec![fp("aaa111", 30.0)],
            ..Default::default()
        };
        // No envelope → "—".
        let v = build_top_view(
            &ViewerScope::Full,
            &src,
            &empty_topology(),
            &AliasRegistry::default(),
            &EnvelopeRegistry::empty(),
            11_000,
        );
        assert_eq!(v.producers[0].band_label, "—");
        assert_eq!(v.producers[0].band_color, "");
        // Declared envelope with max below the live rate → OVER/red.
        let envelopes = EnvelopeRegistry::from_rows([EnvelopeRow {
            anchor: "aaa111".into(),
            min_msgs_per_s: 0.0,
            mean_msgs_per_s: 5.0,
            high_msgs_per_s: 10.0,
            max_msgs_per_s: 20.0,
            min_bytes_per_s: 0.0,
            mean_bytes_per_s: 0.0,
            high_bytes_per_s: 0.0,
            max_bytes_per_s: 0.0,
            source: String::new(),
        }]);
        let v = build_top_view(
            &ViewerScope::Full,
            &src,
            &empty_topology(),
            &AliasRegistry::default(),
            &envelopes,
            11_000,
        );
        assert_eq!(v.producers[0].band_label, "OVER");
        assert_eq!(v.producers[0].band_color, "red");
    }

    fn peer_beat(site: &str, color: &str, seen_ms: u64, peering: &str) -> Beat<EnvHealthV1> {
        Beat {
            msg: EnvHealthV1 {
                domain: "peer".into(),
                scope: site.into(),
                ts_unix_ms: 0,
                overall_color: color.into(),
                sample_count: 2,
                window_secs: 30,
                detail: Some(EnvDetail::Peer(PeerFaultDetailV1 {
                    peers: vec![
                        PeerFaultV1 {
                            zid: "3c9a51e0d2b7f4a6c18e05d9b3a7f2e1".into(),
                            offset_seconds: 17.7,
                            rejections_per_minute: 12.0,
                            attribution_name: Some("lab-router-b".into()),
                            direct_session: Some(false),
                            grade: "peer_at_fault".into(),
                            color: "red".into(),
                            ..Default::default()
                        },
                        PeerFaultV1 {
                            zid: "9f2".into(),
                            offset_seconds: 3.0,
                            rejections_per_minute: 2.0,
                            grade: "peer_at_fault".into(),
                            color: "red".into(),
                            ..Default::default()
                        },
                    ],
                    verdict: "peer_at_fault".into(),
                    verdict_reason: "2 peer(s) had timestamps rejected in the last 60 s".into(),
                    peering: peering.into(),
                    ..Default::default()
                })),
            },
            seen_ms,
        }
    }

    fn view(scope: &ViewerScope, src: &TopSources, now_ms: u64) -> TopView {
        build_top_view(
            scope,
            src,
            &empty_topology(),
            &AliasRegistry::default(),
            &EnvelopeRegistry::empty(),
            now_ms,
        )
    }

    #[test]
    fn peer_domain_renders_a_site_row_with_peer_rows() {
        let src = TopSources {
            peer: vec![peer_beat("example-site", "red", 10_000, "partner-one")],
            ..Default::default()
        };
        let v = view(&ViewerScope::Full, &src, 11_000);
        let chip = v.chips.iter().find(|c| c.domain == "peer").unwrap();
        assert_eq!(chip.state, "red");
        assert_eq!(chip.sample_count, 2);
        assert_eq!(v.peer_sites.len(), 1);
        let s = &v.peer_sites[0];
        assert_eq!(s.site, "example-site");
        assert_eq!(s.state, "red");
        assert_eq!(s.verdict, "peer_at_fault");
        assert_eq!(s.peering, "partner-one");
        assert_eq!(s.peers_withheld, 0);
        // Attributed name when known, Zenoh ID otherwise.
        assert_eq!(s.peers[0].label, "lab-router-b");
        assert_eq!(s.peers[0].offset_seconds, 17.7);
        assert_eq!(s.peers[0].rejections_per_minute, 12.0);
        assert_eq!(s.peers[0].direct_session, "no");
        assert_eq!(s.peers[1].label, "9f2");
        assert_eq!(s.peers[1].direct_session, "unknown");
    }

    #[test]
    fn peer_identities_are_withheld_below_full_scope() {
        let src = TopSources {
            peer: vec![peer_beat("example-site", "red", 10_000, "")],
            ..Default::default()
        };
        for scope in [
            ViewerScope::DataAll,
            scoped(&[], false),
            scoped(&["aaa111"], true),
        ] {
            let v = view(&scope, &src, 11_000);
            let s = &v.peer_sites[0];
            assert!(s.peers.is_empty(), "no Zenoh ID or owner name below Full");
            assert_eq!(s.peers_withheld, 2);
            // The identity-free grade stays visible.
            assert_eq!(s.state, "red");
            assert_eq!(
                v.chips.iter().find(|c| c.domain == "peer").unwrap().state,
                "red"
            );
        }
    }

    #[test]
    fn peer_chip_is_the_worst_fresh_site_and_stale_sites_collapse() {
        let src = TopSources {
            peer: vec![
                peer_beat("site-a", "green", 100_000, ""),
                peer_beat("site-b", "amber", 100_000, ""),
                // Stale (window 30 s, seen 100 s before now): its red must
                // not drive the chip, and its own row reads idle.
                peer_beat("site-c", "red", 10_000, ""),
            ],
            ..Default::default()
        };
        let v = view(&ViewerScope::Full, &src, 110_000);
        let chip = v.chips.iter().find(|c| c.domain == "peer").unwrap();
        assert_eq!(chip.state, "amber");
        assert_eq!(chip.sample_count, 4, "n over the fresh sites");
        let c = v.peer_sites.iter().find(|s| s.site == "site-c").unwrap();
        assert_eq!(c.state, "idle");
        assert_eq!(c.verdict, "stale");
        assert!(c.stale_age.is_some());

        // Every site stale: the chip collapses to idle with an age.
        let src = TopSources {
            peer: vec![peer_beat("site-c", "red", 10_000, "")],
            ..Default::default()
        };
        let v = view(&ViewerScope::Full, &src, 110_000);
        let chip = v.chips.iter().find(|c| c.domain == "peer").unwrap();
        assert_eq!(chip.state, "idle");
        assert!(chip.stale_age.is_some());
    }

    #[test]
    fn peer_beats_decode_from_the_wire() {
        let env = peer_beat("example-site", "red", 0, "").msg;
        let back = EnvHealthV1::decode(env.encode_to_vec().as_slice()).unwrap();
        match back.detail {
            Some(EnvDetail::Peer(d)) => {
                assert_eq!(d.peers.len(), 2);
                assert_eq!(d.peers[0].direct_session, Some(false));
                assert_eq!(d.peers[1].direct_session, None);
            }
            other => panic!("expected the peer arm, got {other:?}"),
        }
    }

    #[test]
    fn mesh_join_never_guesses() {
        let mesh = EnvHealthV1 {
            domain: "mesh".into(),
            scope: "site-a".into(),
            ts_unix_ms: 0,
            overall_color: "green".into(),
            sample_count: 1,
            window_secs: 30,
            detail: Some(EnvDetail::Mesh(MeshDetailV1 {
                links: vec![MeshLinkHealthV1 {
                    peer_fqdn: "thor.site.example.net".into(),
                    peer_ip: "100.127.0.9".into(),
                    color: "green".into(),
                    connected: true,
                    direct: true,
                    conn_type: "P2P".into(),
                    latency_ms: 12.0,
                    relay_address: String::new(),
                    handshake_age_ms: 0,
                    transition_count: 0,
                }],
                vantage_host: "thor.site.example.net".into(),
                peers_total: 1,
                peers_connected: 1,
            })),
        };
        // Anchor matches the link short-name → joined.
        let (path, latency) =
            mesh_join(&fp("thor", 1.0).msg, &AliasRegistry::default(), Some(&mesh));
        assert_eq!(path, "direct P2P");
        assert_eq!(latency, "12 ms");
        // Different namespace (UUID anchor) → honest blank, never a guess.
        let (path, latency) = mesh_join(
            &fp("2cf6aaaa", 1.0).msg,
            &AliasRegistry::default(),
            Some(&mesh),
        );
        assert_eq!(path, "—");
        assert_eq!(latency, "—");
    }

    // ---- the trust-refusal reading ---------------------------------------

    const T_NOW: u64 = 1_794_674_760_000;

    fn alert(name: &str, count: u64, trust_refusal: bool) -> TlsAlertV1 {
        TlsAlertV1 {
            name: name.into(),
            count_in_window: count,
            per_minute: count as f64,
            trust_refusal,
            last_seen_unix_ms: 0,
        }
    }

    fn candidate(zid: &str, addr: Option<&str>, name: Option<&str>) -> PeerFaultV1 {
        PeerFaultV1 {
            zid: zid.into(),
            kind: PEER_FAULT_TRUST_ROOT_REFUSED_CANDIDATE,
            source_address: addr.map(Into::into),
            attribution_name: name.map(Into::into),
            direct_session: Some(false),
            absent_since_unix_ms: Some(T_NOW as i64 - 240_000),
            grade: "candidate".into(),
            // A reader that sent red must still render amber: never red.
            color: "red".into(),
            ..Default::default()
        }
    }

    fn trust_beat(
        color: &str,
        verdict: &str,
        reason: &str,
        trust_verdict: &str,
        peers: Vec<PeerFaultV1>,
        tls_alerts: Vec<TlsAlertV1>,
    ) -> Beat<EnvHealthV1> {
        Beat {
            msg: EnvHealthV1 {
                domain: "peer".into(),
                scope: "example-site".into(),
                ts_unix_ms: 0,
                overall_color: color.into(),
                sample_count: 3,
                window_secs: 30,
                detail: Some(EnvDetail::Peer(PeerFaultDetailV1 {
                    peers,
                    verdict: verdict.into(),
                    verdict_reason: reason.into(),
                    tls_alerts,
                    trust_verdict: trust_verdict.into(),
                    declared_peers_absent: Some(2),
                    ..Default::default()
                })),
            },
            seen_ms: T_NOW - 1_000,
        }
    }

    fn candidates_beat() -> Beat<EnvHealthV1> {
        trust_beat(
            "red",
            "peer_at_fault",
            "1 peer(s) had timestamps rejected in the last 60 s; also: 6 TLS handshake(s) ended in the last 60 s with a trust-refusal alert (UnknownCA x5, BadCertificate x1); 2 declared peer(s) hold no session and are candidates, not confirmed: the router's log cannot name the refusing client",
            "candidates",
            vec![
                PeerFaultV1 {
                    zid: "3c9a51e0d2b7f4a6c18e05d9b3a7f2e1".into(),
                    kind: PEER_FAULT_CLOCK_AHEAD,
                    offset_seconds: 17.7,
                    rejections_per_minute: 3.0,
                    grade: "peer_at_fault".into(),
                    color: "red".into(),
                    ..Default::default()
                },
                candidate("", Some("10.0.0.7"), None),
                candidate("7e41c0d9a2b4", None, Some("lab-router-d")),
            ],
            vec![
                alert("BadCertificate", 1, true),
                alert("CertificateExpired", 1, false),
                alert("UnknownCA", 5, true),
            ],
        )
    }

    #[test]
    fn candidates_render_as_amber_candidates_at_full_scope() {
        let src = TopSources {
            peer: vec![candidates_beat()],
            ..Default::default()
        };
        let v = view(&ViewerScope::Full, &src, T_NOW);
        let s = &v.peer_sites[0];
        assert_eq!(s.state, "red", "the clock fault leads the site");
        assert_eq!(s.peers.len(), 3);

        let clock = &s.peers[0];
        assert_eq!(clock.kind, "clock_ahead");
        assert!(!clock.candidate);
        assert_eq!(clock.grade_label, "peer_at_fault");
        assert_eq!(clock.color, "red");

        // Declared by address: no Zenoh ID, the address is the label.
        let by_addr = &s.peers[1];
        assert_eq!(by_addr.kind, "trust_candidate");
        assert!(by_addr.candidate);
        assert_eq!(by_addr.label, "10.0.0.7");
        assert_eq!(by_addr.zid, "");
        assert_eq!(by_addr.source_address.as_deref(), Some("10.0.0.7"));
        assert_eq!(by_addr.grade_label, "candidate (not confirmed)");
        assert_eq!(by_addr.color, "amber", "a candidate is never red");
        assert_eq!(by_addr.absent_since.as_deref(), Some("4m ago"));
        assert_eq!(by_addr.direct_session, "no");

        let named = &s.peers[2];
        assert_eq!(named.label, "lab-router-d");
        assert_eq!(named.source_address, None);

        let t = s.trust.as_ref().expect("a trust line");
        assert_eq!(t.verdict, "candidates");
        assert_eq!(t.state, "amber");
        assert_eq!(
            t.label,
            "a client refuses the router's certificate; 2 declared peer(s) without a session are candidates, not confirmed"
        );
        assert!(t.reason.starts_with("6 TLS handshake(s)"), "{}", t.reason);
        assert!(
            !t.reason.contains("timestamps rejected"),
            "the clock part is not repeated"
        );
        let names: Vec<(&str, u64, bool)> = t
            .alerts
            .iter()
            .map(|a| (a.name.as_str(), a.count, a.trust_refusal))
            .collect();
        assert_eq!(
            names,
            [
                ("BadCertificate", 1, true),
                ("CertificateExpired", 1, false),
                ("UnknownCA", 5, true),
            ]
        );
        assert_eq!(t.declared_peers_absent, Some(2));
    }

    #[test]
    fn candidates_are_withheld_below_full_scope_and_the_trust_line_is_not() {
        let src = TopSources {
            peer: vec![candidates_beat()],
            ..Default::default()
        };
        for scope in [
            ViewerScope::DataAll,
            scoped(&[], false),
            scoped(&["aaa111"], true),
        ] {
            let v = view(&scope, &src, T_NOW);
            let s = &v.peer_sites[0];
            assert!(
                s.peers.is_empty(),
                "no Zenoh ID or source address below Full"
            );
            assert_eq!(s.peers_withheld, 3);
            assert_eq!(s.candidates_withheld, 2);
            let json = serde_json::to_string(s).unwrap();
            assert!(!json.contains("10.0.0.7"), "{json}");
            assert!(!json.contains("lab-router-d"), "{json}");
            // The trust line is a fact about the router: every tier sees it.
            let t = s.trust.as_ref().unwrap();
            assert_eq!(t.verdict, "candidates");
            assert_eq!(t.alerts.len(), 3);
        }
    }

    #[test]
    fn an_unattributed_refusal_reads_amber_with_the_readers_reason() {
        let reason = "2 TLS handshake(s) ended in the last 60 s with a trust-refusal alert (UnknownCA x2): a client is refusing the router's certificate and cannot be named from the router's log; no peers file declares the expected peers";
        let src = TopSources {
            peer: vec![trust_beat(
                "amber",
                "trust_refused_unattributed",
                reason,
                "unattributed",
                vec![],
                vec![alert("UnknownCA", 2, true)],
            )],
            ..Default::default()
        };
        let v = view(&ViewerScope::Full, &src, T_NOW);
        let s = &v.peer_sites[0];
        assert_eq!(s.state, "amber");
        let t = s.trust.as_ref().unwrap();
        assert_eq!(t.state, "amber");
        assert_eq!(
            t.label,
            "a client refuses the router's certificate; the router's log cannot name it"
        );
        assert_eq!(t.reason, reason);
        assert!(s.peers.is_empty());
    }

    #[test]
    fn trust_not_measured_is_said_so_and_never_green() {
        // A reader that (wrongly) sent green with an unmeasured trust part:
        // the pane still refuses to show green.
        for overall in ["green", "unmetered"] {
            let src = TopSources {
                peer: vec![trust_beat(
                    overall,
                    "trust_unmeasured",
                    "trust refusal is not measured: no TLS fatal-alert line has been read and the router is not declared to log one (PEER_HEALTH_TLS_ALERTS_LOGGED)",
                    "unmeasured",
                    vec![],
                    vec![],
                )],
                ..Default::default()
            };
            let v = view(&ViewerScope::DataAll, &src, T_NOW);
            let s = &v.peer_sites[0];
            assert_eq!(s.state, "unmetered", "from {overall}");
            let t = s.trust.as_ref().unwrap();
            assert_eq!(t.label, "not measured");
            assert_eq!(t.state, "unmetered");
            assert!(t.reason.contains("not measured"));
            let chip = v.chips.iter().find(|c| c.domain == "peer").unwrap();
            assert_eq!(chip.state, "unmetered");
        }

        // A clean trust reading is green, and says what it means.
        let src = TopSources {
            peer: vec![trust_beat(
                "green",
                "clean",
                "no timestamp rejected",
                "clean",
                vec![],
                vec![],
            )],
            ..Default::default()
        };
        let v = view(&ViewerScope::Full, &src, T_NOW);
        let t = v.peer_sites[0].trust.as_ref().unwrap();
        assert_eq!(t.state, "green");
        assert_eq!(t.label, "no client refuses the router's certificate");
        assert_eq!(t.reason, "no trust-refusal alert in the window");
    }

    #[test]
    fn a_stale_trust_line_is_idle_and_an_old_reader_has_none() {
        let mut b = candidates_beat();
        b.seen_ms = T_NOW - 100_000; // window 30 s: stale
        let src = TopSources {
            peer: vec![b],
            ..Default::default()
        };
        let v = view(&ViewerScope::Full, &src, T_NOW);
        let t = v.peer_sites[0].trust.as_ref().unwrap();
        assert_eq!(t.verdict, "stale");
        assert_eq!(t.state, "idle");

        // A reader that predates the trust detector: no trust line at all.
        let src = TopSources {
            peer: vec![peer_beat("example-site", "red", 10_000, "")],
            ..Default::default()
        };
        let v = view(&ViewerScope::Full, &src, 11_000);
        assert!(v.peer_sites[0].trust.is_none());
        assert!(v.peer_sites[0].peers.iter().all(|p| !p.candidate));
    }

    #[test]
    fn trust_fields_decode_from_the_wire() {
        let env = candidates_beat().msg;
        let back = EnvHealthV1::decode(env.encode_to_vec().as_slice()).unwrap();
        let Some(EnvDetail::Peer(d)) = back.detail else {
            panic!("expected the peer arm")
        };
        assert_eq!(d.trust_verdict, "candidates");
        assert_eq!(d.declared_peers_absent, Some(2));
        assert_eq!(d.tls_alerts.len(), 3);
        assert_eq!(d.tls_alerts[2].name, "UnknownCA");
        assert!(!d.tls_alerts[1].trust_refusal);
        assert_eq!(d.peers[1].kind, PEER_FAULT_TRUST_ROOT_REFUSED_CANDIDATE);
        assert_eq!(d.peers[1].source_address.as_deref(), Some("10.0.0.7"));
        assert_eq!(
            d.peers[1].absent_since_unix_ms,
            Some(T_NOW as i64 - 240_000)
        );
        assert_eq!(d.peers[0].source_address, None);
    }
}
