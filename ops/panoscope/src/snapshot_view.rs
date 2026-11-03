// SPDX-License-Identifier: Apache-2.0
//
// Snapshot view types — wire shape for both GET /api/snapshot and the
// initial-message of the /ws connection. Kept here so the HTTP layer
// and the WebSocket layer share one source of truth for the JSON shape
// the browser frontend parses.

use serde::Serialize;

use crate::model::{EdgeState, FlowState, NodeClass, NodeKind, PeerId, PeerInfo, Role, Topology};
#[cfg(test)]
use crate::sample_buffer::SampleBuffer;
use crate::sample_buffer::SampleFormat;
use crate::zenoh_client::EMA_TAU_SECS;

#[derive(Serialize, Clone)]
pub struct NodeView {
    pub peer_id: PeerId,
    /// Slot alias (ADR 1039) — the operator-assigned human name for this
    /// node's slot anchor, resolved from the portal registry when the
    /// `peer_id` is a 32-hex anchor with an assigned alias (e.g.
    /// `pat-operator`, `tarsier-alpha`). `None` when no alias applies (a
    /// random ZID, an unknown anchor, or an empty/degraded registry), so the
    /// frontend's existing `display_name`/truncated-UUID fallback is reached
    /// unchanged. Set in `build_nodes_view`; `From<&PeerInfo>` leaves it
    /// `None`. The raw UUID stays the canonical `peer_id` — this is a
    /// display-only projection, never a machine key (§3 invariant).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_alias: Option<String>,
    /// Human label for the node (e.g. "tarsier-alpha"). Carried in the
    /// schema-decl JSON `display_name` field; falls through as `None`
    /// when absent so the frontend can fall back to `peer_id`/`vendor`.
    /// Distinct from `vendor` because the cert-bound vendor is uniform
    /// after the notional namespace collapse — without
    /// display_name the side panel labels every node "notional".
    pub display_name: Option<String>,
    pub vendor: Option<String>,
    pub role: Role,
    pub publishes: Vec<String>,
    pub subscribes: Vec<String>,
    pub first_seen_ms: u64,
    pub last_seen_ms: u64,
    pub site: Option<String>,
    pub role_class: Option<String>,
    pub format_hint: Option<String>,
    pub synthetic: Option<bool>,
    /// Substrate-level node kind (router / peer / subscriber) — set when
    /// an admin-namespace sample confirmed this ZID. `None` for nodes
    /// learned only from schema-decl or topic-prefix fallback.
    pub node_kind: Option<NodeKind>,
    /// Provenance: `"schema_decl"` / `"admin"` / `"data_tail"`.
    pub attest_source: Option<String>,
    /// Transport locator extracted from admin payload.
    pub locator: Option<String>,
    /// Participant class: `"infra"` (underlying infrastructure — router,
    /// observer subscriber, schema-registry / operator-system publisher,
    /// chaos generator, or self-declared) vs `"app"` (application data
    /// participant). The single axis the frontend badges + filters on, so
    /// "is this substrate?" is decided once here (see
    /// `PeerInfo::node_class`) rather than recomputed per glyph.
    pub node_class: NodeClass,
    /// Infrastructure sub-type key when `node_class == "infra"` (`router`
    /// / `subscriber` / `chaos` / `schema-registry` / `operator-system` /
    /// `infra`), else `None`. The frontend maps it to a human label +
    /// "what this is, not a data participant" explainer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_subkind: Option<&'static str>,
    /// True when this peer subscribes to the universal `**` wildcard — a
    /// "watches everything" observer (omniwatch, audit-log, archive-vault,
    /// the fabric-health meter, an operator console, …). The renderer
    /// badges it and, crucially, does NOT draw a per-topic fan-out edge
    /// from every topic to it: with N such observers on a fabric of M
    /// topics that is N×M edges of pure noise (an edge to a `**` observer
    /// carries no per-topic information — it watches all by definition),
    /// which turns the COP into a near-complete-bipartite hairball. The
    /// edge-suppression itself happens in `build_raw_edges` (these peers
    /// are excluded from `matching_subs`); this flag is just the marker so
    /// the node still renders, badged, as a universal observer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub universal_observer: bool,
    /// Compact pub/sub-hygiene verdict for this publisher (key-shape only —
    /// the cheap, snapshot-path half). `None` for non-publishers. The full
    /// report (finding messages + fixes + the live storage-`get`
    /// retrievability probe) is fetched lazily from
    /// `/api/publisher/hygiene?source=<peer>`. See
    /// docs/design/pubsub-hygiene-diagnostic.md.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hygiene: Option<NodeHygiene>,
}

/// The snapshot-embedded hygiene summary — just enough for the side panel to
/// badge a publisher without a round-trip. `codes` are the stable finding
/// codes (e.g. `version_after_entity`); the prose + fixes + retrievability come
/// from the lazy `/api/publisher/hygiene` endpoint.
#[derive(Serialize, Clone, Debug)]
pub struct NodeHygiene {
    pub verdict: panoscope_core::Verdict,
    pub codes: Vec<String>,
}

/// True when a peer's subscription set includes the universal `**`
/// wildcard — i.e. it consumes every topic on the fabric. Such peers are
/// rendered as badged "universal observers" with no per-topic fan-out
/// edges (see `NodeView.universal_observer` + `build_raw_edges`).
fn subscribes_universally(p: &PeerInfo) -> bool {
    p.subscribes.iter().any(|s| s.as_str() == "**")
}

impl From<&PeerInfo> for NodeView {
    fn from(p: &PeerInfo) -> Self {
        let publishes_raw: Vec<String> = p.publishes.iter().cloned().collect();
        // Server-side singleton fold to bound the wire payload — keeps
        // /api/snapshot in the kilobytes even when a peer publishes
        // thousands of UUID-per-entity topics under one namespace.
        // Pairs with fold_singletons_in_edges in this file. See doc
        // comment on EdgeView.folded_singletons_count.
        let publishes = fold_publishes(&publishes_raw);
        NodeView::with_publishes(p, publishes)
    }
}

impl NodeView {
    /// A node whose `publishes` list the caller supplies. The view model
    /// passes an empty list: it never sends this field (each viewer's peer
    /// output lists that viewer's drawn units), and folding a 9,000-topic
    /// publish set here would dominate the model build on a large fabric.
    pub fn with_publishes(p: &PeerInfo, publishes: Vec<String>) -> Self {
        let mut subscribes: Vec<String> = p.subscribes.iter().cloned().collect();
        subscribes.sort();
        NodeView {
            peer_id: p.peer_id.clone(),
            // Resolved in build_nodes_view, which has the AliasRegistry; the
            // bare conversion has no registry, so it stays None here.
            slot_alias: None,
            display_name: p.display_name.clone(),
            vendor: p.vendor.clone(),
            role: p.role(),
            publishes,
            subscribes,
            first_seen_ms: p.first_seen_ms,
            last_seen_ms: p.last_seen_ms,
            site: p.site.clone(),
            role_class: p.role_class.clone(),
            format_hint: p.format_hint.clone(),
            synthetic: p.synthetic,
            node_kind: p.node_kind,
            attest_source: p.attest_source.clone(),
            locator: p.locator.clone(),
            node_class: p.node_class(),
            node_subkind: p.node_subkind(),
            universal_observer: subscribes_universally(p),
            // Resolved in shared_snapshot_build (needs the raw edges for
            // per-topic event counts + schema presence); the bare conversion
            // leaves it None.
            hygiene: None,
        }
    }
}

#[derive(Serialize, Clone)]
pub struct EdgeView {
    pub source: PeerId,
    pub sink: Option<PeerId>,
    pub topic: String,
    pub ema_eps: f64,
    pub projected_eps: f64,
    pub total_events: u64,
    pub last_event_ms: u64,
    pub schema_family: Option<String>,
    /// Latest classified wire format for this topic — derived from the
    /// most-recent buffered sample. `None` until the first sample lands.
    /// Stable values: "json" / "cbor" / "protobuf" / "text" / "hex" /
    /// "empty" (matches SampleFormat serde).
    pub format: Option<String>,
    /// When set, this edge is the server-side fold of N singleton edges
    /// (each had total_events == 1) under the same (publisher, namespace).
    /// Bounds the wire payload — without this fold, fanout publishers
    /// with thousands of UUID-per-entity topics would each emit N edges, blowing the /api/snapshot payload past
    /// 6 MB and timing out the WS initial-send. The `topic` field on a
    /// folded edge is the namespace plus a marker ("<ns>/* (folded xN)"),
    /// so the frontend can recognize the fold and the operator sees the
    /// shape directly. `None` for regular per-topic edges. F-NEW
    /// (server-side fanout fold) — pairs with the existing client-side
    /// fold from PR #573 (which still runs as defense-in-depth for
    /// groups under the fold threshold).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folded_singletons_count: Option<u64>,
    /// When set with `folded_singletons_count`, names the namespace the
    /// fold collapsed (e.g. `"sensorgrid/v1/entities"`). The frontend uses
    /// this to construct the `/api/snapshot/expand?source=&namespace=`
    /// URL when the operator clicks to explode the synthetic super-edge
    /// — restoring the PR #573 interaction model (peek / explode / Alt+
    /// click drill-in) on top of the wire-bounded server fold. Without
    /// this field the frontend would have to parse it back out of the
    /// synthetic topic string (`"<ns>/* (folded xN)"`), which is brittle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folded_namespace: Option<String>,
}

impl EdgeView {
    pub fn from_state(e: &EdgeState, now_ms: u64, format: Option<String>) -> Self {
        EdgeView {
            source: e.source.clone(),
            sink: e.sink.clone(),
            topic: e.topic.clone(),
            ema_eps: e.ema_eps,
            projected_eps: e.projected_eps(now_ms, EMA_TAU_SECS),
            total_events: e.total_events,
            last_event_ms: e.last_event_ms,
            schema_family: e.schema_family.clone(),
            format,
            folded_singletons_count: None,
            folded_namespace: None,
        }
    }
}

/// Threshold above which singleton edges under the same (publisher,
/// namespace) get folded into one synthetic edge. Mirrors the constant
/// in the frontend `render.ts` singleton-fanout precomputation
/// (`SINGLETON_FANOUT_THRESHOLD`) so the two folds are semantically
/// identical — the server fold runs first to bound the wire payload;
/// the client fold remains for groups under threshold (defense-in-depth
/// for any future un-folded-but-large case, plus the manual-collapse
/// Alt+click path that doesn't care about counts).
const SINGLETON_FANOUT_THRESHOLD: usize = 7;

/// Derive a fold *template* for every topic in one publisher's topic set.
///
/// This generalizes `topic_namespace` (which only ever wildcarded the LAST
/// segment, i.e. grouped `ns/<leaf>` by stripping `<leaf>`). That worked for
/// the canonical per-entity / per-event patterns where the high-cardinality id
/// is the trailing segment (`vendor/system/v1/<entity-id>`). It is BLIND to a
/// high-cardinality id in a *non-terminal* segment — e.g. a publisher emitting
/// `<anchor>/aircraft/<entity-id>/v1` (version-after-entity). There the last
/// segment is the constant `v1`, so the old grouping keyed every topic to a
/// distinct namespace (`<anchor>/aircraft/<entity-id>`) and nothing ever
/// folded: hundreds of single-topic nodes sprayed across the COP.
///
/// The fix: find the varying segment wherever it sits. Topics are bucketed by
/// segment count; within a bucket the position with the most distinct values is
/// the "entity axis" and gets replaced with `*`. The resulting template (e.g.
/// `<anchor>/aircraft/*/v1`) is the group key. Non-varying segments stay
/// literal, so distinct sub-families under the same publisher+count
/// (`a/aircraft/*/v1` vs `a/sensors/*/v1`) keep separate templates. A topic
/// whose bucket has a single member, or whose entity axis doesn't actually vary,
/// maps to itself (it can never reach the fold threshold alone).
///
/// Multi-axis (v2): a bucket is folded GREEDILY — repeatedly pick the single
/// wildcard position whose templated group is largest and over threshold, claim
/// those topics, and repeat on what's left. So a publisher with two distinct
/// fan-out families in the same segment-count bucket varying at DIFFERENT
/// positions folds BOTH: e.g. `<a>/air/purpleair/<id>/v1` (×82, pos 3) and
/// `<a>/weather/<city>/forecast/v1` (×10, pos 2) each collapse to their own
/// template instead of the bigger family monopolizing the bucket's only axis
/// (a single-axis fold leaves the smaller family sprayed across the COP).
///
/// Ties resolve to the LATER position then the lexicographically-smaller
/// template, so a pure trailing-id pattern still derives `ns/<const>/*` —
/// identical to the old `topic_namespace` behaviour plus the explicit `/*`,
/// preserving back-compat. Topics left after no group clears the threshold map
/// to themselves (genuinely-small families stay unfolded, by design).
pub fn fold_group_templates(topics: &[&str]) -> std::collections::HashMap<String, String> {
    use std::collections::HashMap;

    let mut buckets: HashMap<usize, Vec<&str>> = HashMap::new();
    for t in topics {
        buckets.entry(t.split('/').count()).or_default().push(t);
    }

    let mut out: HashMap<String, String> = HashMap::new();
    for (n, group) in buckets {
        // A single-member bucket (or a degenerate 1-segment topic) has no
        // fold axis — map each topic to itself.
        if n <= 1 || group.len() <= 1 {
            for t in &group {
                out.insert((*t).to_string(), (*t).to_string());
            }
            continue;
        }
        // Greedy multi-axis: repeatedly claim the largest over-threshold
        // wildcard-group, on any single position, until none remain.
        let mut remaining: Vec<(&str, Vec<&str>)> =
            group.iter().map(|t| (*t, t.split('/').collect::<Vec<&str>>())).collect();
        loop {
            // Best (count, position, template) over all positions of what's left.
            let mut best: Option<(usize, usize, String)> = None;
            for p in 0..n {
                let mut groups: HashMap<String, usize> = HashMap::new();
                for (_t, segs) in &remaining {
                    let mut tmpl = segs.clone();
                    tmpl[p] = "*";
                    *groups.entry(tmpl.join("/")).or_insert(0) += 1;
                }
                for (tmpl, cnt) in groups {
                    if cnt <= SINGLETON_FANOUT_THRESHOLD {
                        continue;
                    }
                    let better = match &best {
                        None => true,
                        // higher count; tie → later position; tie → smaller template
                        Some((bc, bp, bt)) => {
                            cnt > *bc
                                || (cnt == *bc && p > *bp)
                                || (cnt == *bc && p == *bp && &tmpl < bt)
                        }
                    };
                    if better {
                        best = Some((cnt, p, tmpl));
                    }
                }
            }
            let Some((_cnt, _p, tmpl)) = best else { break };
            // Claim every remaining topic the template matches; rest carries on.
            let mut kept: Vec<(&str, Vec<&str>)> = Vec::new();
            for (t, segs) in remaining.into_iter() {
                if template_matches(&tmpl, t) {
                    out.insert(t.to_string(), tmpl.clone());
                } else {
                    kept.push((t, segs));
                }
            }
            remaining = kept;
        }
        // Genuinely-small families (no group cleared the threshold) → self.
        for (t, _segs) in remaining {
            out.insert(t.to_string(), t.to_string());
        }
    }
    out
}

/// Does a fold template (with `*` wildcards at the varying segment positions)
/// match a concrete topic? Segment counts must be equal and every non-`*`
/// segment must match literally. This is the expand-path counterpart to
/// `fold_group_templates`: `/api/snapshot/expand` is handed a template in the
/// `namespace` query param and uses this to re-select the member topics WITHOUT
/// needing the whole publisher set. (`*` here is a single-segment wildcard —
/// the only kind the fold ever emits; this is NOT general Zenoh keyexpr
/// matching.)
pub fn template_matches(template: &str, topic: &str) -> bool {
    let ts: Vec<&str> = template.split('/').collect();
    let ks: Vec<&str> = topic.split('/').collect();
    ts.len() == ks.len() && ts.iter().zip(ks.iter()).all(|(t, k)| *t == "*" || t == k)
}

/// Fold high-cardinality fanout in-place to bound the wire payload. For
/// each `(publisher, topic_namespace)` group where the group size
/// exceeds the threshold, replace the N individual edges with one
/// synthetic edge carrying `folded_singletons_count: Some(N)` and
/// `topic: "<ns>/* (folded xN)"`. Aggregates: sum EMA + projected EPS +
/// total_events; max last_event_ms; first non-None schema_family +
/// format (groups are typically homogeneous on schema).
///
/// Sink-resolved edges (`e.sink.is_some()`) are deliberately skipped —
/// those are pub→sub flow edges, not fanout pins; folding would
/// conflate subscriber relationships.
///
/// Field + function names retain the "singleton" wording because
/// `folded_singletons_count` is wire-committed (rename would break the
/// frontend's deserialization contract) and the v1 version of this fold
/// gated on `total_events == 1`. That gate misses the real fanout shape: a
/// dominant publisher emitting 2-12 events per UUID-per-entity topic (entity
/// create + update lifecycle) slips past it and the snapshot stays large.
/// The current heuristic
/// is purely structural cardinality: any fanout group over threshold
/// folds regardless of event count. This is the correct semantics — a
/// publisher fanning out 2,000 topics under one namespace is a fanout
/// pattern whether or not those topics keep ticking after init.
fn fold_singletons_in_edges(edges: Vec<EdgeView>) -> Vec<EdgeView> {
    use std::collections::HashMap;

    // Per-publisher backstop first: a publisher whose fan-out stays an outlier
    // even after template folding is rolled up whole, flows included. See
    // `rollup_outlier_publishers`.
    let (mut result, edges) = rollup_outlier_publishers(edges);

    // Split off sink-resolved edges (pub→sub flows are never folded —
    // folding would conflate per-topic subscriber relationships), and bucket
    // the rest by publisher so the template derivation sees each publisher's
    // full topic set at once (the varying-segment detection is per-set).
    let mut by_source: HashMap<PeerId, Vec<EdgeView>> = HashMap::new();
    for e in edges {
        if e.sink.is_some() {
            result.push(e);
            continue;
        }
        by_source.entry(e.source.clone()).or_default().push(e);
    }

    for (source, source_edges) in by_source {
        // Derive the fold template for each of this publisher's topics
        // (wildcards the varying segment at ANY position — see
        // `fold_group_templates`), then group the edges by template.
        let topics: Vec<&str> = source_edges.iter().map(|e| e.topic.as_str()).collect();
        let templates = fold_group_templates(&topics);
        let mut groups: HashMap<String, Vec<EdgeView>> = HashMap::new();
        for e in source_edges {
            let tmpl = templates
                .get(&e.topic)
                .cloned()
                .unwrap_or_else(|| e.topic.clone());
            groups.entry(tmpl).or_default().push(e);
        }

        // STRUCTURAL fanout fold: any (publisher, template) group over the
        // threshold collapses, regardless of per-topic event count. The old
        // `total_events == 1` gate (a proxy for "UUID-per-entity init publish
        // that never updates") was dropped: a publisher emitting 2-12 events per
        // entity slips such a gate entirely. The grouping key is now a varying-segment template,
        // not a strip-last-segment prefix, so version-after-entity fanout
        // (`<anchor>/aircraft/<id>/v1`) folds too.
        for (tmpl, group) in groups {
            if group.len() <= SINGLETON_FANOUT_THRESHOLD {
                result.extend(group);
                continue;
            }
            let count = group.len() as u64;
            let ema_eps: f64 = group.iter().map(|e| e.ema_eps).sum();
            let projected_eps: f64 = group.iter().map(|e| e.projected_eps).sum();
            let total_events: u64 = group.iter().map(|e| e.total_events).sum();
            let last_event_ms: u64 = group.iter().map(|e| e.last_event_ms).max().unwrap_or(0);
            let schema_family = group.iter().find_map(|e| e.schema_family.clone());
            let format = group.iter().find_map(|e| e.format.clone());
            // The template already carries its `*`, so the display string is
            // `"<template> (folded xN)"`. For a trailing-id pattern the
            // template is `<prefix>/*`, reproducing the prior
            // `"<prefix>/* (folded xN)"` display verbatim.
            result.push(EdgeView {
                source: source.clone(),
                sink: None,
                topic: format!("{tmpl} (folded x{count})"),
                ema_eps,
                projected_eps,
                total_events,
                last_event_ms,
                schema_family,
                format,
                folded_singletons_count: Some(count),
                folded_namespace: Some(tmpl),
            });
        }
    }
    result
}

/// Per-publisher fan-out ceiling for the server rollup. Mirrors
/// `PUBLISHER_FANOUT_THRESHOLD` in the frontend `render.ts`: a publisher whose
/// topics still form more than this many separate groups after template
/// folding is an outlier and is rolled up whole.
const PUBLISHER_FANOUT_THRESHOLD: usize = 40;

/// Row floor for the server rollup. The rollup bounds the payload, and a
/// publisher under this many edges costs a few hundred KB at most, so it keeps
/// its per-topic rows for `goat flow` and the frontend folds it for display.
/// A family of 41 topics and 247 edges stays per-topic; the
/// chunk streamer (8,998 topics, 211,796 edges) rolls up.
const PUBLISHER_ROLLUP_MIN_EDGES: usize = 1000;

/// Segment-wise longest common prefix of a topic set (`a/b/c`, `a/b/d` →
/// `a/b`). Empty when the topics share no leading segment.
fn common_segment_prefix(topics: &[&str]) -> String {
    let mut iter = topics.iter();
    let Some(first) = iter.next() else { return String::new() };
    let mut prefix: Vec<&str> = first.split('/').collect();
    for t in iter {
        let n = prefix
            .iter()
            .zip(t.split('/'))
            .take_while(|(a, b)| *a == b)
            .count();
        prefix.truncate(n);
        if prefix.is_empty() {
            break;
        }
    }
    prefix.join("/")
}

/// Does a rollup namespace (`<prefix>/**`) cover this topic? The rollup is the
/// only fold that emits a trailing `/**`; templates only ever carry single `*`.
pub fn rollup_matches(namespace: &str, topic: &str) -> bool {
    match namespace.strip_suffix("/**") {
        Some(prefix) => {
            topic == prefix
                || (topic.len() > prefix.len()
                    && topic.starts_with(prefix)
                    && topic.as_bytes()[prefix.len()] == b'/')
        }
        None => false,
    }
}

/// Distinct topic keys one publisher's edges put on the wire after
/// `fold_singletons_in_edges`' template fold: each over-threshold template of
/// sink-less topics counts once, every other topic (sink-resolved or in a small
/// family) counts once.
fn wire_topic_keys(group: &[EdgeView]) -> usize {
    use std::collections::{HashMap, HashSet};
    let mut sinkless_seen: HashSet<&str> = HashSet::new();
    let sinkless: Vec<&str> = group
        .iter()
        .filter(|e| e.sink.is_none())
        .map(|e| e.topic.as_str())
        .filter(|t| sinkless_seen.insert(t))
        .collect();
    let templates = fold_group_templates(&sinkless);
    let mut sizes: HashMap<&str, usize> = HashMap::new();
    for t in templates.values() {
        *sizes.entry(t.as_str()).or_insert(0) += 1;
    }
    let mut keys: HashSet<&str> = HashSet::new();
    for e in group {
        let key = match (e.sink.is_none(), templates.get(e.topic.as_str())) {
            (true, Some(t)) if sizes[t.as_str()] > SINGLETON_FANOUT_THRESHOLD => t.as_str(),
            _ => e.topic.as_str(),
        };
        keys.insert(key);
    }
    keys.len()
}

/// Roll up every publisher whose fan-out is an outlier into ONE synthetic edge,
/// sink-resolved flows included. Returns `(rolled_up_edges, untouched_edges)`.
///
/// The template fold below skips sink-resolved edges. A publisher that sends
/// one topic per chunk (`<anchor>/imagery/v1/stream/<id>/chunk/<n>/<part>`) to
/// a set of wildcard subscribers therefore reaches the wire unfolded, once per
/// topic per subscriber. One such publisher can produce thousands of
/// topics × ~23 subscribers = 211,796 edges: a 65 MB snapshot on every poll.
///
/// Rule: count the distinct topic keys this publisher would put on the wire
/// after the template fold (a folded sink-less group counts once, every
/// sink-resolved topic counts once however many subscribers it has). Over
/// `PUBLISHER_FANOUT_THRESHOLD`, with more than `PUBLISHER_ROLLUP_MIN_EDGES`
/// edges → one edge, topic `<prefix>/** (folded xN)`,
/// `folded_namespace = <prefix>/**`, where `<prefix>` is the topics' common
/// leading segments and N the distinct topic count. The frontend's
/// per-publisher fold bundles the same publisher at the same threshold, so this
/// changes the payload, not what paints. Rates are summed over distinct topics,
/// not per-subscriber copies. A publisher with no common prefix is left alone,
/// so a scoped viewer's per-key filter still sees an anchored key.
/// `/api/snapshot/expand` with that namespace lists the publisher's topics.
fn rollup_outlier_publishers(edges: Vec<EdgeView>) -> (Vec<EdgeView>, Vec<EdgeView>) {
    use std::collections::{HashMap, HashSet};

    let mut by_source: HashMap<PeerId, Vec<EdgeView>> = HashMap::new();
    for e in edges {
        by_source.entry(e.source.clone()).or_default().push(e);
    }

    let mut rolled: Vec<EdgeView> = Vec::new();
    let mut rest: Vec<EdgeView> = Vec::new();
    for (source, group) in by_source {
        let mut seen: HashSet<&str> = HashSet::new();
        let topics: Vec<&str> = group
            .iter()
            .map(|e| e.topic.as_str())
            .filter(|t| seen.insert(t))
            .collect();
        if group.len() <= PUBLISHER_ROLLUP_MIN_EDGES || topics.len() <= PUBLISHER_FANOUT_THRESHOLD {
            rest.extend(group);
            continue;
        }
        if wire_topic_keys(&group) <= PUBLISHER_FANOUT_THRESHOLD {
            rest.extend(group);
            continue;
        }
        let prefix = common_segment_prefix(&topics);
        if prefix.is_empty() {
            rest.extend(group);
            continue;
        }
        let count = topics.len() as u64;
        // One representative edge per distinct topic: a sink-resolved topic
        // repeats the same flow state once per subscriber.
        let mut per_topic: HashMap<&str, &EdgeView> = HashMap::new();
        for e in &group {
            per_topic.entry(e.topic.as_str()).or_insert(e);
        }
        let reps: Vec<&EdgeView> = per_topic.into_values().collect();
        let namespace = format!("{prefix}/**");
        rolled.push(EdgeView {
            source: source.clone(),
            sink: None,
            topic: format!("{namespace} (folded x{count})"),
            ema_eps: reps.iter().map(|e| e.ema_eps).sum(),
            projected_eps: reps.iter().map(|e| e.projected_eps).sum(),
            total_events: reps.iter().map(|e| e.total_events).sum(),
            last_event_ms: reps.iter().map(|e| e.last_event_ms).max().unwrap_or(0),
            schema_family: reps.iter().find_map(|e| e.schema_family.clone()),
            format: reps.iter().find_map(|e| e.format.clone()),
            folded_singletons_count: Some(count),
            folded_namespace: Some(namespace),
        });
    }
    (rolled, rest)
}

/// Sentinel `folded_namespace` value the aggressive per-publisher rollup
/// emits. The expand endpoint recognizes it and returns ALL of the source's
/// fanout edges (not a single template's), so the operator can still drill in.
pub const AGGRESSIVE_ALL_NAMESPACE: &str = "**";

/// Aggressive simplified view (the nuanced replacement for the blunt
/// "collapse all / expand all" buttons). Runs AFTER `fold_singletons_in_edges`
/// and collapses EVERYTHING a publisher emits — its namespace-folds AND its
/// distinct one-off topics — into ONE synthetic bundle edge per publisher.
/// The blunt collapse only hid the already-folded namespace groups; this rolls
/// up the long tail of heterogeneous singletons that the structural fold leaves
/// untouched (each below threshold on its own), which is what actually clutters
/// a wide-shallow publisher's corner of the map. pub→sub flows are never folded
/// (same rule as the namespace fold — those are real relationships, not fanout).
/// The bundle carries `folded_namespace = AGGRESSIVE_ALL_NAMESPACE` so the
/// existing expand interaction (click to explode) drills the whole publisher.
fn fold_all_by_publisher(edges: Vec<EdgeView>) -> Vec<EdgeView> {
    use std::collections::HashMap;
    let mut by_source: HashMap<PeerId, Vec<EdgeView>> = HashMap::new();
    let mut result: Vec<EdgeView> = Vec::new();
    for e in edges {
        if e.sink.is_some() {
            result.push(e); // flows pass through untouched
            continue;
        }
        by_source.entry(e.source.clone()).or_default().push(e);
    }

    for (source, group) in by_source {
        // A single fanout edge needs no bundling — nothing to simplify.
        if group.len() <= 1 {
            result.extend(group);
            continue;
        }
        // Distinct-topic count behind the bundle: a namespace-fold edge already
        // stands for N topics (folded_singletons_count), a one-off stands for 1.
        let count: u64 = group
            .iter()
            .map(|e| e.folded_singletons_count.unwrap_or(1))
            .sum();
        let ema_eps: f64 = group.iter().map(|e| e.ema_eps).sum();
        let projected_eps: f64 = group.iter().map(|e| e.projected_eps).sum();
        let total_events: u64 = group.iter().map(|e| e.total_events).sum();
        let last_event_ms: u64 = group.iter().map(|e| e.last_event_ms).max().unwrap_or(0);
        let schema_family = group.iter().find_map(|e| e.schema_family.clone());
        let format = group.iter().find_map(|e| e.format.clone());
        result.push(EdgeView {
            source: source.clone(),
            sink: None,
            topic: format!("all topics (folded x{count})"),
            ema_eps,
            projected_eps,
            total_events,
            last_event_ms,
            schema_family,
            format,
            folded_singletons_count: Some(count),
            folded_namespace: Some(AGGRESSIVE_ALL_NAMESPACE.to_string()),
        });
    }
    result
}

/// Apply the aggressive per-publisher rollup to a built snapshot edge set,
/// preserving the snapshot's (source, topic) sort order. Pure wrapper so the
/// handler stays a one-liner and the fold is unit-testable.
pub fn aggressive_rollup(mut edges: Vec<EdgeView>) -> Vec<EdgeView> {
    edges = fold_all_by_publisher(edges);
    edges.sort_by(|a, b| a.source.cmp(&b.source).then_with(|| a.topic.cmp(&b.topic)));
    edges
}

/// Companion to `fold_singletons_in_edges`: bound the `publishes` array
/// on each peer so a publisher with 2.2k UUID-per-entity topics doesn't
/// carry 2.2k strings on the wire. Same threshold + namespace grouping
/// as the edge fold; folded groups are replaced with one synthetic
/// entry `"<ns>/* (folded xN)"` matching the edge-fold topic format.
/// Operators reading the side panel see the same shape they see on the
/// canvas.
fn fold_publishes(publishes: &[String]) -> Vec<String> {
    use std::collections::HashMap;
    // Same varying-segment template grouping as the edge fold, so the side
    // panel's `publishes` list folds the exact same shape the canvas does.
    let topics: Vec<&str> = publishes.iter().map(|s| s.as_str()).collect();
    let templates = fold_group_templates(&topics);
    let mut by_tmpl: HashMap<String, Vec<String>> = HashMap::new();
    for t in publishes {
        let tmpl = templates.get(t).cloned().unwrap_or_else(|| t.clone());
        by_tmpl.entry(tmpl).or_default().push(t.clone());
    }
    let mut result: Vec<String> = Vec::new();
    for (tmpl, group) in by_tmpl {
        if group.len() > SINGLETON_FANOUT_THRESHOLD {
            result.push(format!("{tmpl} (folded x{})", group.len()));
        } else {
            result.extend(group);
        }
    }
    result.sort();
    result
}

/// History-lens window filter (docs/design/panoscope-history-lens.md): keep
/// only peers seen within the operator's chosen horizon, and drop edges whose
/// endpoints fell out so the frontend isn't left with danglers. `window_hours`
/// of 0 means "no filter" (every accumulated peer) — pre-lens behavior. Relies
/// on `last_seen_ms` being the REAL last-published time (replay now stamps the
/// event ts), so live vs historical is honestly separable.
pub fn filter_snapshot_window(
    nodes: Vec<NodeView>,
    edges: Vec<EdgeView>,
    now_ms: u64,
    window_hours: u64,
) -> (Vec<NodeView>, Vec<EdgeView>) {
    if window_hours == 0 {
        return (nodes, edges);
    }
    let cutoff = now_ms.saturating_sub(window_hours.saturating_mul(3_600_000));
    let kept: std::collections::HashSet<String> = nodes
        .iter()
        .filter(|n| n.last_seen_ms >= cutoff)
        .map(|n| n.peer_id.clone())
        .collect();
    let nodes: Vec<NodeView> = nodes
        .into_iter()
        .filter(|n| kept.contains(&n.peer_id))
        .collect();
    let edges: Vec<EdgeView> = edges
        .into_iter()
        .filter(|e| kept.contains(&e.source) && e.sink.as_ref().map_or(true, |s| kept.contains(s)))
        .collect();
    (nodes, edges)
}

/// Per-publisher `TopicStat` set for the hygiene classifier, built from the
/// RAW (unfolded) edges so the classifier sees the true topic cardinality +
/// per-topic event counts. Keyed by canonical publisher; one stat per distinct
/// topic (`total_events` from the edge, `has_schema` true when a schema family
/// or wire format is known). Sink-resolved edges still count — a topic the
/// publisher emits is part of its key space regardless of who subscribes.
/// Takes raw edges the caller already built, so the snapshot build walks the
/// flow graph once.
fn publisher_topic_stats_from(
    raw: &[EdgeView],
) -> std::collections::HashMap<PeerId, Vec<panoscope_core::TopicStat>> {
    use std::collections::HashMap;
    let mut by_src: HashMap<PeerId, HashMap<String, panoscope_core::TopicStat>> = HashMap::new();
    for e in raw {
        let per_topic = by_src.entry(e.source.clone()).or_default();
        // Dedup by topic; keep the largest event count + any schema signal
        // across fan-out-to-subscriber duplicate edges of the same topic.
        let has_schema = e.schema_family.is_some() || e.format.is_some();
        per_topic
            .entry(e.topic.clone())
            .and_modify(|s| {
                s.total_events = s.total_events.max(e.total_events);
                s.has_schema = s.has_schema || has_schema;
            })
            .or_insert(panoscope_core::TopicStat {
                key: e.topic.clone(),
                total_events: e.total_events,
                has_schema,
            });
    }
    by_src
        .into_iter()
        .map(|(src, topics)| (src, topics.into_values().collect()))
        .collect()
}

/// Attach the cheap (key-shape-only) hygiene verdict to each publisher node.
/// Non-publishers (no edges) get no `hygiene`.
fn attach_hygiene_stats(
    nodes: &mut [NodeView],
    stats: &std::collections::HashMap<PeerId, Vec<panoscope_core::TopicStat>>,
) {
    for node in nodes.iter_mut() {
        if let Some(s) = stats.get(&node.peer_id) {
            let h = panoscope_core::classify_publisher(s);
            node.hygiene = Some(NodeHygiene {
                verdict: h.verdict,
                codes: h.findings.iter().map(|f| f.code.clone()).collect(),
            });
        }
    }
}

pub(crate) fn format_label(f: SampleFormat) -> &'static str {
    match f {
        SampleFormat::Json => "json",
        SampleFormat::Cbor => "cbor",
        SampleFormat::Protobuf => "protobuf",
        SampleFormat::Text => "text",
        SampleFormat::Hex => "hex",
        SampleFormat::Empty => "empty",
    }
}

pub fn build_nodes_view(
    topology: &Topology,
    aliases: &panoscope_core::AliasRegistry,
) -> Vec<NodeView> {
    build_nodes_view_with(topology, aliases, |p| NodeView::from(p))
}

/// `build_nodes_view` for the view model: identical ghost filter and alias
/// projection, without the per-node `publishes` fold.
pub fn build_nodes_view_unfolded(
    topology: &Topology,
    aliases: &panoscope_core::AliasRegistry,
) -> Vec<NodeView> {
    build_nodes_view_with(topology, aliases, |p| NodeView::with_publishes(p, Vec::new()))
}

/// How long a subscriber-only peer may go unseen before it stops being drawn
/// and stops costing a rebuild. `0` disables the filter entirely.
///
/// WHY THIS EXISTS. A subscriber that reconnects arrives with a NEW session
/// id and nothing ties it to the old one: it carries no locator, no vendor,
/// no alias — nearly every subscriber-only peer has
/// `attest_source = "admin"` and every other identifying field
/// empty. Publishers are folded across reconnects because a topic has a
/// schema-declared owner (`topic_to_publisher`); subscribers have no
/// equivalent, so there is nothing to fold ON and the sessions accumulate.
///
/// A long-running fabric therefore accumulates "peers" that are mostly
/// short-lived subscriber sessions, many subscribing to one identical topic:
/// one client reconnecting every few minutes, not many systems. That miscount
/// is not only cosmetic: the model build computes a subscriber list per topic,
/// so every phantom joins a cross-product that multiplies rebuild cost.
///
/// Ageing them out is the honest filter available. It needs no identity, and
/// a peer nobody has heard from in an hour is not part of the current picture.
/// Staleness is relative to the freshest thing the topology knows about, so a
/// replayed week of history is not wiped out by the wall clock.
/// A PUBLISHER is never aged out here, however quiet: "declared but silent" is
/// a fact an operator needs to see.
pub fn subscriber_ttl_ms() -> u64 {
    std::env::var("PANOSCOPE_SUBSCRIBER_TTL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(3_600)
        * 1_000
}

fn build_nodes_view_with(
    topology: &Topology,
    aliases: &panoscope_core::AliasRegistry,
    make: impl Fn(&PeerInfo) -> NodeView,
) -> Vec<NodeView> {
    let ttl = subscriber_ttl_ms();
    // The most recent moment this topology knows about. On a live fabric that
    // is ~now; on a replayed one it is the replay head.
    let newest = topology.peers.values().map(|p| p.last_seen_ms).max().unwrap_or(0);
    let mut nodes: Vec<NodeView> = topology
        .peers
        .values()
        // Hide "ghost" peers — those created by the topic-prefix
        // fallback in the data tail BEFORE the canonical schema-
        // declaration arrived. Ghosts have all of their publish
        // topics now "owned" by a different peer via the
        // topic_to_publisher index. Peers with no publishes (e.g.
        // pure subscribers from schema decl) are always kept.
        .filter(|p| {
            if p.publishes.is_empty() {
                // A subscriber-only peer is kept only while it is still
                // current: see `subscriber_ttl_ms`. Without this a
                // reconnecting subscriber accumulates one peer per session
                // for the whole replay window.
                //
                // Staleness is measured against the FRESHEST observation in
                // this topology, not the wall clock. panoscope replays up to
                // a week of history on boot, where wall-clock now is hours
                // ahead of the data — comparing to it would age out the
                // entire replayed fabric.
                if ttl == 0 {
                    return true;
                }
                return p.last_seen_ms == 0 || newest.saturating_sub(p.last_seen_ms) <= ttl;
            }
            p.publishes.iter().any(|t| {
                topology
                    .topic_to_publisher
                    .get(t)
                    .map_or(true, |owner| owner == &p.peer_id)
            })
        })
        .map(|p| {
            let mut nv = make(p);
            // Project the slot anchor → its alias for display. `alias_of`
            // returns Some only when peer_id is a known 32-hex anchor; a
            // random ZID, an unknown anchor, or an empty/degraded registry
            // all yield None, so the frontend's display_name/UUID fallback is
            // untouched. peer_id itself stays the canonical UUID (§3).
            nv.slot_alias = aliases
                .alias_of(&nv.peer_id)
                .or_else(|| write_root_alias(p.publishes.iter(), aliases));
            nv
        })
        .collect();
    nodes.sort_by(|a, b| a.peer_id.cmp(&b.peer_id));
    nodes
}

/// F-278 fallback: name a publisher by the slot it writes into when its
/// `peer_id` itself resolves no alias. Most live publishers are keyed by
/// their Zenoh session ZID (SourceInfo / admin space), not by their slot
/// anchor, so the peer_id lookup misses even though the registry knows the
/// slot — the anchor is sitting right there as the leading segment of every
/// key the peer publishes.
///
/// Sound only for a SINGLE anchor-shaped write root: `<uuid>/**` roots are
/// single-owner (ADR 1105 — co-publish happens under `multipub/...`, whose
/// leading segment is not anchor-shaped and therefore never matches), so one
/// root pins the publisher to one slot identity. Zero or several distinct
/// anchor roots → `None`, and the ZID/hex fallback stands. Display-only,
/// same as every alias projection (ADR 1039 §3).
fn write_root_alias<'k>(
    publishes: impl IntoIterator<Item = &'k String>,
    aliases: &panoscope_core::AliasRegistry,
) -> Option<String> {
    let mut root: Option<&str> = None;
    for key in publishes {
        let first = key.trim_start_matches('/').split('/').next().unwrap_or("");
        if !panoscope_core::alias::is_anchor_shaped(first) {
            continue;
        }
        match root {
            None => root = Some(first),
            Some(r) if r == first => {}
            Some(_) => return None, // multiple slot roots — ambiguous, stay hex
        }
    }
    aliases.alias_of(root?)
}

/// Per-topic edges without the fold applied. The folded `/api/snapshot`
/// response calls this then folds; the `/api/snapshot/expand` endpoint
/// calls this then filters to the requested (source, namespace) group
/// so the operator can drill into a server-folded super-edge without
/// the rest of the snapshot un-folding too. The factor-out keeps the
/// two callers semantically aligned — same source-canonicalization,
/// same subscriber-matching, same format detection.
#[cfg(test)]
fn build_raw_edges(
    flow: &FlowState,
    topology: &Topology,
    samples: &SampleBuffer,
    now_ms: u64,
) -> Vec<EdgeView> {
    build_raw_edges_with(flow, topology, &|t| samples.latest_format(t), now_ms)
}

/// `build_raw_edges` with the latest-format lookup supplied by the caller, so
/// the shared build can run without holding the sample-buffer lock.
fn build_raw_edges_with(
    flow: &FlowState,
    topology: &Topology,
    latest_format: &dyn Fn(&str) -> Option<SampleFormat>,
    now_ms: u64,
) -> Vec<EdgeView> {
    let mut edges: Vec<EdgeView> = Vec::new();
    for e in flow.edges.values() {
        // Resolve the edge source to the canonical publisher (via the
        // schema-declaration index). Older edges may carry the
        // topic-prefix ghost peer_id from before the schema decl
        // arrived; swap to the real one so the rendered edge connects
        // to a peer that's still in the nodes list.
        let canonical_source = topology
            .topic_to_publisher
            .get(&e.topic)
            .cloned()
            .unwrap_or_else(|| e.source.clone());
        let format = latest_format(&e.topic)
            .map(|f| format_label(f).to_string());
        // Find subscriber peers whose declared subscribes match this
        // publisher edge's topic. When found, materialize the edge as
        // publisher → subscriber so the COP shows real cross-vendor
        // data flow. When not found, fall back to publisher → null
        // (frontend renders as topic-virtual node, current behaviour).
        let matching_subs: Vec<PeerId> = topology
            .peers
            .values()
            .filter(|p| p.peer_id != canonical_source)
            // Universal `**` observers are excluded: drawing a per-topic
            // edge from every topic to every "watches everything" consumer
            // (omniwatch, audit-log, archive-vault, fabric-health, an
            // operator console) is N×M edges of pure noise that turns the
            // COP into a near-complete-bipartite hairball. They still render
            // as nodes, badged via `NodeView.universal_observer`; a topic
            // consumed ONLY by such observers falls through to a clean
            // `pub → topic` edge (matching_subs empty). Specific
            // (non-universal) subscribers are unaffected.
            .filter(|p| !subscribes_universally(p))
            .filter(|p| {
                p.subscribes
                    .iter()
                    .any(|s| keyexpr_matches(s, &e.topic))
            })
            .map(|p| p.peer_id.clone())
            .collect();
        if matching_subs.is_empty() {
            let mut ev = EdgeView::from_state(e, now_ms, format.clone());
            ev.source = canonical_source.clone();
            edges.push(ev);
        } else {
            for sink in matching_subs {
                let mut ev = EdgeView::from_state(e, now_ms, format.clone());
                ev.source = canonical_source.clone();
                ev.sink = Some(sink);
                edges.push(ev);
            }
        }
    }
    dedup_canonical_edges(edges)
}

/// Collapse edges that became identical after source-canonicalization.
///
/// `flow.edges` is keyed by `EdgeKey { source, topic }` where `source` is
/// the raw session ZID. A publisher that reconnects (new ZID each time)
/// leaves one stale `EdgeState` per past session, and nothing evicts them
/// — so a long-lived or frequently-restarted fabric accumulates many
/// ghost edges for the same logical topic. The source-canonicalization
/// above rewrites every one of those to the same real publisher, which
/// makes them collide on `(source, topic, sink)`. Without this dedup the
/// snapshot carries N copies of every flow after a day of restarts,
/// which both crowds the canvas AND inflates the fleet-rate sum the renderer
/// computes from `projected_eps`, silently tripping the >200 packet-
/// dot kill so no dots animate anywhere even on a quiet fabric.
///
/// The ghosts are the *same* logical flow across sessions, not concurrent
/// flows, so we keep the freshest by `last_event_ms` (its EMA reflects the
/// current rate) rather than summing — summing would double-count one flow.
fn dedup_canonical_edges(edges: Vec<EdgeView>) -> Vec<EdgeView> {
    use std::collections::HashMap;
    let mut best: HashMap<(PeerId, Option<PeerId>, String), EdgeView> = HashMap::new();
    for ev in edges {
        let key = (ev.source.clone(), ev.sink.clone(), ev.topic.clone());
        match best.get(&key) {
            Some(existing) if existing.last_event_ms >= ev.last_event_ms => {}
            _ => {
                best.insert(key, ev);
            }
        }
    }
    best.into_values().collect()
}

/// One snapshot build shared by every viewer (see `shared_snapshot_build`).
/// Held behind `Arc`s: a request clones the vectors it filters, never the
/// build.
#[derive(Clone)]
pub struct SnapshotBuild {
    pub built_at_ms: u64,
    pub build_ms: u64,
    pub nodes: std::sync::Arc<Vec<NodeView>>,
    pub edges: std::sync::Arc<Vec<EdgeView>>,
    pub sampling_drops: std::sync::Arc<std::collections::HashMap<String, u64>>,
}

pub type SnapshotBuildCache = std::sync::Arc<tokio::sync::Mutex<Option<SnapshotBuild>>>;

/// Floor on how long a build is reused. The effective lifetime is the larger
/// of this and twice the last build's duration, so a slow build cannot be
/// immediately rebuilt by the next waiting request.
pub const SNAPSHOT_BUILD_MIN_TTL_MS: u64 = 1_000;

/// The unscoped, unwindowed snapshot build, shared across viewers.
///
/// Before this, every `/api/snapshot` request and every WebSocket connect
/// rebuilt the whole graph: raw edges (every flow edge against every peer's
/// subscribe patterns) twice — once for hygiene, once for the edges — then
/// the fold. That can reach 1.4–2.2 s per request, and ten
/// viewers polling every 1.5 s held panoscope at 219% CPU (four busy tokio
/// workers). Cost grew with every open tab.
///
/// Now one build runs at a time (the mutex is the single flight: requests
/// that arrive during a build wait for it and reuse it) and it is reused for
/// `max(SNAPSHOT_BUILD_MIN_TTL_MS, 2 × last build)`. Viewer scope, history
/// window and the aggressive rollup stay per request, applied to a clone.
/// Raw edges are built once per build.
pub async fn shared_snapshot_build(state: &crate::zenoh_client::SharedState) -> SnapshotBuild {
    let mut guard = state.snapshot_build.lock().await;
    let now = crate::model::now_ms();
    if let Some(b) = guard.as_ref() {
        let ttl = SNAPSHOT_BUILD_MIN_TTL_MS.max(b.build_ms.saturating_mul(2));
        if now.saturating_sub(b.built_at_ms) < ttl {
            return b.clone();
        }
    }
    let started = std::time::Instant::now();
    // Copy what the build reads, one lock at a time, then build with no lock
    // held. The data tail takes the topology, flow and sample-buffer WRITE
    // locks on every sample; a build that held the read locks for its full
    // locks on every sample; a build that holds the read locks for its full
    // duration stalls ingest, panoscope stops draining its Zenoh session, and
    // the router closes the transport ("Unable to push non droppable network
    // message"). The copies cost milliseconds.
    let (topology, flow, formats) = copy_ingest_state(state).await;
    let aliases = state.aliases.read().await.clone();
    let copy_ms = started.elapsed().as_millis() as u64;
    // The build itself is CPU-bound for seconds on a large fabric: run it on
    // the blocking pool, not a tokio worker.
    // Admin path, not the relay hot path the workspace lint is written for.
    #[allow(clippy::disallowed_methods)]
    let built = tokio::task::spawn_blocking(move || {
        let raw = build_raw_edges_with(&flow, &topology, &|t| formats.get(t).copied(), now);
        let mut nodes = build_nodes_view(&topology, &aliases);
        attach_hygiene_stats(&mut nodes, &publisher_topic_stats_from(&raw));
        (nodes, fold_and_sort(raw), flow.sampling_drops)
    })
    .await;
    let (nodes, edges, sampling_drops) = match built {
        Ok(b) => b,
        Err(e) => {
            // A panicked build serves the previous one (or an empty one)
            // rather than taking the request down with it.
            tracing::warn!(error = %e, "snapshot: shared build failed");
            if let Some(b) = guard.as_ref() {
                return b.clone();
            }
            (Vec::new(), Vec::new(), std::collections::HashMap::new())
        }
    };
    let build_ms = started.elapsed().as_millis() as u64;
    tracing::debug!(copy_ms, build_ms, edges = edges.len(), "snapshot: shared build");
    let build = SnapshotBuild {
        built_at_ms: now,
        build_ms,
        nodes: std::sync::Arc::new(nodes),
        edges: std::sync::Arc::new(edges),
        sampling_drops: std::sync::Arc::new(sampling_drops),
    };
    *guard = Some(build.clone());
    build
}

/// Copy the topology, the flow state and the latest wire format of every flow
/// topic, taking each ingest lock only for its own copy. Builds then run on
/// the copies with no lock held.
pub async fn copy_ingest_state(
    state: &crate::zenoh_client::SharedState,
) -> (
    Topology,
    FlowState,
    std::collections::HashMap<String, SampleFormat>,
) {
    let topology = state.topology.read().await.clone();
    let flow = state.flow.read().await.clone();
    let formats: std::collections::HashMap<String, SampleFormat> = {
        let samples = state.samples.read().await;
        flow.edges
            .values()
            .filter_map(|e| samples.latest_format(&e.topic).map(|f| (e.topic.clone(), f)))
            .collect()
    };
    (topology, flow, formats)
}

fn fold_and_sort(raw: Vec<EdgeView>) -> Vec<EdgeView> {
    let mut edges = fold_singletons_in_edges(raw);
    edges.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then_with(|| a.topic.cmp(&b.topic))
    });
    edges
}

/// Un-fold the (publisher, namespace) group that the snapshot fold
/// collapsed into a single synthetic super-edge. Called by the
/// `/api/snapshot/expand` handler when the operator double-clicks a
/// singletons-super or Alt+clicks to drill into it. Returns the per-
/// topic edges that would have appeared in the snapshot if the fold
/// hadn't run for this group. Caller filters down — we don't refold,
/// don't trim, don't sort by anything other than the snapshot's own
/// (source, topic) order so the frontend's merge into local state is
/// deterministic.
pub fn build_expanded_edges_with(
    flow: &FlowState,
    topology: &Topology,
    latest_format: &dyn Fn(&str) -> Option<SampleFormat>,
    now_ms: u64,
    source: &PeerId,
    namespace: &str,
) -> Vec<EdgeView> {
    let mut edges: Vec<EdgeView> = build_raw_edges_with(flow, topology, latest_format, now_ms)
        .into_iter()
        .filter(|e| {
            // Match the canonical source AND the namespace.
            // Sink-resolved edges are excluded because the fold itself
            // excludes them (folded groups only contain sink:None
            // fanout pins), so the expand returns the same shape the
            // unfolded snapshot would have for this group.
            // `namespace` is the fold *template* (e.g. `<anchor>/aircraft/*/v1`)
            // carried back from the synthetic edge's `folded_namespace`. Match
            // member topics against it segment-wise — the expand counterpart to
            // `fold_group_templates`. (A literal prefix with no `*`, from the
            // legacy trailing-id case, still matches only its exact self, which
            // is fine: the frontend always passes the template we emitted.)
            if &e.source != source {
                return false;
            }
            // An outlier-publisher rollup (`<prefix>/**`) folded the
            // publisher's flows too, so its expand covers them (one row per
            // topic, deduped below).
            if rollup_matches(namespace, &e.topic) {
                return true;
            }
            e.sink.is_none()
                // The aggressive rollup's `**` sentinel means "every fanout
                // topic this publisher emits" — drill the whole publisher, not
                // a single template. Otherwise match the fold template.
                && (namespace == AGGRESSIVE_ALL_NAMESPACE || template_matches(namespace, &e.topic))
        })
        .collect();
    if namespace.ends_with("/**") {
        // A rollup expand lists topics, not per-subscriber copies: an outlier
        // publisher's topics × subscribers is the payload the rollup exists to
        // avoid (211,796 rows for 8,998 topics).
        let mut seen = std::collections::HashSet::new();
        edges.retain(|e| seen.insert(e.topic.clone()));
        for e in &mut edges {
            e.sink = None;
        }
    }
    edges.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then_with(|| a.topic.cmp(&b.topic))
    });
    edges
}

/// Does a subscriber's `pattern` receive `topic`? Uses Zenoh's own
/// key-expression matching so panoscope's edge-drawing agrees with what
/// subscribers actually receive on the wire — including `**` in any
/// position (`**/alpha-fob/**`), single-segment `*`, and `$*` chunks, not
/// just the `<vendor>/**` prefix shape the prior hand-rolled matcher knew.
/// (The minimal matcher silently drew zero edges for the complementary
/// multi-site listeners introduced .) `topic` is a concrete key,
/// so `pattern.includes(topic)` is exactly "pattern matches topic." Falls
/// back to the old minimal rules only when a string isn't a canonical
/// keyexpr — e.g. the synthetic folded markers like `<ns>/* (folded xN)`
/// that carry spaces and would error in `keyexpr::new`.
pub(crate) fn keyexpr_matches(pattern: &str, topic: &str) -> bool {
    use zenoh::key_expr::keyexpr;
    if let (Ok(p), Ok(t)) = (keyexpr::new(pattern), keyexpr::new(topic)) {
        return p.includes(t);
    }
    if pattern == "**" || pattern == topic {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return topic == prefix || topic.starts_with(&format!("{prefix}/"));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Topology;

    #[test]
    fn keyexpr_double_star_matches_anything() {
        assert!(keyexpr_matches("**", "caracal/tracks-demo/entity/v1"));
        assert!(keyexpr_matches("**", "x"));
    }

    #[test]
    fn keyexpr_mid_double_star_matches_site_across_uuids() {
        // The complementary-listener case: `**/<site>/**` must match a
        // per-uuid topic with <site> in the middle, regardless of the uuid
        // prefix or the suffix. The old hand-rolled matcher returned false
        // here (only knew prefix/**), drawing zero edges.
        assert!(keyexpr_matches("**/alpha-fob/**", "2cf6fe6d/octopus/alpha-fob/tracks/v1"));
        assert!(keyexpr_matches("**/theater/**", "e0832940/squid/theater/confirmed/v1"));
        assert!(!keyexpr_matches("**/alpha-fob/**", "2cf6fe6d/aurochs/charlie-fob/launched/v1"));
        // Single-segment `*` works too (a real Zenoh keyexpr feature).
        assert!(keyexpr_matches("*/octopus/alpha-fob/tracks/v1", "x/octopus/alpha-fob/tracks/v1"));
    }

    #[test]
    fn keyexpr_prefix_double_star_matches_under_prefix() {
        assert!(keyexpr_matches("caracal/**", "caracal/tracks-demo/entity/v1"));
        assert!(keyexpr_matches("caracal/**", "caracal/power-demo/site/v1"));
        assert!(!keyexpr_matches("caracal/**", "community/beacon-demo/x"));
    }

    #[test]
    fn keyexpr_exact_match() {
        assert!(keyexpr_matches("a/b/c", "a/b/c"));
        assert!(!keyexpr_matches("a/b/c", "a/b/d"));
    }

    // ------- Server-side singleton fold (bounds /api/snapshot wire) -------

    fn synthetic_edge(source: &str, topic: &str, total_events: u64) -> EdgeView {
        EdgeView {
            source: source.to_string(),
            sink: None,
            topic: topic.to_string(),
            ema_eps: 0.0,
            projected_eps: 0.0,
            total_events,
            last_event_ms: 1000,
            schema_family: None,
            format: None,
            folded_singletons_count: None,
            folded_namespace: None,
        }
    }

    fn node_with_last_seen(id: &str, last_seen_ms: u64) -> NodeView {
        let mut p = PeerInfo::new(id.to_string(), last_seen_ms);
        p.last_seen_ms = last_seen_ms;
        NodeView::from(&p)
    }

    #[test]
    fn window_filter_keeps_in_window_drops_old() {
        let now = 100 * 3_600_000; // 100h, in ms
        // NodeView/EdgeView aren't Clone (wire structs), so rebuild per call.
        let nodes = || {
            vec![
                node_with_last_seen("live", now),                   // age 0h
                node_with_last_seen("recent", now - 5 * 3_600_000), // 5h
                node_with_last_seen("old", now - 50 * 3_600_000),   // 50h
            ]
        };
        let edges = || {
            vec![
                synthetic_edge("live", "live/t", 1),
                synthetic_edge("old", "old/t", 1),
            ]
        };

        // 6h window: live + recent kept; old dropped, and old's edge with it.
        let (n, e) = filter_snapshot_window(nodes(), edges(), now, 6);
        let ids: std::collections::HashSet<_> = n.iter().map(|x| x.peer_id.clone()).collect();
        assert!(ids.contains("live") && ids.contains("recent"));
        assert!(!ids.contains("old"), "50h-old peer dropped at 6h window");
        assert!(e.iter().any(|x| x.source == "live"));
        assert!(!e.iter().any(|x| x.source == "old"), "edge of dropped peer pruned");

        // 48h window: still drops the 50h peer, keeps the 5h one.
        let (n48, _) = filter_snapshot_window(nodes(), edges(), now, 48);
        assert_eq!(n48.len(), 2);

        // 168h (7d) window: everything within reach.
        let (n7d, e7d) = filter_snapshot_window(nodes(), edges(), now, 168);
        assert_eq!(n7d.len(), 3);
        assert_eq!(e7d.len(), 2);

        // window 0 = no filter (pre-lens behavior): all kept.
        let (n0, e0) = filter_snapshot_window(nodes(), edges(), now, 0);
        assert_eq!(n0.len(), 3);
        assert_eq!(e0.len(), 2);
    }

    /// A subscriber that reconnects leaves one peer per session and nothing
    /// ties them together, so the view must age them out or it counts the
    /// same client hundreds of times (2,821 subscriber
    /// sessions, median lifetime 60 s, 564 of them on one identical topic).
    #[test]
    fn stale_subscriber_sessions_are_aged_out_but_publishers_never_are() {
        std::env::set_var("PANOSCOPE_SUBSCRIBER_TTL_SECS", "3600");
        let now = panoscope_core::time::now_ms();
        let hour_ago = now - 3_600_000 - 60_000;
        let mut t = Topology::default();

        // Three reconnect sessions of one subscriber, all long gone.
        for zid in ["sess-a", "sess-b", "sess-c"] {
            t.record_subscribe_declaration(zid.into(), "x/**".into(), hour_ago);
        }
        // One subscriber still current.
        t.record_subscribe_declaration("sess-live".into(), "x/**".into(), now);
        // A publisher that has been SILENT far longer than the TTL. It must
        // still be drawn: "declared but silent" is a fact, not noise.
        t.record_publish_declaration("quiet-pub".into(), "x/one".into(), hour_ago);

        let aliases = panoscope_core::AliasRegistry::empty();
        let nodes = build_nodes_view_unfolded(&t, &aliases);
        let ids: Vec<&str> = nodes.iter().map(|n| n.peer_id.as_str()).collect();

        assert!(ids.contains(&"sess-live"), "a current subscriber must stay");
        assert!(ids.contains(&"quiet-pub"), "a silent PUBLISHER must never be aged out");
        for gone in ["sess-a", "sess-b", "sess-c"] {
            assert!(!ids.contains(&gone), "{gone} was last seen over the TTL ago and must go");
        }
        std::env::remove_var("PANOSCOPE_SUBSCRIBER_TTL_SECS");
    }

    /// The filter is disableable, because a goatnet that wants the old
    /// behaviour should not have to patch the binary to get it.
    #[test]
    fn a_zero_ttl_keeps_every_subscriber() {
        std::env::set_var("PANOSCOPE_SUBSCRIBER_TTL_SECS", "0");
        let ancient = panoscope_core::time::now_ms() - 30 * 86_400_000;
        let mut t = Topology::default();
        t.record_subscribe_declaration("ancient".into(), "x/**".into(), ancient);
        let aliases = panoscope_core::AliasRegistry::empty();
        let nodes = build_nodes_view_unfolded(&t, &aliases);
        assert!(nodes.iter().any(|n| n.peer_id == "ancient"), "ttl=0 disables the filter");
        std::env::remove_var("PANOSCOPE_SUBSCRIBER_TTL_SECS");
    }

    #[test]
    fn dedup_canonical_edges_collapses_ghost_zid_duplicates() {
        // Same logical flow observed across 42 reconnect sessions — after
        // source-canonicalization they all share (source, topic, sink).
        // Without dedup the fleet-rate sum counts the flow 42×.
        let mut edges: Vec<EdgeView> = Vec::new();
        for i in 0..42 {
            let mut e = synthetic_edge("real-publisher", "uuid/mantis/alpha-fob/imagery/v1", 100);
            e.ema_eps = 5.0;
            // The freshest session has the highest last_event_ms.
            e.last_event_ms = 1000 + i;
            edges.push(e);
        }
        // A genuinely distinct flow (different topic) must survive.
        edges.push(synthetic_edge("real-publisher", "uuid/tarsier/bravo/contacts/v1", 1));

        let deduped = dedup_canonical_edges(edges);
        assert_eq!(deduped.len(), 2, "42 ghosts of one flow + 1 distinct → 2 edges");
        let mantis = deduped
            .iter()
            .find(|e| e.topic == "uuid/mantis/alpha-fob/imagery/v1")
            .expect("mantis flow survives");
        assert_eq!(mantis.last_event_ms, 1041, "keeps the freshest session");
    }

    #[test]
    fn dedup_canonical_edges_keeps_distinct_sinks() {
        // Same (source, topic) fanned out to two different subscribers are
        // distinct flows — dedup must NOT collapse them.
        let mut a = synthetic_edge("pub", "uuid/t/v1", 1);
        a.sink = Some("sink-a".to_string());
        let mut b = synthetic_edge("pub", "uuid/t/v1", 1);
        b.sink = Some("sink-b".to_string());
        let deduped = dedup_canonical_edges(vec![a, b]);
        assert_eq!(deduped.len(), 2, "distinct sinks are distinct edges");
    }

    #[tokio::test]
    async fn shared_build_is_reused_within_its_lifetime_and_rebuilt_after() {
        let state = crate::zenoh_client::SharedState::default();
        state.flow.write().await.record_event("p".into(), "a/t".into(), crate::model::now_ms(), 5.0);
        state.topology.write().await.record_publish_declaration("p".into(), "a/t".into(), crate::model::now_ms());

        let first = shared_snapshot_build(&state).await;
        let second = shared_snapshot_build(&state).await;
        assert!(std::sync::Arc::ptr_eq(&first.edges, &second.edges), "a second viewer reuses the build");
        assert_eq!(first.edges.len(), 1);

        // Age the build past its lifetime: the next request rebuilds.
        state.snapshot_build.lock().await.as_mut().unwrap().built_at_ms -= 10 * SNAPSHOT_BUILD_MIN_TTL_MS;
        let third = shared_snapshot_build(&state).await;
        assert!(!std::sync::Arc::ptr_eq(&first.edges, &third.edges), "an expired build is rebuilt");
    }

    #[tokio::test]
    async fn concurrent_requests_share_one_build() {
        let state = crate::zenoh_client::SharedState::default();
        let builds = futures::future::join_all((0..8).map(|_| shared_snapshot_build(&state))).await;
        assert!(builds.windows(2).all(|w| std::sync::Arc::ptr_eq(&w[0].nodes, &w[1].nodes)));
    }

    #[test]
    fn fold_singletons_collapses_overthreshold_groups() {
        // 10 singletons under one namespace from one publisher — folds.
        let mut edges: Vec<EdgeView> = Vec::new();
        for i in 0..10 {
            edges.push(synthetic_edge(
                "publisher-zid",
                &format!("vendor-c/fleet/entities/v3/entity-{i}"),
                1,
            ));
        }

        let folded = fold_singletons_in_edges(edges);
        assert_eq!(folded.len(), 1, "10 singletons under threshold(7) → 1 folded edge");
        let e = &folded[0];
        assert_eq!(e.source, "publisher-zid");
        assert_eq!(e.folded_singletons_count, Some(10));
        assert!(e.topic.starts_with("vendor-c/fleet/entities/v3/* (folded x10)"));
        assert_eq!(e.total_events, 10);
    }

    /// A chunk shape: one topic per chunk part under nested
    /// per-stream / per-chunk directories, each topic delivered to several
    /// wildcard subscribers, one event per topic.
    fn chunked_stream_edges(streams: usize, chunks: usize, parts: usize, sinks: usize) -> Vec<EdgeView> {
        let mut edges = Vec::new();
        for s in 0..streams {
            for c in 0..chunks {
                for p in 0..parts {
                    let topic = format!("anchor-a/imagery/v1/stream/s{s}/chunk/{c:04}/{p:06}");
                    for k in 0..sinks {
                        let mut e = synthetic_edge("pub-chunks", &topic, 1);
                        e.ema_eps = 0.5;
                        e.sink = Some(format!("sub-{k}"));
                        edges.push(e);
                    }
                }
            }
        }
        edges
    }

    #[test]
    fn outlier_publisher_rolls_up_flows_into_one_edge() {
        // 3 streams × 20 chunks × 5 parts = 300 topics, × 6 subscribers = 1,800
        // sink-resolved edges. The template fold skips all of them.
        let mut edges = chunked_stream_edges(3, 20, 5, 6);
        edges.push(synthetic_edge("small-pub", "anchor-b/status/v1", 4));
        let folded = fold_singletons_in_edges(edges);

        let rolled: Vec<&EdgeView> = folded.iter().filter(|e| e.source == "pub-chunks").collect();
        assert_eq!(rolled.len(), 1, "the whole publisher, flows included, is one edge");
        let r = rolled[0];
        assert_eq!(r.sink, None);
        assert_eq!(r.folded_singletons_count, Some(300), "distinct topics, not topic × subscriber");
        assert_eq!(r.folded_namespace.as_deref(), Some("anchor-a/imagery/v1/stream/**"));
        assert_eq!(r.topic, "anchor-a/imagery/v1/stream/** (folded x300)");
        assert_eq!(r.total_events, 300, "summed once per topic, not per subscriber copy");
        assert!((r.ema_eps - 150.0).abs() < 1e-9);

        let small: Vec<&EdgeView> = folded.iter().filter(|e| e.source == "small-pub").collect();
        assert_eq!(small.len(), 1);
        assert_eq!(small[0].folded_singletons_count, None, "a small publisher is untouched");
    }

    #[test]
    fn many_sinkless_topics_in_one_template_still_template_fold() {
        // 500 per-entity topics with no subscriber fold into ONE template edge:
        // one wire key, so no rollup — the existing fold is unchanged.
        let edges: Vec<EdgeView> = (0..500)
            .map(|i| synthetic_edge("pub-entities", &format!("anchor-c/entities/v3/e{i}"), 3))
            .collect();
        let folded = fold_singletons_in_edges(edges);
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].folded_namespace.as_deref(), Some("anchor-c/entities/v3/*"));
    }

    #[test]
    fn a_publisher_with_many_subscribers_but_few_topics_is_not_an_outlier() {
        // 3 topics × 40 subscribers = 120 edges. Topic count, not edge count,
        // decides — a popular small publisher keeps its flows.
        let mut edges = Vec::new();
        for t in 0..3 {
            for k in 0..40 {
                let mut e = synthetic_edge("release", &format!("release/REL-ALL/t{t}/v1"), 9);
                e.sink = Some(format!("sub-{k}"));
                edges.push(e);
            }
        }
        let folded = fold_singletons_in_edges(edges);
        assert_eq!(folded.len(), 120);
        assert!(folded.iter().all(|e| e.folded_singletons_count.is_none()));
    }

    #[test]
    fn outlier_without_a_common_prefix_is_left_alone() {
        // No shared leading segment → no anchored key a scoped viewer could be
        // granted, so the rollup does not fire.
        let mut edges = Vec::new();
        for i in 0..60 {
            for k in 0..30 {
                let mut e = synthetic_edge("pub-mixed", &format!("org{i}/x/y{i}/z"), 1);
                e.sink = Some(format!("s{k}"));
                edges.push(e);
            }
        }
        let folded = fold_singletons_in_edges(edges);
        assert_eq!(folded.len(), 1800);
    }

    #[test]
    fn a_small_payload_publisher_keeps_per_topic_rows() {
        // `ops/fabric` : 41 topics, 247 edges. Over the topic
        // threshold, under the row floor — per-topic rows stay for `goat flow`.
        let mut edges = Vec::new();
        for t in 0..41 {
            for k in 0..6 {
                let mut e = synthetic_edge("ops/fabric", &format!("ops/fabric/footprint/p{t}"), 5);
                e.sink = Some(format!("obs-{k}"));
                edges.push(e);
            }
        }
        let folded = fold_singletons_in_edges(edges);
        assert_eq!(folded.len(), 246);
        assert!(folded.iter().all(|e| e.folded_singletons_count.is_none()));
    }

    #[test]
    fn rollup_matches_whole_segments_only() {
        assert!(rollup_matches("a/b/**", "a/b/c"));
        assert!(rollup_matches("a/b/**", "a/b"));
        assert!(!rollup_matches("a/b/**", "a/bc/d"));
        assert!(!rollup_matches("a/*/c", "a/b/c"), "a template is not a rollup");
        assert_eq!(common_segment_prefix(&["a/b/c", "a/b/d/e"]), "a/b");
        assert_eq!(common_segment_prefix(&["a/b", "c/d"]), "");
    }

    #[test]
    fn fold_singletons_keeps_small_groups() {
        // 3 singletons — below threshold, all kept.
        let mut edges: Vec<EdgeView> = Vec::new();
        for i in 0..3 {
            edges.push(synthetic_edge("publisher-zid", &format!("ns/t{i}"), 1));
        }
        let folded = fold_singletons_in_edges(edges);
        assert_eq!(folded.len(), 3, "3 singletons stay un-folded");
        assert!(folded.iter().all(|e| e.folded_singletons_count.is_none()));
    }

    #[test]
    fn aggressive_rollup_bundles_whole_publisher() {
        // A wide-shallow publisher: 3 distinct one-off topics (each below the
        // namespace-fold threshold) the blunt collapse would leave scattered.
        let edges = vec![
            synthetic_edge("pub-a", "a/one", 2),
            synthetic_edge("pub-a", "b/two", 3),
            synthetic_edge("pub-a", "c/three", 5),
        ];
        let rolled = aggressive_rollup(edges);
        assert_eq!(rolled.len(), 1, "whole publisher → one bundle edge");
        let e = &rolled[0];
        assert_eq!(e.source, "pub-a");
        assert_eq!(e.folded_singletons_count, Some(3), "3 distinct one-offs counted");
        assert_eq!(e.folded_namespace.as_deref(), Some(AGGRESSIVE_ALL_NAMESPACE));
        assert_eq!(e.total_events, 10, "events summed across the bundle");
    }

    #[test]
    fn aggressive_rollup_counts_namespace_folds_and_preserves_flows() {
        // A namespace-fold edge stands for N topics; the bundle must count N,
        // not 1. And a pub→sub flow must pass through unbundled.
        let mut nsfold = synthetic_edge("pub-b", "ns/* (folded x20)", 20);
        nsfold.folded_singletons_count = Some(20);
        let oneoff = synthetic_edge("pub-b", "misc/solo", 1);
        let mut flow = synthetic_edge("pub-b", "ns/derived", 4);
        flow.sink = Some("subscriber".to_string());

        let rolled = aggressive_rollup(vec![nsfold, oneoff, flow]);
        // Expect one bundle (the two fanout edges) + the untouched flow.
        assert_eq!(rolled.len(), 2);
        let bundle = rolled.iter().find(|e| e.sink.is_none()).unwrap();
        assert_eq!(bundle.folded_singletons_count, Some(21), "20 + 1 one-off");
        assert!(rolled.iter().any(|e| e.sink.as_deref() == Some("subscriber")));
    }

    #[test]
    fn fold_includes_multi_event_edges() {
        // Multi-event fanout fold case — the original `total_events == 1`
        // gate was dropped because some publishers emit 2-12
        // events per entity, which the singleton gate skipped, defeating
        // the bound. 10 multi-event edges under one namespace now fold
        // by structural cardinality regardless of event count.
        let mut edges: Vec<EdgeView> = Vec::new();
        for i in 0..10 {
            edges.push(synthetic_edge(
                "pub",
                &format!("sensorgrid/v1/entities/{i}"),
                5, // multi-event — would have been skipped by the old gate
            ));
        }
        let folded = fold_singletons_in_edges(edges);
        assert_eq!(folded.len(), 1, "10 multi-event edges over threshold → 1 folded");
        let e = &folded[0];
        assert_eq!(e.folded_singletons_count, Some(10));
        assert_eq!(e.total_events, 50, "aggregated total_events = 10 × 5");
        assert!(e.topic.contains("sensorgrid/v1/entities"));
    }

    #[test]
    fn fold_groups_singleton_and_multi_event_together() {
        // Mixed event-count edges under one (publisher, namespace) fold
        // as a single group — the heuristic is structural, not event-count
        // based, after the fix.
        let mut edges: Vec<EdgeView> = Vec::new();
        for i in 0..5 {
            edges.push(synthetic_edge(
                "pub",
                &format!("mixed-ns/init-only-{i}"),
                1, // singletons
            ));
        }
        for i in 0..5 {
            edges.push(synthetic_edge(
                "pub",
                &format!("mixed-ns/updated-{i}"),
                7, // multi-event
            ));
        }
        let folded = fold_singletons_in_edges(edges);
        assert_eq!(folded.len(), 1, "10 mixed-event edges over threshold → 1 folded");
        let e = &folded[0];
        assert_eq!(e.folded_singletons_count, Some(10));
        assert_eq!(e.total_events, 5 + 35, "aggregated 5×1 + 5×7");
    }

    #[test]
    fn fold_singletons_groups_per_publisher() {
        // Two publishers, each with 10 singletons under the same namespace.
        // Each gets its own folded edge (publisher is part of the group key).
        let mut edges: Vec<EdgeView> = Vec::new();
        for pub_id in ["pub-a", "pub-b"] {
            for i in 0..10 {
                edges.push(synthetic_edge(pub_id, &format!("multipub/ns/{i}"), 1));
            }
        }
        let folded = fold_singletons_in_edges(edges);
        assert_eq!(folded.len(), 2);
        let pubs: std::collections::HashSet<_> =
            folded.iter().map(|e| e.source.clone()).collect();
        assert!(pubs.contains("pub-a"));
        assert!(pubs.contains("pub-b"));
    }

    #[test]
    fn fold_singletons_preserves_pub_sub_edges() {
        // Edges with sink (subscriber-resolved) are NOT folded — folding
        // would conflate per-topic subscriber relationships.
        let mut e = synthetic_edge("pub", "ns/t-with-sub", 1);
        e.sink = Some("subscriber-zid".to_string());
        let folded = fold_singletons_in_edges(vec![e]);
        assert_eq!(folded.len(), 1);
        assert!(folded[0].folded_singletons_count.is_none());
    }

    #[test]
    fn fold_publishes_collapses_overthreshold_namespaces() {
        // 12 topics under one namespace + 2 elsewhere — fold the 12; keep
        // the 2 individual.
        let mut publishes: Vec<String> = (0..12)
            .map(|i| format!("sensorgrid/v1/entities/entity-{i}"))
            .collect();
        publishes.push("misc/t-1".to_string());
        publishes.push("misc/t-2".to_string());
        let folded = fold_publishes(&publishes);
        assert_eq!(folded.len(), 3, "1 fold marker + 2 misc");
        assert!(folded.iter().any(|s| s.starts_with("sensorgrid/v1/entities/* (folded x12)")));
        assert!(folded.iter().any(|s| s == "misc/t-1"));
        assert!(folded.iter().any(|s| s == "misc/t-2"));
    }

    #[test]
    fn fold_publishes_keeps_small_namespaces_individual() {
        let publishes = vec!["a/t1".to_string(), "a/t2".to_string(), "b/t1".to_string()];
        let folded = fold_publishes(&publishes);
        assert_eq!(folded.len(), 3);
        assert!(folded.iter().all(|s| !s.contains("folded x")));
    }

    #[test]
    fn folded_edge_carries_namespace_for_lazy_expand() {
        // The folded synthetic edge must carry `folded_namespace` so the
        // frontend can build the /api/snapshot/expand?source=&namespace=
        // URL without parsing it back out of the topic string. Verifies
        // the field is populated with the exact namespace key the fold
        // grouped on (NOT the formatted display topic).
        let mut edges: Vec<EdgeView> = Vec::new();
        for i in 0..10 {
            edges.push(synthetic_edge("pub", &format!("ns/sub/entity-{i}"), 3));
        }
        let folded = fold_singletons_in_edges(edges);
        assert_eq!(folded.len(), 1);
        let e = &folded[0];
        // `folded_namespace` is now the fold TEMPLATE (varying segment → `*`),
        // not the strip-last-segment prefix. For a trailing-id pattern that is
        // `<prefix>/*`. The frontend passes this verbatim to
        // /api/snapshot/expand, which re-matches members via `template_matches`.
        assert_eq!(e.folded_namespace.as_deref(), Some("ns/sub/*"));
        assert!(e.topic.contains("ns/sub/*"));
    }

    #[test]
    fn fold_collapses_varying_segment_in_non_terminal_position() {
        // Version-after-entity: `<anchor>/aircraft/<entity-id>/v1` —
        // version-after-entity, so the high-cardinality id is the 3rd of 4
        // segments and the LAST segment is the constant `v1`. The old
        // strip-last-segment grouping keyed each topic to its own namespace
        // (`<anchor>/aircraft/<id>`) and nothing folded. The template fold
        // wildcards the varying segment wherever it sits.
        let mut edges: Vec<EdgeView> = Vec::new();
        for i in 0..270 {
            edges.push(synthetic_edge("pub-a", &format!("15f1/aircraft/{i:06x}/v1"), 4));
        }
        let folded = fold_singletons_in_edges(edges);
        assert_eq!(folded.len(), 1, "270 entity-in-middle topics → 1 folded");
        let e = &folded[0];
        assert_eq!(e.folded_singletons_count, Some(270));
        assert_eq!(e.folded_namespace.as_deref(), Some("15f1/aircraft/*/v1"));
        assert!(e.topic.contains("15f1/aircraft/*/v1"));
        // The expand path must re-select every member from the template alone.
        assert!(template_matches("15f1/aircraft/*/v1", "15f1/aircraft/0000ff/v1"));
        assert!(!template_matches("15f1/aircraft/*/v1", "15f1/aircraft/0000ff/v2"));
        assert!(!template_matches("15f1/aircraft/*/v1", "15f1/sensors/0000ff/v1"));
    }

    #[test]
    fn fold_collapses_two_families_varying_at_different_positions() {
        // One publisher, same segment count, TWO
        // fan-out families whose varying segment is at DIFFERENT positions.
        // v1 single-axis folded only the bigger (purpleair) and scattered the
        // other (weather); greedy multi-axis folds BOTH.
        let mut edges: Vec<EdgeView> = Vec::new();
        for i in 0..20 {
            edges.push(synthetic_edge("g", &format!("g/air/purpleair/sensor-{i}/v1"), 3)); // varies pos3
        }
        for c in ["oslo", "kaunas", "vilnius", "riga", "tallinn", "helsinki", "klaipeda", "siauliai", "panevezys", "kaliningrad"] {
            edges.push(synthetic_edge("g", &format!("g/weather/{c}/forecast/v1"), 3)); // varies pos2
        }
        let folded = fold_singletons_in_edges(edges);
        let templates: std::collections::HashSet<_> =
            folded.iter().filter_map(|e| e.folded_namespace.clone()).collect();
        assert!(templates.contains("g/air/purpleair/*/v1"), "purpleair family folds");
        assert!(templates.contains("g/weather/*/forecast/v1"), "weather family ALSO folds (multi-axis)");
        assert_eq!(folded.len(), 2, "both families fold, nothing left scattered");
    }

    #[test]
    fn fold_keeps_distinct_subfamilies_separate() {
        // Same publisher, same segment count, two families differing in a
        // non-varying segment: each folds to its own template, not one blob.
        let mut edges: Vec<EdgeView> = Vec::new();
        for i in 0..20 {
            edges.push(synthetic_edge("p", &format!("a/aircraft/{i}/v1"), 2));
            edges.push(synthetic_edge("p", &format!("a/sensors/{i}/v1"), 2));
        }
        let folded = fold_singletons_in_edges(edges);
        assert_eq!(folded.len(), 2, "two families → two folded templates");
        let templates: std::collections::HashSet<_> =
            folded.iter().filter_map(|e| e.folded_namespace.clone()).collect();
        assert!(templates.contains("a/aircraft/*/v1"));
        assert!(templates.contains("a/sensors/*/v1"));
    }

    /// F-147 fixture: synthesize a router admin upsert + a subscriber
    /// admin upsert into a clean topology and verify both render in the
    /// snapshot view with the right `node_kind` + non-synthetic posture
    /// (router/peer/subscriber kinds carry no schema-decl, so synthetic
    /// stays None — that's the signal the frontend uses to put them in
    /// the REAL bucket).
    #[test]
    fn admin_upserts_router_and_subscriber_render_in_snapshot() {
        let mut topology = Topology::default();

        // Router self-admin sample
        topology.upsert_admin_peer(
            "router0aabbcc11".to_string(),
            Some(NodeKind::Router),
            Some("tcp/198.51.100.7:7447".to_string()),
            100,
        );

        // Subscriber-only peer (audit-subscriber shape): subscribes on **
        topology.upsert_admin_peer(
            "audit0deadbeef".to_string(),
            Some(NodeKind::Subscriber),
            None,
            100,
        );
        topology.record_subscribe_declaration(
            "audit0deadbeef".to_string(),
            "**".to_string(),
            100,
        );

        let nodes = build_nodes_view(&topology, &panoscope_core::AliasRegistry::empty());
        assert_eq!(nodes.len(), 2, "router + subscriber should both render");

        let router = nodes
            .iter()
            .find(|n| n.peer_id == "router0aabbcc11")
            .expect("router node missing");
        assert_eq!(router.node_kind, Some(NodeKind::Router));
        assert_eq!(router.locator.as_deref(), Some("tcp/198.51.100.7:7447"));
        assert_eq!(router.attest_source.as_deref(), Some("admin"));
        assert_eq!(router.synthetic, None, "admin-only nodes are not synthetic");
        assert_eq!(router.node_class, NodeClass::Infra, "router is infrastructure");
        assert_eq!(router.node_subkind, Some("router"));

        let sub = nodes
            .iter()
            .find(|n| n.peer_id == "audit0deadbeef")
            .expect("subscriber node missing");
        assert_eq!(sub.node_kind, Some(NodeKind::Subscriber));
        assert_eq!(sub.subscribes, vec!["**".to_string()]);
        assert!(sub.publishes.is_empty(), "subscriber-only has no publishes");
        assert_eq!(sub.role, Role::Sink);
        assert_eq!(sub.node_class, NodeClass::Infra, "audit-subscriber is infrastructure");
        assert_eq!(sub.node_subkind, Some("subscriber"));
    }

    #[test]
    fn slot_alias_rendered_for_known_anchor_only() {
        use panoscope_core::{AliasRegistry, AliasRow};
        // Two publishers: one whose peer_id is a known slot anchor, one a
        // random ZID with no alias.
        const ANCHOR: &str = "e7a9c75b532e3d434bb09fefd3ce1ec7";
        let mut topology = Topology::default();
        topology.record_publish_declaration(ANCHOR.to_string(), format!("{ANCHOR}/obs/v1"), 100);
        topology.record_publish_declaration(
            "randomzid00112233".to_string(),
            "randomzid00112233/obs/v1".to_string(),
            100,
        );

        let registry = AliasRegistry::from_rows([AliasRow {
            slot_id: ANCHOR.to_string(),
            alias: "pat-operator".into(),
        }]);
        let nodes = build_nodes_view(&topology, &registry);

        let anchored = nodes.iter().find(|n| n.peer_id == ANCHOR).expect("anchor node");
        assert_eq!(
            anchored.slot_alias.as_deref(),
            Some("pat-operator"),
            "a known anchor projects its alias"
        );
        // peer_id stays the canonical UUID — alias is display-only (§3).
        assert_eq!(anchored.peer_id, ANCHOR);

        let random = nodes.iter().find(|n| n.peer_id == "randomzid00112233").expect("zid node");
        assert_eq!(random.slot_alias, None, "an unknown id has no alias");

        // An empty/degraded registry never produces an alias.
        let degraded = build_nodes_view(&topology, &AliasRegistry::empty());
        assert!(
            degraded.iter().all(|n| n.slot_alias.is_none()),
            "empty registry → every node renders its UUID, no alias"
        );
    }

    #[test]
    fn write_root_alias_names_zid_keyed_publisher() {
        use panoscope_core::{AliasRegistry, AliasRow};
        // F-278: publishers are usually keyed by their Zenoh session ZID
        // (SourceInfo), which the alias registry can never know — but the
        // slot they write into is the leading segment of every published
        // key. A single aliased write root names the publisher.
        const SLOT: &str = "19c3d52b6fc91c015343b6cee6eeb6d7";
        const OTHER_SLOT: &str = "e7a9c75b532e3d434bb09fefd3ce1ec7";
        const ZID: &str = "4c8dc42f42354e5a6fc2cc6facee8ee5";
        let mut topology = Topology::default();
        topology.record_publish_declaration(ZID.to_string(), format!("{SLOT}/patrol/log"), 100);
        topology.record_publish_declaration(ZID.to_string(), format!("{SLOT}/patrol/status"), 100);

        // Publisher writing into TWO distinct slot roots — ambiguous, stays hex.
        topology.record_publish_declaration(
            "ambizid000000000".to_string(),
            format!("{SLOT}/a/v1"),
            100,
        );
        topology.record_publish_declaration(
            "ambizid000000000".to_string(),
            format!("{OTHER_SLOT}/b/v1"),
            100,
        );

        // Publisher under a multipub/ root (multi-publisher class, ADR 1105) —
        // leading segment is not anchor-shaped, so no attribution.
        topology.record_publish_declaration(
            "sharedzid0000000".to_string(),
            "multipub/org-pool/topic/v1".to_string(),
            100,
        );

        let registry = AliasRegistry::from_rows([
            AliasRow { slot_id: SLOT.to_string(), alias: "org-demo-alias".into() },
            AliasRow { slot_id: OTHER_SLOT.to_string(), alias: "pat-operator".into() },
        ]);
        let nodes = build_nodes_view(&topology, &registry);

        let bridged = nodes.iter().find(|n| n.peer_id == ZID).expect("zid publisher");
        assert_eq!(
            bridged.slot_alias.as_deref(),
            Some("org-demo-alias"),
            "single aliased write root names the ZID-keyed publisher"
        );
        assert_eq!(bridged.peer_id, ZID, "peer_id stays the canonical ZID (§3)");

        let ambiguous = nodes.iter().find(|n| n.peer_id == "ambizid000000000").unwrap();
        assert_eq!(ambiguous.slot_alias, None, "two slot roots → no attribution");

        let shared = nodes.iter().find(|n| n.peer_id == "sharedzid0000000").unwrap();
        assert_eq!(shared.slot_alias, None, "multipub/ root never attributes");
    }

    #[test]
    fn peer_id_alias_outranks_write_root() {
        use panoscope_core::{AliasRegistry, AliasRow};
        // A publisher whose own peer_id is an aliased anchor keeps its own
        // name even when publishing into another slot's root.
        const OWN: &str = "e7a9c75b532e3d434bb09fefd3ce1ec7";
        const FOREIGN: &str = "19c3d52b6fc91c015343b6cee6eeb6d7";
        let mut topology = Topology::default();
        topology.record_publish_declaration(OWN.to_string(), format!("{FOREIGN}/relay/v1"), 100);

        let registry = AliasRegistry::from_rows([
            AliasRow { slot_id: OWN.to_string(), alias: "pat-operator".into() },
            AliasRow { slot_id: FOREIGN.to_string(), alias: "org-demo-alias".into() },
        ]);
        let nodes = build_nodes_view(&topology, &registry);
        let own = nodes.iter().find(|n| n.peer_id == OWN).expect("own node");
        assert_eq!(
            own.slot_alias.as_deref(),
            Some("pat-operator"),
            "peer_id lookup wins over the write-root fallback"
        );
    }

    #[test]
    fn universal_observer_flagged_and_excluded_from_fanout() {
        use crate::sample_buffer::SampleBuffer;
        let topic = "uuid/tarsier/alpha-fob/contacts/v1";
        let mut topology = Topology::default();
        // Publisher of `topic`.
        topology.record_publish_declaration("pub-zid".to_string(), topic.to_string(), 100);
        // Universal `**` observer (omniwatch-shaped).
        topology
            .upsert_peer("omniwatch-zid".to_string(), 100)
            .subscribes
            .insert("**".to_string());
        // Specific subscriber to the topic (a real consumer).
        topology
            .upsert_peer("fusion-zid".to_string(), 100)
            .subscribes
            .insert(topic.to_string());

        // NodeView flag: the `**` observer is flagged, the specific one isn't.
        let nodes = build_nodes_view(&topology, &panoscope_core::AliasRegistry::empty());
        assert!(
            nodes.iter().find(|n| n.peer_id == "omniwatch-zid").unwrap().universal_observer,
            "** subscriber is a universal observer"
        );
        assert!(
            !nodes.iter().find(|n| n.peer_id == "fusion-zid").unwrap().universal_observer,
            "a specific subscriber is not"
        );

        // Edges: the specific subscriber gets a fan-out edge; the universal
        // observer does NOT (no per-topic edge spam).
        let mut flow = FlowState::default();
        flow.record_event("pub-zid".to_string(), topic.to_string(), 1000, 5.0);
        let samples = SampleBuffer::default();
        let edges = build_raw_edges(&flow, &topology, &samples, 1000);
        let sinks: Vec<String> = edges.iter().filter_map(|e| e.sink.clone()).collect();
        assert!(sinks.contains(&"fusion-zid".to_string()), "specific subscriber gets an edge");
        assert!(
            !sinks.contains(&"omniwatch-zid".to_string()),
            "universal observer must NOT get a per-topic fan-out edge"
        );
    }

    /// F-147 fixture: when an admin sample lands first and a schema-decl
    /// arrives later, the richer schema-decl metadata wins but the admin-
    /// derived node_kind + locator persist.
    #[test]
    fn schema_decl_wins_but_admin_substrate_facts_survive() {
        let mut topology = Topology::default();

        topology.upsert_admin_peer(
            "peer123abcdef0".to_string(),
            Some(NodeKind::Peer),
            Some("tcp/10.0.0.5:7447".to_string()),
            100,
        );

        // Simulate schema-decl arriving second
        let p = topology.upsert_peer("peer123abcdef0".to_string(), 200);
        p.vendor = Some("caracal".to_string());
        p.site = Some("alpha-fob".to_string());
        p.role_class = Some("sensor".to_string());
        p.synthetic = Some(true);
        p.attest_source = Some("schema_decl".to_string());

        let nodes = build_nodes_view(&topology, &panoscope_core::AliasRegistry::empty());
        let n = nodes.iter().find(|n| n.peer_id == "peer123abcdef0").unwrap();
        // Schema-decl wins
        assert_eq!(n.vendor.as_deref(), Some("caracal"));
        assert_eq!(n.synthetic, Some(true));
        assert_eq!(n.attest_source.as_deref(), Some("schema_decl"));
        // Admin facts survive
        assert_eq!(n.node_kind, Some(NodeKind::Peer));
        assert_eq!(n.locator.as_deref(), Some("tcp/10.0.0.5:7447"));
        // A plain Zenoh peer that's a real vendor is an application
        // participant — NodeKind::Peer alone is not a substrate signal.
        assert_eq!(n.node_class, NodeClass::App);
        assert_eq!(n.node_subkind, None);
    }

    /// F-147 follow-up: when a session-transport admin reply with no
    /// `whatami` arrives first (the /link/ leaf carries `{src,dst}` only),
    /// followed by the parent transport reply with `whatami=router`, the
    /// later sample must promote the node-kind to Router. Mirrors the
    /// reply-order non-determinism of a live router.
    #[test]
    fn admin_upsert_promotes_peer_to_router_when_whatami_arrives_later() {
        let mut topology = Topology::default();

        // First sample: /link/ leaf, locator-only, no whatami
        topology.upsert_admin_peer(
            "routerzid000001".to_string(),
            None,
            Some("tls/172.21.0.3:60270".to_string()),
            100,
        );
        // After this we have node_kind=None
        assert_eq!(
            topology.peers.get("routerzid000001").unwrap().node_kind,
            None
        );

        // Second sample: parent transport with whatami=router
        topology.upsert_admin_peer(
            "routerzid000001".to_string(),
            Some(NodeKind::Router),
            None,
            200,
        );
        assert_eq!(
            topology.peers.get("routerzid000001").unwrap().node_kind,
            Some(NodeKind::Router)
        );

        // Third sample: another /link/ leaf without whatami — must NOT
        // demote the classification.
        topology.upsert_admin_peer(
            "routerzid000001".to_string(),
            None,
            None,
            300,
        );
        assert_eq!(
            topology.peers.get("routerzid000001").unwrap().node_kind,
            Some(NodeKind::Router)
        );

        // Fourth sample: a stray Peer classification (shouldn't happen
        // on the wire, but verify Router doesn't get demoted by a later
        // Peer either).
        topology.upsert_admin_peer(
            "routerzid000001".to_string(),
            Some(NodeKind::Peer),
            None,
            400,
        );
        assert_eq!(
            topology.peers.get("routerzid000001").unwrap().node_kind,
            Some(NodeKind::Router),
            "Router classification must survive a later Peer sample"
        );
    }

    /// The hygiene endpoint now reads the view model instead of rebuilding
    /// the raw edge graph under the ingest locks. The verdict must not change:
    /// same codes, same probe selectors, as the raw-edge path produced.
    #[test]
    fn hygiene_from_the_view_model_matches_the_raw_edge_path() {
        let now = 10 * 3_600_000;
        let mut topology = Topology::default();
        let mut flow = FlowState::default();
        let anchor = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        // A fan-out publisher (one topic per entity) with a wildcard
        // subscriber, plus a well-shaped publisher.
        for i in 0..40 {
            let key = format!("{anchor}/aircraft/{i:04}/v1");
            topology.record_publish_declaration("fanout".into(), key.clone(), now - 1_000);
            flow.record_event("fanout".into(), key, now - 1_000, 5.0);
        }
        topology.record_subscribe_declaration("sink".into(), format!("{anchor}/aircraft/**"), now);
        topology.record_publish_declaration("tidy".into(), "bbbb/status/v1".into(), now - 1_000);
        flow.record_event("tidy".into(), "bbbb/status/v1".into(), now - 1_000, 5.0);

        let samples = SampleBuffer::default();
        let raw_stats = publisher_topic_stats_from(&build_raw_edges(&flow, &topology, &samples, now));
        let mut idx = crate::view::subs::SubscriptionIndex::default();
        let model = crate::view::model::build_model(
            &topology,
            &flow,
            &|_| None,
            &panoscope_core::AliasRegistry::empty(),
            &mut idx,
            &crate::view::model::FabricModel::empty(),
            now,
        );
        for source in ["fanout", "tidy", "unknown-peer"] {
            let old = panoscope_core::classify_publisher(
                &raw_stats.get(source).cloned().unwrap_or_default(),
            );
            let new = crate::view::service::publisher_hygiene(&model, source);
            let codes = |h: &panoscope_core::PublisherHygiene| {
                let mut c: Vec<String> = h.findings.iter().map(|f| f.code.clone()).collect();
                c.sort();
                c
            };
            let mut old_sel = old.probe_selectors.clone();
            let mut new_sel = new.probe_selectors.clone();
            old_sel.sort();
            new_sel.sort();
            assert_eq!(old.verdict, new.verdict, "{source}");
            assert_eq!(codes(&old), codes(&new), "{source}");
            assert_eq!(old_sel, new_sel, "{source}");
        }
        assert!(
            !crate::view::service::publisher_hygiene(&model, "fanout").findings.is_empty(),
            "the fan-out publisher has findings, so the comparison is not vacuous"
        );
    }

    /// The shared build runs on the blocking pool; it must still produce the
    /// same result on a multi-thread runtime while ingest keeps writing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shared_build_does_not_hold_ingest_locks_while_building() {
        let state = crate::zenoh_client::SharedState::default();
        {
            let mut flow = state.flow.write().await;
            let mut topo = state.topology.write().await;
            for i in 0..2_000 {
                let key = format!("a/t/{i}");
                flow.record_event("p".into(), key.clone(), crate::model::now_ms(), 5.0);
                topo.record_publish_declaration("p".into(), key, crate::model::now_ms());
            }
        }
        // clippy.toml forbids `tokio::spawn` workspace-wide because goat-relay
        // pins its hot-path tasks to one shard for life (ADR 0709 D7). This is
        // a test in a different crate, and the point of it is that the build
        // runs on ANOTHER thread while this one takes the ingest lock — a
        // `spawn_local` would run on this thread and prove nothing.
        #[allow(clippy::disallowed_methods)]
        let build = tokio::spawn({
            let state = state.clone();
            async move { shared_snapshot_build(&state).await }
        });
        // A writer taking the flow lock repeatedly completes promptly.
        for i in 0..50 {
            let w = tokio::time::timeout(std::time::Duration::from_secs(2), state.flow.write()).await;
            let mut flow = w.expect("flow write lock is never held across a build");
            flow.record_event("q".into(), format!("b/{i}"), crate::model::now_ms(), 5.0);
        }
        let b = build.await.expect("build task");
        assert!(!b.edges.is_empty());
    }
}
