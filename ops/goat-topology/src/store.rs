// SPDX-License-Identifier: Apache-2.0
//! What the surface knows, and when it changed.
//!
//! # The one performance decision that matters
//!
//! **The projection is computed on WRITE, not on read.** A router publishes
//! every 30s; a browser polls every 15s and there may be several. The findings
//! engine, the graph edges and the counts are a pure function of what is stored,
//! so recomputing them per request would redo identical work for every viewer
//! and would put that work inside a lock every viewer contends for.
//!
//! So [`Store::ingest`] recomputes the projection once and keeps it serialized.
//! A request clones one `Arc` and returns the bytes. That also sharpens the
//! ADR 1123 property this component exists to honour: a page view now does
//! *literally* no work, rather than merely no fabric work.
//!
//! The version counter behind the `ETag` means an unchanged fabric answers `304`
//! and sends no body at all — which is the common case, because nothing about a
//! settled routing plane changes between polls.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Serialize;

use crate::wire::{GatewayMode, RouterTopologyV1};

/// Enough history to be useful during an incident without becoming a log.
pub const MAX_CHANGES: usize = 200;

/// A router's last snapshot, and what it looked like before.
pub struct Entry {
    pub snap: RouterTopologyV1,
    pub received_ms: u64,
    /// So a change can be NAMED rather than implied by a number moving.
    pub fingerprint: Fingerprint,
}

/// The parts of a snapshot whose change is worth telling an operator about.
///
/// Deliberately **not** the whole snapshot. `ts_unix_ms` moves every interval
/// and client sessions churn constantly; a feed that fires every 30s on every
/// router teaches people to ignore it, which is worse than not having one.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Fingerprint {
    gateway_mode: i32,
    subregions: usize,
    /// `name=V/E` per region, sorted — catches a region appearing, vanishing,
    /// or changing membership.
    regions: Vec<String>,
    /// Peer ZIDs of non-client sessions, sorted. A client arriving is traffic;
    /// a ROUTER arriving is a topology change.
    peers: Vec<String>,
    zenoh_version: String,
}

impl Fingerprint {
    pub fn of(s: &RouterTopologyV1) -> Self {
        let mut regions: Vec<String> = s
            .regions
            .iter()
            .map(|r| format!("{}={}/{}", r.name, r.link_state_v, r.link_state_e))
            .collect();
        regions.sort();
        let mut peers: Vec<String> = s
            .sessions
            .iter()
            .filter(|x| x.whatami != "client")
            .map(|x| x.peer_zid.clone())
            .collect();
        peers.sort();
        peers.dedup();
        Fingerprint {
            gateway_mode: s.gateway_mode,
            subregions: s.custom_subregions.len(),
            regions,
            peers,
            zenoh_version: s.zenoh_version.clone(),
        }
    }

    fn mode_label(v: i32) -> &'static str {
        GatewayMode::try_from(v)
            .unwrap_or(GatewayMode::Unspecified)
            .label()
    }

    /// Plain-language differences, or empty when nothing worth saying changed.
    pub fn diff(&self, next: &Fingerprint, names: &BTreeMap<String, String>) -> Vec<String> {
        let mut out = Vec::new();
        if self.gateway_mode != next.gateway_mode {
            out.push(format!(
                "gateway mode {} -> {}",
                Self::mode_label(self.gateway_mode),
                Self::mode_label(next.gateway_mode)
            ));
        }
        if self.subregions != next.subregions {
            out.push(format!(
                "declared subregions {} -> {}",
                self.subregions, next.subregions
            ));
        }
        if self.zenoh_version != next.zenoh_version && !self.zenoh_version.is_empty() {
            out.push(format!(
                "zenoh {} -> {}",
                self.zenoh_version, next.zenoh_version
            ));
        }
        let base = |r: &String| {
            r.split_once('=')
                .map(|(n, _)| n.to_string())
                .unwrap_or_default()
        };
        for r in &next.regions {
            if !self.regions.contains(r) {
                let n = base(r);
                if self.regions.iter().any(|o| base(o) == n) {
                    out.push(format!("region {r} resized"));
                } else {
                    out.push(format!("region {r} appeared"));
                }
            }
        }
        for r in &self.regions {
            let n = base(r);
            if !next.regions.iter().any(|o| base(o) == n) {
                out.push(format!("region {n} vanished"));
            }
        }
        let show = |z: &String| {
            names
                .get(z)
                .cloned()
                .unwrap_or_else(|| z[..z.len().min(8)].to_string())
        };
        for p in &next.peers {
            if !self.peers.contains(p) {
                out.push(format!("peered with {}", show(p)));
            }
        }
        for p in &self.peers {
            if !next.peers.contains(p) {
                out.push(format!("lost peer {}", show(p)));
            }
        }
        out
    }
}

/// One thing that happened, newest first.
#[derive(Clone, Serialize)]
pub struct Change {
    pub ts_unix_ms: u64,
    pub node: String,
    pub what: String,
}

/// The serialized projection, ready to hand to a request untouched.
#[derive(Clone)]
pub struct Cached {
    pub body: Arc<Vec<u8>>,
    /// Weak ETag from the version counter. An unchanged fabric answers 304.
    pub etag: String,
}

#[derive(Default)]
pub struct Store {
    /// Keyed `<site>/<node>` so two sites cannot collide on node name.
    routers: BTreeMap<String, Entry>,
    changes: Vec<Change>,
    started_ms: u64,
    version: u64,
    cached: Option<Cached>,
    /// Which routers were stale at the last rebuild. The refresh tick rebuilds
    /// ONLY when this set changes — see [`Store::refresh`].
    stale_set: std::collections::BTreeSet<String>,
}

impl Store {
    pub fn new(now_ms: u64) -> Self {
        Store {
            started_ms: now_ms,
            ..Default::default()
        }
    }

    pub fn started_ms(&self) -> u64 {
        self.started_ms
    }
    pub fn routers(&self) -> &BTreeMap<String, Entry> {
        &self.routers
    }
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }
    pub fn len(&self) -> usize {
        self.routers.len()
    }

    /// The bytes a request should return. `None` only before the first sample.
    pub fn cached(&self) -> Option<Cached> {
        self.cached.clone()
    }

    /// Record one router's snapshot and note what changed.
    ///
    /// Returns `true` when the caller should rebuild the projection. The store
    /// does not build it itself: that would make state depend on presentation,
    /// and the two would import each other.
    pub fn ingest(&mut self, id: String, snap: RouterTopologyV1, now_ms: u64) -> bool {
        let fp = Fingerprint::of(&snap);
        let node = if snap.node.is_empty() {
            snap.zid.clone()
        } else {
            snap.node.clone()
        };

        if let Some(prev) = self.routers.get(&id) {
            let names: BTreeMap<String, String> = self
                .routers
                .values()
                .map(|e| {
                    let n = if e.snap.node.is_empty() {
                        e.snap.zid.clone()
                    } else {
                        e.snap.node.clone()
                    };
                    (e.snap.zid.clone(), n)
                })
                .collect();
            for what in prev.fingerprint.diff(&fp, &names) {
                self.changes.insert(
                    0,
                    Change {
                        ts_unix_ms: now_ms,
                        node: node.clone(),
                        what,
                    },
                );
            }
            self.changes.truncate(MAX_CHANGES);
        }

        self.routers.insert(
            id,
            Entry {
                snap,
                received_ms: now_ms,
                fingerprint: fp,
            },
        );
        self.stale_set = self.compute_stale(now_ms);
        true
    }

    /// Surface staleness even when nothing publishes — a router going quiet IS
    /// the event, and it would otherwise never appear, because staleness is the
    /// absence of a write.
    ///
    /// **Rebuilds only when the stale set actually changes.** A tick that
    /// rebuilt unconditionally would mint a new ETag every few seconds on a
    /// fabric where nothing had happened, so every browser would re-fetch and
    /// re-render on every poll. With this check a settled fabric produces
    /// byte-identical payloads, every poll answers 304, and the page does
    /// nothing at all.
    pub fn refresh(&mut self, now_ms: u64) -> bool {
        if self.routers.is_empty() {
            return false;
        }
        let now_stale = self.compute_stale(now_ms);
        if now_stale != self.stale_set {
            self.stale_set = now_stale;
            return true;
        }
        false
    }

    fn compute_stale(&self, now_ms: u64) -> std::collections::BTreeSet<String> {
        self.routers
            .iter()
            .filter(|(_, e)| {
                let interval = if e.snap.interval_secs == 0 {
                    30
                } else {
                    e.snap.interval_secs as u64
                };
                now_ms.saturating_sub(e.received_ms) / 1000 > interval.saturating_mul(3)
            })
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// Install a freshly built projection. Called by the orchestrator after
    /// [`Store::ingest`] or [`Store::refresh`] reports a change.
    pub fn set_cached(&mut self, body: Vec<u8>) {
        self.version += 1;
        self.cached = Some(Cached {
            body: Arc::new(body),
            etag: format!("W/\"v{}\"", self.version),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::{ingest, refresh};
    use crate::wire::{RegionV1, SessionV1};

    fn router(node: &str, zid: &str) -> RouterTopologyV1 {
        RouterTopologyV1 {
            zid: zid.into(),
            node: node.into(),
            site: "zagros-sandbox".into(),
            zenoh_version: "v1.9.0".into(),
            interval_secs: 30,
            gateway_mode: GatewayMode::Auto as i32,
            regions: vec![RegionV1 {
                name: "north".into(),
                link_state_v: 1,
                link_state_e: 0,
                member_zids: vec![zid.into()],
                session_count: 0,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn a_quiet_tick_produces_no_change_and_a_peering_does() {
        let mut st = Store::new(0);
        let a = router("a", "aaaa");
        ingest(&mut st, "s/a".into(), a.clone(), 1_000);
        assert!(st.changes().is_empty(), "first sample is not a change");

        ingest(&mut st, "s/a".into(), a.clone(), 31_000);
        assert!(
            st.changes().is_empty(),
            "an identical tick must stay silent"
        );

        let mut b = a.clone();
        b.sessions.push(SessionV1 {
            peer_zid: "bbbb".into(),
            whatami: "router".into(),
            region: "north".into(),
            link_dst: vec![],
        });
        ingest(&mut st, "s/a".into(), b, 61_000);
        assert_eq!(st.changes().len(), 1);
        assert!(st.changes()[0].what.contains("peered with"));
    }

    #[test]
    fn client_churn_does_not_fill_the_feed() {
        let mut st = Store::new(0);
        let a = router("a", "aaaa");
        ingest(&mut st, "s/a".into(), a.clone(), 1_000);
        for i in 0..20 {
            let mut n = a.clone();
            n.sessions = (0..i)
                .map(|c| SessionV1 {
                    peer_zid: format!("client-{c}"),
                    whatami: "client".into(),
                    region: "south:0:client".into(),
                    link_dst: vec![],
                })
                .collect();
            ingest(&mut st, "s/a".into(), n, 2_000 + i as u64 * 1_000);
        }
        assert!(
            st.changes().is_empty(),
            "clients cost routing nothing; their churn must not be reported as topology"
        );
    }

    #[test]
    fn the_change_log_is_bounded() {
        let mut st = Store::new(0);
        let a = router("a", "aaaa");
        ingest(&mut st, "s/a".into(), a.clone(), 0);
        // Flip the mode back and forth; each flip is one change.
        for i in 0..(MAX_CHANGES + 50) {
            let mut n = a.clone();
            n.gateway_mode = if i % 2 == 0 {
                GatewayMode::Custom as i32
            } else {
                GatewayMode::Auto as i32
            };
            ingest(&mut st, "s/a".into(), n, i as u64);
        }
        assert_eq!(st.changes().len(), MAX_CHANGES, "log must not grow forever");
    }

    #[test]
    fn a_settled_fabric_produces_an_unchanging_payload() {
        // The whole point of the 304 path: if the bytes churn while nothing is
        // happening, every browser re-fetches and re-renders on every poll.
        let mut st = Store::new(0);
        ingest(&mut st, "s/a".into(), router("a", "aaaa"), 1_000);
        let first = st.cached().unwrap();

        for t in [11_000, 21_000, 31_000, 41_000] {
            refresh(&mut st, t);
        }
        let later = st.cached().unwrap();
        assert_eq!(
            first.etag, later.etag,
            "a quiet fabric must not mint new etags"
        );
        assert_eq!(first.body, later.body, "and the bytes must be identical");
    }

    #[test]
    fn crossing_into_stale_does_rebuild() {
        let mut st = Store::new(0);
        ingest(&mut st, "s/a".into(), router("a", "aaaa"), 0);
        let before = st.cached().unwrap();
        refresh(&mut st, 10_000); // still fresh
        assert_eq!(before.etag, st.cached().unwrap().etag);

        refresh(&mut st, 500_000); // now well past 3 intervals
        let after = st.cached().unwrap();
        assert_ne!(
            before.etag, after.etag,
            "going quiet is an event and must publish"
        );
    }

    #[test]
    fn the_projection_is_built_on_write_and_the_version_moves() {
        let mut st = Store::new(0);
        assert!(
            st.cached().is_none(),
            "nothing to serve before the first sample"
        );

        ingest(&mut st, "s/a".into(), router("a", "aaaa"), 1_000);
        let first = st.cached().expect("cached after ingest");
        assert!(!first.body.is_empty());

        ingest(&mut st, "s/a".into(), router("a", "aaaa"), 2_000);
        let second = st.cached().unwrap();
        assert_ne!(
            first.etag, second.etag,
            "every ingest must move the etag, or a client caches a stale age"
        );
    }
}
