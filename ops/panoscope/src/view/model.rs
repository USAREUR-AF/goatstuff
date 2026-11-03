// SPDX-License-Identifier: Apache-2.0
//
// Fabric model — one canonical, read-only picture of the fabric, rebuilt on a
// short tick and shared by every viewer.
//
// Requests never touch the ingest locks (topology / flow / samples). They read
// an `Arc<FabricModel>` swapped in by `spawn_model_builder`, so a slow or
// numerous client cannot stall the data tail, and N open tabs cost one build
// per tick instead of N builds per poll.
//
// What a build does, in order:
//   1. Nodes: `build_nodes_view_unfolded` (ghost-peer filter + slot alias;
//      no per-node publishes fold — the view never sends that list).
//   2. Topics: one record per (canonical publisher, topic). Flow edges are
//      keyed by session ZID, so a publisher that reconnected leaves one edge
//      per past session; the canonical publisher (schema-declared owner, else
//      the edge source) collapses them and the freshest session wins — the
//      same rule `dedup_canonical_edges` applied to the v1 wire.
//   3. Subscribers: from the incremental `SubscriptionIndex`.
//   4. Hygiene: the key-shape verdict per publisher, from the topic records.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::model::{FlowState, PeerId, Topology};
use crate::sample_buffer::SampleFormat;
use crate::snapshot_view::{build_nodes_view_unfolded, format_label, NodeHygiene, NodeView};
use crate::view::subs::{IndexUpdate, SubscriptionIndex};
use crate::view::Fnv;
use crate::zenoh_client::EMA_TAU_SECS;

/// One topic as published by one canonical publisher.
pub struct TopicRec {
    pub key: String,
    /// Index into `FabricModel::peers`.
    pub publisher: usize,
    pub projected_eps: f64,
    pub total_events: u64,
    pub last_event_ms: u64,
    pub schema_family: Option<String>,
    pub format: Option<String>,
    /// Indices into `FabricModel::peers`; never the publisher itself, never a
    /// universal `**` observer.
    pub subscribers: Vec<usize>,
}

/// Per-peer facts the view needs that `NodeView` does not carry in raw form.
pub struct PeerFacts {
    /// Leading segments of every topic this peer publishes (and the second
    /// segment of `volatile/<anchor>/…`), for the scoped-viewer anchor check.
    /// Built from the RAW publish set: `NodeView.publishes` is folded for
    /// display and cannot answer an authorization question.
    pub publish_roots: HashSet<String>,
    /// Indices into `FabricModel::topics` this peer publishes.
    pub topics: Vec<usize>,
    /// True when every topic this peer publishes is test data (see
    /// `is_test_topic`). False for a peer that publishes nothing.
    pub test_publisher: bool,
}

pub struct FabricModel {
    /// Changes only when the picture changed (see `fingerprint`).
    pub generation: u64,
    pub built_at_ms: u64,
    pub peers: Vec<NodeView>,
    pub facts: Vec<PeerFacts>,
    pub peer_index: HashMap<PeerId, usize>,
    pub topics: Vec<TopicRec>,
    pub sampling_drops: Vec<(String, u64)>,
    pub build: BuildStats,
    fingerprint: u64,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct BuildStats {
    pub build_ms: f64,
    /// Per-phase wall time, in the order `build_model` runs them.
    pub phase_ms: PhaseMs,
    pub flow_edges: usize,
    pub peers: usize,
    pub topics: usize,
    pub subscriber_links: usize,
    pub index: IndexUpdate,
}

impl FabricModel {
    pub fn empty() -> Self {
        FabricModel {
            generation: 0,
            built_at_ms: 0,
            peers: Vec::new(),
            facts: Vec::new(),
            peer_index: HashMap::new(),
            topics: Vec::new(),
            sampling_drops: Vec::new(),
            build: BuildStats::default(),
            fingerprint: 0,
        }
    }

    pub fn peer(&self, id: &str) -> Option<usize> {
        self.peer_index.get(id).copied()
    }
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct PhaseMs {
    pub nodes: f64,
    pub canonical: f64,
    pub index: f64,
    pub topics: f64,
    pub facts: f64,
    pub hygiene: f64,
    pub fingerprint: f64,
}

/// Build a model. `prev` supplies the generation counter: the generation
/// advances only when the content fingerprint differs, so an idle fabric
/// keeps answering `304 Not Modified`.
pub fn build_model(
    topology: &Topology,
    flow: &FlowState,
    latest_format: &dyn Fn(&str) -> Option<SampleFormat>,
    aliases: &panoscope_core::AliasRegistry,
    index: &mut SubscriptionIndex,
    prev: &FabricModel,
    now_ms: u64,
) -> FabricModel {
    let started = std::time::Instant::now();
    let mut phase = PhaseMs::default();
    let mut lap = std::time::Instant::now();
    let mark = |slot: &mut f64, lap: &mut std::time::Instant| {
        *slot = lap.elapsed().as_secs_f64() * 1000.0;
        *lap = std::time::Instant::now();
    };

    let peers = build_nodes_view_unfolded(topology, aliases);
    let peer_index: HashMap<PeerId, usize> =
        peers.iter().enumerate().map(|(i, n)| (n.peer_id.clone(), i)).collect();

    mark(&mut phase.nodes, &mut lap);
    // Canonical (publisher, topic) records, freshest session wins.
    let mut best: HashMap<(usize, &str), &crate::model::EdgeState> = HashMap::new();
    for e in flow.edges.values() {
        let canonical = topology
            .topic_to_publisher
            .get(&e.topic)
            .unwrap_or(&e.source);
        // A topic whose publisher is not a rendered node (evicted, or hidden
        // as a ghost with no canonical owner) has nothing to attach to.
        let Some(&pi) = peer_index.get(canonical) else { continue };
        let slot = best.entry((pi, e.topic.as_str())).or_insert(e);
        if e.last_event_ms > slot.last_event_ms {
            *slot = e;
        }
    }

    mark(&mut phase.canonical, &mut lap);
    let index_update = index.update(topology, best.keys().map(|(_, t)| *t));
    mark(&mut phase.index, &mut lap);

    let mut entries: Vec<((usize, &str), &crate::model::EdgeState)> = best.into_iter().collect();
    entries.sort_by(|a, b| a.0 .1.cmp(b.0 .1).then(a.0 .0.cmp(&b.0 .0)));

    let mut topics: Vec<TopicRec> = Vec::with_capacity(entries.len());
    let mut subscriber_links = 0usize;
    for ((pi, key), e) in entries {
        let subscribers: Vec<usize> = index
            .subscribers_of(key)
            .iter()
            .filter_map(|s| peer_index.get(s).copied())
            .filter(|&si| si != pi)
            .collect();
        subscriber_links += subscribers.len();
        topics.push(TopicRec {
            key: key.to_string(),
            publisher: pi,
            projected_eps: e.projected_eps(now_ms, EMA_TAU_SECS),
            total_events: e.total_events,
            last_event_ms: e.last_event_ms,
            schema_family: e.schema_family.clone(),
            format: latest_format(key).map(|f| format_label(f).to_string()),
            subscribers,
        });
    }

    mark(&mut phase.topics, &mut lap);
    let mut facts: Vec<PeerFacts> = peers
        .iter()
        .map(|n| {
            let raw = topology.peers.get(&n.peer_id);
            let publish_roots = raw
                .map(|p| p.publishes.iter().flat_map(|k| publish_roots_of(k)).collect())
                .unwrap_or_default();
            PeerFacts { publish_roots, topics: Vec::new(), test_publisher: false }
        })
        .collect();
    for (ti, t) in topics.iter().enumerate() {
        facts[t.publisher].topics.push(ti);
    }
    for f in facts.iter_mut() {
        f.test_publisher = !f.topics.is_empty() && f.topics.iter().all(|&ti| is_test_topic(&topics[ti].key));
    }

    mark(&mut phase.facts, &mut lap);
    // Hygiene verdict per publisher (the snapshot-path, key-shape half).
    let mut peers = peers;
    for (pi, f) in facts.iter().enumerate() {
        if f.topics.is_empty() {
            continue;
        }
        let stats: Vec<panoscope_core::TopicStat> = f
            .topics
            .iter()
            .map(|&ti| {
                let t = &topics[ti];
                panoscope_core::TopicStat {
                    key: t.key.clone(),
                    total_events: t.total_events,
                    has_schema: t.schema_family.is_some() || t.format.is_some(),
                }
            })
            .collect();
        let h = panoscope_core::classify_publisher(&stats);
        peers[pi].hygiene = Some(NodeHygiene {
            verdict: h.verdict,
            codes: h.findings.iter().map(|f| f.code.clone()).collect(),
        });
    }

    let mut sampling_drops: Vec<(String, u64)> =
        flow.sampling_drops.iter().map(|(k, v)| (k.clone(), *v)).collect();
    sampling_drops.sort();

    mark(&mut phase.hygiene, &mut lap);
    let fp = fingerprint(&peers, &topics, &sampling_drops);
    mark(&mut phase.fingerprint, &mut lap);
    let generation = if fp == prev.fingerprint && prev.generation > 0 {
        prev.generation
    } else {
        prev.generation + 1
    };

    let build = BuildStats {
        build_ms: started.elapsed().as_secs_f64() * 1000.0,
        phase_ms: phase,
        flow_edges: flow.edges.len(),
        peers: peers.len(),
        topics: topics.len(),
        subscriber_links,
        index: index_update,
    };
    FabricModel {
        generation,
        built_at_ms: now_ms,
        peers,
        facts,
        peer_index,
        topics,
        sampling_drops,
        build,
        fingerprint: fp,
    }
}

/// Leading authorization roots of a key: its first segment, plus the second
/// segment of a `volatile/<anchor>/…` key (ADR 1032 class).
fn publish_roots_of(key: &str) -> Vec<String> {
    let mut segs = key.split('/');
    let first = segs.next().unwrap_or("");
    let mut out = vec![first.to_string()];
    if first == "volatile" {
        if let Some(second) = segs.next() {
            out.push(second.to_string());
        }
    }
    out
}

/// Interim provenance classifier for test data (ADR 1103), identical to the
/// frontend's former `TEST_TOPIC_RE` = `/(^|\/)loadtest(-[a-z0-9]+)?(\/|$)/i`:
/// some segment is `loadtest` or `loadtest-<alnum>`, case-insensitive.
pub fn is_test_topic(key: &str) -> bool {
    key.split('/').any(|seg| {
        let s = seg.to_ascii_lowercase();
        match s.strip_prefix("loadtest") {
            Some("") => true,
            Some(rest) => rest
                .strip_prefix('-')
                .is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())),
            None => false,
        }
    })
}

/// Content fingerprint. Rates are rounded to 0.01 msg/s, the precision the
/// COP displays, so a decaying rate still advances the generation while
/// float noise does not.
fn fingerprint(peers: &[NodeView], topics: &[TopicRec], drops: &[(String, u64)]) -> u64 {
    let mut h = Fnv::new();
    for n in peers {
        h.write(n.peer_id.as_bytes());
        h.write_u64(n.last_seen_ms);
        h.write(n.slot_alias.as_deref().unwrap_or("").as_bytes());
        h.write(n.display_name.as_deref().unwrap_or("").as_bytes());
        h.write_u64(n.node_class as u64);
        h.write_u64(n.subscribes.len() as u64);
    }
    for t in topics {
        h.write(t.key.as_bytes());
        h.write_u64(t.publisher as u64);
        h.write_u64(t.total_events);
        h.write_u64(t.last_event_ms);
        h.write_u64((t.projected_eps * 100.0).round() as u64);
        h.write_u64(t.subscribers.len() as u64);
        h.write(t.format.as_deref().unwrap_or("").as_bytes());
    }
    for (k, v) in drops {
        h.write(k.as_bytes());
        h.write_u64(*v);
    }
    h.finish()
}


/// Shared handle the HTTP and WS layers read.
pub type ModelHandle = Arc<tokio::sync::RwLock<Arc<FabricModel>>>;

#[cfg(test)]
mod tests {
    use super::*;

    use crate::model::{FlowState, Topology};
    use crate::view::subs::SubscriptionIndex;

    /// A fabric of `n` topics from two publishers, with `broad` subscribers that
    /// each match every topic.
    ///
    /// `broad` IS THE PARAMETER THAT MATTERS and the original fixture hard-coded
    /// it to one. The topics phase builds a subscriber list per topic, so its
    /// cost is topics x matching-subscribers, not topics alone. A fleet shape
    /// runs to tens of subscribers per topic. A
    /// one-subscriber fixture models none of that and reports a per-topic
    /// constant that does not exist at fleet shape.
    fn fixture(n: usize, now: u64) -> (Topology, FlowState) {
        fixture_fanout(n, 1, now)
    }

    fn fixture_fanout(n: usize, broad: usize, now: u64) -> (Topology, FlowState) {
        let mut t = Topology::default();
        let mut f = FlowState::default();
        for i in 0..n {
            let (peer, key) = if i % 2 == 0 {
                ("quiet".to_string(), format!("q/entities/e{i:05}"))
            } else {
                ("busy".to_string(), format!("b/stream/s{i:05}"))
            };
            t.record_publish_declaration(peer.clone(), key.clone(), now);
            f.record_event(peer, key, now, 3.0);
        }
        // NOT "**": a universal observer is deliberately excluded from a topic's
        // subscriber list, so it produces zero links. Real fan-out comes from
        // broad-but-not-universal patterns, which is what these are.
        for s in 0..broad {
            let pat = if s % 2 == 0 { "q/**" } else { "b/**" };
            t.record_subscribe_declaration(format!("watcher{s:04}"), pat.into(), now);
        }
        (t, f)
    }

    /// What a build costs, and where. Ignored by default — it is a
    /// measurement, not a gate:
    ///   cargo test --release --bin panoscope build_timing -- --ignored --nocapture
    ///
    /// Every phase is a full pass over every topic, and very few records
    /// differ between two builds.
    ///
    /// Carrying the unchanged records forward by comparing them was tried and
    /// measured SLOWER — 354 ms against 233 ms — because deciding a record is
    /// unchanged touches the same data as rebuilding it, and a short-string
    /// allocation is cheap. Only NOT LOOKING can win, which means ingest has
    /// to say what changed rather than the build finding out.
    ///
    /// `build_cost_breakdown` sizes the mechanisms inside that build and finds
    /// no hot spot to attack: of 121 ms at 73,819 topics, the String copies
    /// every record makes are 5 ms and an `Arc<str>` would make them 1 ms, the
    /// canonical map's string hashing is 3 ms, and the sort is 11 ms. The rest
    /// is two or three hash lookups and one small allocation per topic, in
    /// each of six phases. Interning keys, reserving capacity or dropping the
    /// sort are all worth single-digit milliseconds.
    ///
    /// THAT "ABOUT 1.6 us PER TOPIC, SPREAD EVENLY" READING WAS AN ARTEFACT OF
    /// THE FIXTURE AND IS WITHDRAWN. The fixture declared ONE subscriber, so it
    /// produced few subscriber links for its topic count. A real fabric carries
    /// about 100x more at the same topic count, because the topics phase builds
    /// a subscriber list PER TOPIC and its cost is topics x matching-subscribers,
    /// not topics alone. Few of the peers with broad patterns have been seen
    /// recently: the build spends most of its time deciding which mostly-absent
    /// peers would be listening to each topic.
    ///
    /// `build_at_fleet_shape` varies that fan-out and is the honest harness.
    /// Measured on an M-series laptop at 82,620 topics:
    ///
    ///   broad subscribers   1 ->     41,310 links, topics phase  35 ms
    ///   broad subscribers  56 ->  2,313,360 links, topics phase 171 ms
    ///
    /// It still under-models peer COUNT, which is what the nodes, facts and
    /// hygiene phases scale with, so treat it as a floor.
    ///
    /// Two levers follow, and the cheaper one is not architectural: a peer
    /// unseen for a day still multiplies the cross-product every second, so
    /// the replay window and the ghost-peer filter decide most of this before
    /// any incremental rebuild does.
    #[test]
    #[ignore]
    fn build_timing() {
        let now = 1_800_000_000_000;
        let (t, f) = fixture(73_819, now);
        let quiet = now + 8 * 3_600_000;
        let mut idx = SubscriptionIndex::default();
        let aliases = panoscope_core::AliasRegistry::empty();
        let warm = build_model(&t, &f, &|_| None, &aliases, &mut idx, &FabricModel::empty(), quiet);

        let started = std::time::Instant::now();
        let m = build_model(&t, &f, &|_| None, &aliases, &mut idx, &warm, quiet);
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        eprintln!("topics={} build={:.0} ms phases={:?}", m.topics.len(), ms, m.build.phase_ms);
    }


    /// Where the build's time goes at a large fabric's size, by mechanism rather than by
    /// phase. Ignored — a measurement, not a gate:
    ///   cargo test --release --bin panoscope build_cost_breakdown -- --ignored --nocapture
    /// What subscriber fan-out costs, which the one-subscriber fixture hid.
    ///   cargo test --release --bin panoscope build_at_fleet_shape -- --ignored --nocapture
    #[test]
    #[ignore]
    fn build_at_fleet_shape() {
        let now = 1_800_000_000_000;
        let quiet = now + 8 * 3_600_000;
        let aliases = panoscope_core::AliasRegistry::empty();
        for (topics, broad) in [(82_620, 1usize), (82_620, 8), (82_620, 28), (82_620, 56)] {
            let (t, f) = fixture_fanout(topics, broad, now);
            let mut idx = SubscriptionIndex::default();
            let warm = build_model(&t, &f, &|_| None, &aliases, &mut idx, &FabricModel::empty(), quiet);
            let started = std::time::Instant::now();
            let m = build_model(&t, &f, &|_| None, &aliases, &mut idx, &warm, quiet);
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            eprintln!(
                "broad_subscribers={broad:3}  links={:9}  build={ms:7.0} ms  topics_phase={:7.1} ms",
                m.build.subscriber_links, m.build.phase_ms.topics
            );
        }
    }

    #[test]
    #[ignore]
    fn build_cost_breakdown() {
        let now = 1_800_000_000_000;
        let n = 73_819;
        let (t, f) = fixture(n, now);
        let quiet = now + 8 * 3_600_000;
        let keys: Vec<&str> = f.edges.values().map(|e| e.topic.as_str()).collect();
        let ms = |s: std::time::Instant| s.elapsed().as_secs_f64() * 1000.0;

        // 1. Allocating one String per topic key, twice (topics phase does it
        //    once, hygiene's TopicStat does it again).
        let s = std::time::Instant::now();
        let a: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
        let b: Vec<String> = a.iter().map(|k| k.clone()).collect();
        eprintln!("string alloc x2      {:.0} ms  ({} + {})", ms(s), a.len(), b.len());

        // 2. The same, if the key were an Arc<str> cloned instead of copied.
        let arcs: Vec<std::sync::Arc<str>> = keys.iter().map(|k| std::sync::Arc::from(*k)).collect();
        let s = std::time::Instant::now();
        let c: Vec<std::sync::Arc<str>> = arcs.iter().map(|k| k.clone()).collect();
        let d: Vec<std::sync::Arc<str>> = c.iter().map(|k| k.clone()).collect();
        eprintln!("Arc<str> clone x2    {:.0} ms  ({} + {})", ms(s), c.len(), d.len());

        // 3. Building the canonical `best` map: hashing every key string.
        let s = std::time::Instant::now();
        let mut m: HashMap<(usize, &str), u64> = HashMap::with_capacity(keys.len());
        for (i, k) in keys.iter().enumerate() { m.insert((i & 1, k), i as u64); }
        eprintln!("HashMap<&str> build  {:.0} ms  ({})", ms(s), m.len());

        // 4. The topics-phase sort over the same entries.
        let mut ent: Vec<((usize, &str), u64)> = m.into_iter().collect();
        let s = std::time::Instant::now();
        ent.sort_by(|x, y| x.0 .1.cmp(y.0 .1).then(x.0 .0.cmp(&y.0 .0)));
        eprintln!("sort                 {:.0} ms  ({})", ms(s), ent.len());

        // 5. The whole build, for the denominator.
        let mut idx = SubscriptionIndex::default();
        let aliases = panoscope_core::AliasRegistry::empty();
        let warm = build_model(&t, &f, &|_| None, &aliases, &mut idx, &FabricModel::empty(), quiet);
        let s = std::time::Instant::now();
        let m2 = build_model(&t, &f, &|_| None, &aliases, &mut idx, &warm, quiet);
        eprintln!("FULL BUILD           {:.0} ms  ({} topics) {:?}", ms(s), m2.topics.len(), m2.build.phase_ms);
    }

    #[test]
    fn test_topic_classifier_matches_the_former_frontend_regex() {
        for (k, want) in [
            ("loadtest", true),
            ("a/loadtest/b", true),
            ("a/LOADTEST-Wedge1/3", true),
            ("a/loadtest-/b", false),
            ("a/loadtests/b", false),
            ("a/myloadtest/b", false),
            ("a/loadtest-x_y/b", false),
            ("uuid/imagery/v1", false),
        ] {
            assert_eq!(is_test_topic(k), want, "{k}");
        }
    }

    #[test]
    fn roots_include_volatile_anchor() {
        assert_eq!(publish_roots_of("abc/x/y"), vec!["abc"]);
        assert_eq!(publish_roots_of("volatile/abc/x"), vec!["volatile", "abc"]);
    }
}
