// SPDX-License-Identifier: Apache-2.0
//
// Subscription index — topic → subscribers, maintained incrementally.
//
// The v1 snapshot matched every flow edge against every peer's subscribe
// patterns on every request, parsing both key expressions each time
// (O(edges × peers × patterns) per poll, seconds on a large fabric).
// The answer only changes when a topic appears or a peer's
// subscription set changes, so this index keeps the result and updates it
// for exactly those two events:
//
//   - a NEW topic is matched once against every subscriber's patterns;
//   - a peer whose subscription set CHANGED (or that appeared or left) is
//     removed from, then re-matched against, every known topic.
//
// Matching semantics are identical to `snapshot_view::keyexpr_matches`:
// Zenoh's own `includes` when both strings are canonical key expressions,
// the minimal literal rules otherwise. Universal `**` observers are never
// indexed as subscribers (they would draw a per-topic edge to every topic;
// see `NodeView.universal_observer`).

use std::collections::{HashMap, HashSet};

use zenoh::key_expr::{keyexpr, OwnedKeyExpr};

use crate::model::{PeerId, Topology};

/// One parsed subscribe pattern. Parsed once per pattern, not per match.
enum Pattern {
    Ke(OwnedKeyExpr),
    Raw(String),
}

impl Pattern {
    fn parse(s: &str) -> Self {
        match OwnedKeyExpr::new(s) {
            Ok(k) => Pattern::Ke(k),
            Err(_) => Pattern::Raw(s.to_string()),
        }
    }

    fn matches(&self, topic_ke: Option<&keyexpr>, topic: &str) -> bool {
        match (self, topic_ke) {
            (Pattern::Ke(p), Some(t)) => p.includes(t),
            (Pattern::Ke(p), None) => raw_matches(p.as_str(), topic),
            (Pattern::Raw(p), _) => raw_matches(p, topic),
        }
    }
}

/// The fallback rules `keyexpr_matches` applies when a string is not a
/// canonical key expression.
fn raw_matches(pattern: &str, topic: &str) -> bool {
    if pattern == "**" || pattern == topic {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return topic == prefix
            || (topic.len() > prefix.len()
                && topic.starts_with(prefix)
                && topic.as_bytes()[prefix.len()] == b'/');
    }
    false
}

struct PeerPatterns {
    fingerprint: u64,
    patterns: Vec<Pattern>,
}

impl PeerPatterns {
    fn matches(&self, topic_ke: Option<&keyexpr>, topic: &str) -> bool {
        self.patterns.iter().any(|p| p.matches(topic_ke, topic))
    }
}

#[derive(Default)]
pub struct SubscriptionIndex {
    peers: HashMap<PeerId, PeerPatterns>,
    topic_subs: HashMap<String, Vec<PeerId>>,
}

/// Counters from one `update`, surfaced on the view's build stats so the
/// incremental claim is observable rather than asserted.
#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct IndexUpdate {
    pub new_topics: usize,
    pub dropped_topics: usize,
    pub changed_peers: usize,
}

impl SubscriptionIndex {
    /// Bring the index in line with the current topology and topic set.
    pub fn update<'a>(
        &mut self,
        topology: &Topology,
        topics: impl Iterator<Item = &'a str>,
    ) -> IndexUpdate {
        let mut stats = IndexUpdate::default();
        let current_topics: HashSet<&str> = topics.collect();

        // Topics that disappeared.
        let before = self.topic_subs.len();
        self.topic_subs.retain(|t, _| current_topics.contains(t.as_str()));
        stats.dropped_topics = before - self.topic_subs.len();

        // Current indexable subscribers and their fingerprints.
        let mut current_peers: HashMap<&str, u64> = HashMap::new();
        for p in topology.peers.values() {
            if p.subscribes.is_empty() || p.subscribes.iter().any(|s| s == "**") {
                continue;
            }
            current_peers.insert(p.peer_id.as_str(), fingerprint(p.subscribes.iter()));
        }

        // Peers that left or whose subscription set changed: unindex.
        let stale: Vec<PeerId> = self
            .peers
            .iter()
            .filter(|(id, pp)| current_peers.get(id.as_str()) != Some(&pp.fingerprint))
            .map(|(id, _)| id.clone())
            .collect();
        if !stale.is_empty() {
            let stale_set: HashSet<&str> = stale.iter().map(String::as_str).collect();
            for subs in self.topic_subs.values_mut() {
                subs.retain(|s| !stale_set.contains(s.as_str()));
            }
            for id in &stale {
                self.peers.remove(id);
            }
        }

        // Peers to (re-)index.
        let mut added: Vec<PeerId> = Vec::new();
        for (id, fp) in &current_peers {
            if self.peers.contains_key(*id) {
                continue;
            }
            let p = &topology.peers[*id];
            let mut subs: Vec<&String> = p.subscribes.iter().collect();
            subs.sort();
            self.peers.insert(
                (*id).to_string(),
                PeerPatterns {
                    fingerprint: *fp,
                    patterns: subs.iter().map(|s| Pattern::parse(s)).collect(),
                },
            );
            added.push((*id).to_string());
        }
        stats.changed_peers = added.len() + stale.iter().filter(|s| !added.contains(s)).count();

        // Match added peers against topics already indexed.
        if !added.is_empty() {
            for (topic, subs) in self.topic_subs.iter_mut() {
                let ke = keyexpr::new(topic.as_str()).ok();
                for id in &added {
                    if self.peers[id].matches(ke, topic) {
                        subs.push(id.clone());
                    }
                }
            }
        }

        // Match new topics against every indexed peer.
        for topic in current_topics {
            if self.topic_subs.contains_key(topic) {
                continue;
            }
            stats.new_topics += 1;
            let ke = keyexpr::new(topic).ok();
            let mut subs: Vec<PeerId> = self
                .peers
                .iter()
                .filter(|(_, pp)| pp.matches(ke, topic))
                .map(|(id, _)| id.clone())
                .collect();
            subs.sort();
            self.topic_subs.insert(topic.to_string(), subs);
        }
        stats
    }

    /// Subscribers of `topic` (never universal observers). Empty when the
    /// topic is unknown to the index.
    pub fn subscribers_of(&self, topic: &str) -> &[PeerId] {
        self.topic_subs.get(topic).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Order-independent FNV-1a over a string set.
fn fingerprint<'a>(items: impl Iterator<Item = &'a String>) -> u64 {
    let mut v: Vec<&String> = items.collect();
    v.sort();
    let mut h = crate::view::Fnv::new();
    for s in v {
        h.write(s.as_bytes());
        h.write(&[0xff]);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot_view::keyexpr_matches;

    fn topo(subs: &[(&str, &[&str])]) -> Topology {
        let mut t = Topology::default();
        for (peer, pats) in subs {
            for p in *pats {
                t.record_subscribe_declaration((*peer).into(), (*p).into(), 1);
            }
        }
        t
    }

    #[test]
    fn matches_like_keyexpr_matches() {
        let pats = ["a/**", "a/*/c", "**/alpha-fob/**", "x/y", "not a keyexpr/**"];
        let topics = ["a/b/c", "a/b", "z/alpha-fob/t", "x/y", "x/y/z", "not a keyexpr/t", "a"];
        for p in pats {
            let pat = Pattern::parse(p);
            for t in topics {
                let ke = keyexpr::new(t).ok();
                assert_eq!(pat.matches(ke, t), keyexpr_matches(p, t), "pattern {p} topic {t}");
            }
        }
    }

    #[test]
    fn incremental_update_equals_full_rebuild() {
        let mut t = topo(&[("s1", &["a/**"]), ("s2", &["a/b/*"]), ("obs", &["**"])]);
        let mut idx = SubscriptionIndex::default();
        idx.update(&t, ["a/b/c", "a/x", "q/r"].into_iter());
        assert_eq!(idx.subscribers_of("a/b/c"), &["s1".to_string(), "s2".to_string()]);
        assert_eq!(idx.subscribers_of("q/r"), &[] as &[String]);

        // New topic, new peer, changed peer, departed peer.
        t.record_subscribe_declaration("s3".into(), "q/**".into(), 2);
        t.record_subscribe_declaration("s2".into(), "a/x".into(), 2);
        t.peers.remove("s1");
        let u = idx.update(&t, ["a/b/c", "a/x", "q/r", "q/new"].into_iter());
        assert_eq!(u.new_topics, 1);

        let mut full = SubscriptionIndex::default();
        full.update(&t, ["a/b/c", "a/x", "q/r", "q/new"].into_iter());
        for topic in ["a/b/c", "a/x", "q/r", "q/new"] {
            let mut a = idx.subscribers_of(topic).to_vec();
            let mut b = full.subscribers_of(topic).to_vec();
            a.sort();
            b.sort();
            assert_eq!(a, b, "topic {topic}");
        }
        assert!(idx.subscribers_of("a/b/c").iter().all(|s| s != "obs"), "universal observer never indexed");
    }
}
