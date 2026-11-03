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

use serde::Serialize;

use crate::health_wire::{
    fmt_age, short_name, EnvDetail, EnvHealthV1, FabricHealthV1, ProducerFootprintV1,
    FOOTPRINT_PREFIX, HEALTH_PREFIX, MESH_HEALTH_PREFIX, PRESENCE_HEALTH_PREFIX,
    TIME_HEALTH_PREFIX,
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
/// The four env-health domains in canonical strip order.
const DOMAINS: [&str; 4] = ["mesh", "fabric", "time", "presence"];

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
                src.fabric = Some(Beat { msg: h, seen_ms: e.ts_ms });
            }
        }
    }
    for e in buf.latest_by_prefix(FOOTPRINT_PREFIX) {
        let Some(raw) = &e.raw_bytes else { continue };
        if let Ok(fp) = ProducerFootprintV1::decode(raw.as_slice()) {
            if !fp.peer.is_empty() {
                src.footprints.push(Beat { msg: fp, seen_ms: e.ts_ms });
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
                    *slot = Some(Beat { msg: env, seen_ms: e.ts_ms });
                }
            }
        }
    }
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
                        stale_age: is_stale
                            .then(|| fmt_age(now_ms.saturating_sub(seen) as i64)),
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
        site: src.fabric.as_ref().map(|b| b.msg.site.clone()),
        fabric_active_producers: src.fabric.as_ref().map(|b| b.msg.active_producers),
        producers,
        totals,
        generated_at_ms: now_ms,
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
    let host = addr.trim_start_matches("rels://").trim_start_matches("rel://");
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
    use crate::health_wire::{ClassHealthV1, ClassShareV1, MeshDetailV1, MeshLinkHealthV1};
    use panoscope_core::portal::EnvelopeRow;
    use std::collections::HashSet;

    fn scoped(anchors: &[&str], include_notional: bool) -> ViewerScope {
        ViewerScope::Scoped {
            anchors: anchors.iter().map(|s| s.to_string()).collect::<HashSet<_>>(),
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
        assert!(v.producers.is_empty(), "app producer identities are scoped information");
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
        let src = TopSources { fabric: Some(fabric(5, 0)), ..Default::default() };
        let v = build_top_view(
            &ViewerScope::Full,
            &src,
            &empty_topology(),
            &AliasRegistry::default(),
            &EnvelopeRegistry::empty(),
            20_000,
        );
        let fab = v.chips.iter().find(|c| c.domain == "fabric").unwrap();
        assert_eq!(fab.state, "idle", "stale beat never re-asserts its live color");
        assert!(fab.stale_age.is_some());
        assert!(matches!(v.classes, ClassPane::Stale { .. }), "bars hidden, not faked");
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
        assert!(v.chips.iter().all(|c| c.state == "unmetered" && c.sample_count == 0));
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
        assert!(v.producers.is_empty(), "quiet producers drop, never false-live");
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
        let (path, latency) = mesh_join(&fp("thor", 1.0).msg, &AliasRegistry::default(), Some(&mesh));
        assert_eq!(path, "direct P2P");
        assert_eq!(latency, "12 ms");
        // Different namespace (UUID anchor) → honest blank, never a guess.
        let (path, latency) =
            mesh_join(&fp("2cf6aaaa", 1.0).msg, &AliasRegistry::default(), Some(&mesh));
        assert_eq!(path, "—");
        assert_eq!(latency, "—");
    }
}
