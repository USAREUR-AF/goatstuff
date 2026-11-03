// SPDX-License-Identifier: Apache-2.0
//! The API projection — what the browser is actually given.
//!
//! Separate from [`crate::store`] because it is a pure function of the store,
//! and separate from [`crate::serve`] because HTTP should not know the shape of
//! the payload. It is computed once per ingest, never per request.
//!
//! # Send what the page reads, and nothing else
//!
//! The wire snapshot carries every session a router holds — one entry per
//! client at any scale. The page uses the *count*. It
//! also carries each region's full member list, which the graph does not read
//! because edges are resolved here.
//!
//! So neither is projected. That is not micro-optimisation: session lists are
//! the one part of this payload that grows with the fabric, and shipping them
//! would make the page heavier exactly as a goatnet gets busier, for data
//! nothing renders.

use serde::Serialize;

use crate::findings::{self, Finding, Known};
use crate::store::{Change, Store};
use crate::wire::RegionV1;

/// One region, as the page shows it.
#[derive(Serialize)]
pub struct RegionView<'a> {
    pub name: &'a str,
    pub link_state_v: u32,
    pub link_state_e: u32,
}

#[derive(Serialize)]
pub struct RouterView<'a> {
    pub site: &'a str,
    pub node: &'a str,
    pub zid: &'a str,
    pub zenoh_version: &'a str,
    pub gateway_mode: &'static str,
    pub gateway_read_error: &'a str,
    pub observe_error: &'a str,
    /// ABSOLUTE, not an age. An age would move every refresh tick, changing
    /// the payload — and therefore the ETag — while nothing had actually
    /// happened, which would defeat the 304 and force a full re-render every
    /// poll. The browser has a clock; let it subtract.
    pub received_ms: u64,
    /// Outranks everything: a router we cannot see is not healthy, and the page
    /// must not paint it as such.
    pub stale: bool,
    pub regions: Vec<RegionView<'a>>,
    pub sessions_total: usize,
    pub route_successor_count: u32,
    pub route_successor_expected: u32,
}

/// A router-to-router link, deduplicated so A→B and B→A draw one line.
#[derive(Serialize)]
pub struct Edge {
    pub a: String,
    pub b: String,
    pub region: String,
}

#[derive(Serialize, Default)]
pub struct Counts {
    pub routers: usize,
    pub stale: usize,
    pub clients: usize,
    pub critical: usize,
    pub warning: usize,
}

#[derive(Serialize)]
pub struct Snapshot<'a> {
    /// When this process started watching. The change feed cannot know about
    /// anything before it, and the page says so rather than implying quiet.
    ///
    /// There is deliberately no `generated_ms`: it would move on every rebuild
    /// and make every payload unique, defeating the 304. The page stamps its
    /// own "updated" time from the response it just received.
    pub watching_since_ms: u64,
    pub routers: Vec<RouterView<'a>>,
    pub findings: Vec<Finding>,
    pub changes: &'a [Change],
    pub edges: Vec<Edge>,
    pub counts: Counts,
}

fn display_name<'a>(node: &'a str, zid: &'a str) -> &'a str {
    if node.is_empty() {
        zid
    } else {
        node
    }
}

/// Build and serialize the payload in one step, for the orchestrator to install.
///
/// Lives here rather than in the store because it is presentation: the store
/// holds what is true, this decides what is shown. Keeping the direction
/// one-way — view reads store, never the reverse — is what stops the two
/// growing into each other.
pub fn build(store: &Store, now_ms: u64) -> Vec<u8> {
    serde_json::to_vec(&project(store, now_ms)).unwrap_or_else(|_| b"{}".to_vec())
}

/// Build the payload. Called once per ingest, from the write path.
pub fn project<'a>(store: &'a Store, now_ms: u64) -> Snapshot<'a> {
    let known: Vec<Known<'a>> = store
        .routers()
        .values()
        .map(|e| Known {
            snap: &e.snap,
            age_secs: now_ms.saturating_sub(e.received_ms) / 1000,
        })
        .collect();

    let findings = findings::evaluate(&known);

    // zid -> display name, so edges read as node names rather than hex.
    let names: std::collections::BTreeMap<&str, &str> = known
        .iter()
        .map(|k| (k.snap.zid.as_str(), display_name(&k.snap.node, &k.snap.zid)))
        .collect();

    let mut edges = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for k in &known {
        let from = display_name(&k.snap.node, &k.snap.zid);
        for s in &k.snap.sessions {
            if s.whatami == "client" {
                continue;
            }
            let to = names
                .get(s.peer_zid.as_str())
                .copied()
                .unwrap_or(&s.peer_zid);
            let (a, b) = if from <= to { (from, to) } else { (to, from) };
            if seen.insert((a.to_string(), b.to_string())) {
                edges.push(Edge {
                    a: a.into(),
                    b: b.into(),
                    region: s.region.clone(),
                });
            }
        }
    }

    let mut counts = Counts {
        routers: known.len(),
        ..Default::default()
    };
    for k in &known {
        if k.is_stale() {
            counts.stale += 1;
        }
        counts.clients += k
            .snap
            .sessions
            .iter()
            .filter(|s| s.whatami == "client")
            .count();
    }
    for f in &findings {
        match f.severity {
            findings::Severity::Critical => counts.critical += 1,
            findings::Severity::Warning => counts.warning += 1,
            _ => {}
        }
    }

    let routers = known
        .iter()
        .map(|k| RouterView {
            site: &k.snap.site,
            node: &k.snap.node,
            zid: &k.snap.zid,
            zenoh_version: &k.snap.zenoh_version,
            gateway_mode: k.snap.mode().label(),
            gateway_read_error: &k.snap.gateway_read_error,
            observe_error: &k.snap.observe_error,
            received_ms: now_ms.saturating_sub(k.age_secs * 1000),
            stale: k.is_stale(),
            regions: k
                .snap
                .regions
                .iter()
                .map(|r: &RegionV1| RegionView {
                    name: &r.name,
                    link_state_v: r.link_state_v,
                    link_state_e: r.link_state_e,
                })
                .collect(),
            sessions_total: k.snap.sessions.len(),
            route_successor_count: k.snap.route_successor_count,
            route_successor_expected: k.snap.expected_successors(),
        })
        .collect();

    Snapshot {
        watching_since_ms: store.started_ms(),
        routers,
        findings,
        changes: store.changes(),
        edges,
        counts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::ingest;
    use crate::wire::{GatewayMode, RegionV1, RouterTopologyV1, SessionV1};

    fn with_clients(node: &str, zid: &str, clients: usize) -> RouterTopologyV1 {
        RouterTopologyV1 {
            zid: zid.into(),
            node: node.into(),
            site: "zagros-sandbox".into(),
            interval_secs: 30,
            gateway_mode: GatewayMode::Auto as i32,
            regions: vec![RegionV1 {
                name: "north".into(),
                link_state_v: 1,
                link_state_e: 0,
                member_zids: vec![zid.into()],
                session_count: 0,
            }],
            sessions: (0..clients)
                .map(|i| SessionV1 {
                    peer_zid: format!("c{i}"),
                    whatami: "client".into(),
                    region: "south:0:client".into(),
                    link_dst: vec![],
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn session_lists_never_reach_the_browser() {
        // The one part of this payload that grows with the fabric. The page
        // reads the count; shipping the list would make the page heavier
        // exactly as a goatnet gets busier, for data nothing renders.
        let mut st = Store::new(0);
        ingest(&mut st, "s/a".into(), with_clients("a", "aaaa", 200), 1_000);
        let body = st.cached().unwrap().body;
        let json = String::from_utf8_lossy(&body);

        assert!(json.contains("\"sessions_total\":200"), "the count is sent");
        assert!(!json.contains("\"peer_zid\""), "the session list is not");
        assert!(!json.contains("\"member_zids\""), "member lists are not");
    }

    #[test]
    fn payload_size_is_flat_in_client_count() {
        let mut small = Store::new(0);
        ingest(
            &mut small,
            "s/a".into(),
            with_clients("a", "aaaa", 1),
            1_000,
        );
        let mut big = Store::new(0);
        ingest(
            &mut big,
            "s/a".into(),
            with_clients("a", "aaaa", 500),
            1_000,
        );

        let s = small.cached().unwrap().body.len();
        let b = big.cached().unwrap().body.len();
        assert!(
            b < s + 40,
            "payload grew {} bytes for 499 more clients; it should be flat",
            b - s
        );
    }
}

/// Test-only helpers so every suite drives the same ingest-then-build path the
/// server does, rather than each one re-implementing the orchestration.
#[cfg(test)]
pub mod testing {
    use crate::store::Store;
    use crate::wire::RouterTopologyV1;

    pub fn ingest(st: &mut Store, id: String, snap: RouterTopologyV1, now_ms: u64) {
        if st.ingest(id, snap, now_ms) {
            let body = super::build(st, now_ms);
            st.set_cached(body);
        }
    }

    pub fn refresh(st: &mut Store, now_ms: u64) {
        if st.refresh(now_ms) {
            let body = super::build(st, now_ms);
            st.set_cached(body);
        }
    }
}
