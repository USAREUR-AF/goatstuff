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

use std::collections::{BTreeMap, BTreeSet};

use crate::directory::{Owner, Peer};
use crate::findings::{self, Finding, Known, Silent};
use crate::store::{Change, Store};
use crate::wire::{is_router_region, DeclarationsV1, RegionV1};

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
    /// Empty when the route table was read; otherwise why not, and
    /// `route_successor_count` is then unknown, not zero.
    pub route_table_note: &'a str,
    /// What the publisher could not read in this router's adminspace.
    pub shape_notes: &'a [String],
    /// `false` = known only from other routers' link-state; this router sends
    /// no snapshot of its own (a partner's router, or a host with no
    /// publisher). It is drawn so the picture is complete, and everything
    /// above about its settings and load is unknown, not zero.
    pub reporting: bool,
    /// For a router that is not reporting: the reporting routers whose
    /// link-state lists it. Empty for a reporting router.
    pub known_from: Vec<&'a str>,
    /// Whether a default client can hold a session with this router: `refused`
    /// raises T-013. `unspecified` makes no claim.
    pub client_admission: &'static str,
    /// The mesh address a reporting router sees this router's session come
    /// from. `None` for a router no reporter holds a session with.
    pub address: Option<&'a str>,
    /// The mesh peer at that address, from the peer directory.
    pub peer: Option<Peer>,
    /// Who the identity registry records for that peer (see
    /// [`crate::directory`]); `None` when the peer name leads to no roster
    /// handle.
    pub owner: Option<Owner>,
    /// Why there is no peer name, when there is none. Empty when there is.
    pub name_note: &'static str,
    /// This router's own declarations census. Reporting routers only; `None`
    /// from a publisher older than the census or run with it off.
    pub census: Option<&'a DeclarationsV1>,
    /// What a reporting router in the same routing group attributes to this
    /// one. `None` when no such reporter has a census.
    pub held: Option<HeldView<'a>>,
    /// Route-table entries per neighbour, keyed as edges are (node name or
    /// ZID). Reporting routers only.
    pub next_hops: Vec<HopView<'a>>,
}

/// The share of a reporter's subscriber table attributed to one router.
#[derive(Serialize)]
pub struct HeldView<'a> {
    /// The reporter whose census this is.
    pub by: &'a str,
    pub read_unix_ms: u64,
    pub subscribed_keys: u32,
    pub sample_keys: &'a [String],
    pub sample_truncated: bool,
}

#[derive(Serialize)]
pub struct HopView<'a> {
    pub to: &'a str,
    pub routes: u32,
}

/// What the page needs to say about the two identity inputs, once.
#[derive(Serialize)]
pub struct DirectoryView<'a> {
    pub peers_configured: bool,
    pub roster_configured: bool,
    pub errors: &'a [String],
    /// When the peer directory and the roster were last written (Unix ms).
    pub peers_modified_ms: u64,
    pub roster_modified_ms: u64,
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
    /// Every router drawn: `reporting + not_reporting`.
    pub routers: usize,
    pub reporting: usize,
    pub not_reporting: usize,
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
    pub directory: DirectoryView<'a>,
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

/// Is `new` a better label for a pair than `old`? A terminated link wins over a
/// routed one: the gateway sees a pod in `south:<id>:router` while the pod sees
/// the gateway in its own `north`, and the gateway's view is the true one —
/// the pod sits below it.
fn outranks(new: &str, old: &str) -> bool {
    old == "north" && new != "north"
}

/// Build the payload. Called once per ingest, from the write path.
///
/// # The routers on the page
///
/// Every router that publishes a snapshot, PLUS every router another
/// router's link-state lists that does not. The second set is the join this
/// function exists for: a router with no publisher of its own is still in its
/// neighbors' routing maps, so it is drawn from there and marked not reporting.
/// Only router regions count (`north`, `south:<id>:router`); a `:peer` or
/// `:client` region lists things that are not routers.
///
/// Nothing here asks a router anything. Every fact is from a snapshot already
/// in the store, so the page shows the whole fabric without a page view, or
/// this function, ever becoming fabric load (ADR 1123).
pub fn project<'a>(store: &'a Store, now_ms: u64) -> Snapshot<'a> {
    let known: Vec<Known<'a>> = store
        .routers()
        .values()
        .map(|e| Known {
            snap: &e.snap,
            age_secs: now_ms.saturating_sub(e.received_ms) / 1000,
        })
        .collect();

    let mut findings = findings::evaluate(&known);

    // zid -> display name, so edges read as node names rather than hex.
    let names: BTreeMap<&str, &str> = known
        .iter()
        .filter(|k| !k.snap.zid.is_empty())
        .map(|k| (k.snap.zid.as_str(), display_name(&k.snap.node, &k.snap.zid)))
        .collect();
    let name_of = |z: &'a str| -> &'a str { names.get(z).copied().unwrap_or(z) };

    // zid -> the mesh address a reporter sees its session come from. A fresh
    // reporter's view wins over a stale one's; otherwise the first reporter in
    // key order, so the choice is the same on every rebuild.
    let mut addr: BTreeMap<&'a str, (&'a str, bool)> = BTreeMap::new();
    for k in &known {
        let fresh = !k.is_stale();
        for s in &k.snap.sessions {
            let Some(dst) = s.link_dst.first() else {
                continue;
            };
            match addr.get(s.peer_zid.as_str()) {
                Some((_, true)) => {}
                Some((_, false)) if !fresh => {}
                _ => {
                    addr.insert(&s.peer_zid, (dst.as_str(), fresh));
                }
            }
        }
    }
    let dir = store.directory();
    let who = |z: &'a str| -> (Option<&'a str>, Option<Peer>, Option<Owner>, &'static str) {
        let Some((a, _)) = addr.get(z).copied() else {
            return (
                None,
                None,
                None,
                "known only through link state: no reporting router holds a session with it, \
                 so the address it connects from is not seen",
            );
        };
        if !dir.peers_configured {
            return (
                Some(a),
                None,
                None,
                "no peer directory is configured on this surface",
            );
        }
        match dir.peer_at(a) {
            Some(p) => {
                let o = dir.owner_of(&p.name);
                (Some(a), Some(p.clone()), o, "")
            }
            None => (
                Some(a),
                None,
                None,
                "the address it connects from is not in the peer directory",
            ),
        }
    };

    // zid -> what the freshest readable census among the reporters that list it
    // attributes to it. Only a reporter in the same routing group sees its
    // declarations, so a reporter that does not list a router says nothing
    // about it — not zero.
    let held = |z: &'a str| -> Option<HeldView<'a>> {
        let lists = |k: &Known<'a>| {
            k.snap
                .regions
                .iter()
                .filter(|r| is_router_region(&r.name))
                .any(|r| r.member_zids.iter().any(|m| m == z))
                || k.snap.sessions.iter().any(|s| s.peer_zid == z)
        };
        known
            .iter()
            .filter(|k| k.snap.zid != z && lists(k))
            .filter_map(|k| {
                let d = k.snap.declarations.as_ref()?;
                (d.read_error.is_empty() && d.read_unix_ms > 0).then_some((k, d))
            })
            .max_by_key(|(_, d)| d.read_unix_ms)
            .map(|(k, d)| {
                let r = d.routers.iter().find(|r| r.zid == z);
                HeldView {
                    by: display_name(&k.snap.node, &k.snap.zid),
                    read_unix_ms: d.read_unix_ms,
                    subscribed_keys: r.map_or(0, |r| r.subscribed_keys),
                    sample_keys: r.map_or(&[][..], |r| &r.sample_keys[..]),
                    sample_truncated: r.is_some_and(|r| r.sample_truncated),
                }
            })
    };

    // --- routers known from link-state that send no snapshot ---------------
    // zid -> (reporters that list it, whether any of those is fresh)
    let mut silent: BTreeMap<&'a str, (BTreeSet<&'a str>, bool)> = BTreeMap::new();
    let mut note = |z: &'a str, by: &'a str, fresh: bool| {
        if z.is_empty() || names.contains_key(z) {
            return;
        }
        let e = silent.entry(z).or_default();
        e.0.insert(by);
        e.1 |= fresh;
    };
    for k in &known {
        let by = display_name(&k.snap.node, &k.snap.zid);
        let fresh = !k.is_stale();
        for r in k.snap.regions.iter().filter(|r| is_router_region(&r.name)) {
            for z in &r.member_zids {
                note(z, by, fresh);
            }
            for l in &r.links {
                note(&l.a, by, fresh);
                note(&l.b, by, fresh);
            }
        }
        for s in k.snap.sessions.iter().filter(|s| s.whatami == "router") {
            note(&s.peer_zid, by, fresh);
        }
    }

    // --- edges: link-state pairs, then this router's own router sessions ----
    // A link-state pair covers routers two hops away, which no session does.
    // Sessions still count, for a publisher older than the link list.
    let mut pairs: BTreeMap<(&'a str, &'a str), &'a str> = BTreeMap::new();
    let mut add = |x: &'a str, y: &'a str, region: &'a str| {
        if x == y {
            return;
        }
        let key = if x <= y { (x, y) } else { (y, x) };
        match pairs.get(&key) {
            Some(old) if !outranks(region, old) => {}
            _ => {
                pairs.insert(key, region);
            }
        }
    };
    for k in &known {
        let from = display_name(&k.snap.node, &k.snap.zid);
        for r in k.snap.regions.iter().filter(|r| is_router_region(&r.name)) {
            for l in &r.links {
                add(name_of(&l.a), name_of(&l.b), &r.name);
            }
        }
        for s in k.snap.sessions.iter().filter(|s| s.whatami == "router") {
            add(from, name_of(&s.peer_zid), &s.region);
        }
    }
    let edges = pairs
        .into_iter()
        .map(|((a, b), region)| Edge {
            a: a.into(),
            b: b.into(),
            region: region.into(),
        })
        .collect();

    let silent_list: Vec<Silent<'a>> = silent
        .iter()
        .map(|(z, (by, _))| Silent {
            zid: z,
            known_from: by.iter().copied().collect(),
        })
        .collect();
    if let Some(f) = findings::not_reporting(&silent_list) {
        findings.push(f);
        findings::sort(&mut findings);
    }

    let mut counts = Counts {
        routers: known.len() + silent.len(),
        reporting: known.len(),
        not_reporting: silent.len(),
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

    let mut routers: Vec<RouterView<'a>> = known
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
            route_table_note: &k.snap.route_table_note,
            shape_notes: &k.snap.shape_notes,
            reporting: true,
            known_from: vec![],
            client_admission: k.snap.admission().label(),
            address: None,
            peer: None,
            owner: None,
            name_note: "",
            census: k.snap.declarations.as_ref(),
            held: held(&k.snap.zid),
            next_hops: k
                .snap
                .next_hops
                .iter()
                .map(|h| HopView {
                    to: name_of(&h.zid),
                    routes: h.routes,
                })
                .collect(),
        })
        .collect();
    // A reporting router is named by its own node name; the peer it connects
    // from, when another reporter sees it, still says who operates it.
    for r in routers.iter_mut() {
        let (a, p, o, _) = who(r.zid);
        r.address = a;
        r.peer = p;
        r.owner = o;
    }
    routers.extend(silent.into_iter().map(|(z, (by, fresh))| {
        let (address, peer, owner, name_note) = who(z);
        RouterView {
            site: "",
            node: "",
            zid: z,
            zenoh_version: "",
            gateway_mode: "unknown",
            gateway_read_error: "",
            observe_error: "",
            received_ms: 0,
            // Known only from reporters that have themselves gone quiet: the
            // router may be long gone, and the page must not paint it as present.
            stale: !fresh,
            regions: vec![],
            sessions_total: 0,
            route_successor_count: 0,
            route_successor_expected: 0,
            route_table_note: "",
            shape_notes: &[],
            reporting: false,
            known_from: by.into_iter().collect(),
            client_admission: "unspecified",
            address,
            peer,
            owner,
            name_note,
            census: None,
            held: held(z),
            next_hops: vec![],
        }
    }));

    Snapshot {
        watching_since_ms: store.started_ms(),
        routers,
        findings,
        changes: store.changes(),
        edges,
        counts,
        directory: DirectoryView {
            peers_configured: dir.peers_configured,
            roster_configured: dir.roster_configured,
            errors: &dir.errors,
            peers_modified_ms: dir.peers_modified_ms,
            roster_modified_ms: dir.roster_modified_ms,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::ingest;
    use crate::wire::{GatewayMode, LinkV1, RegionV1, RouterTopologyV1, SessionV1};

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
                ..Default::default()
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

    fn region(name: &str, members: &[&str], links: &[(&str, &str)]) -> RegionV1 {
        RegionV1 {
            name: name.into(),
            link_state_v: members.len() as u32,
            link_state_e: links.len() as u32,
            member_zids: members.iter().map(|s| s.to_string()).collect(),
            links: links
                .iter()
                .map(|(a, b)| LinkV1 {
                    a: a.to_string(),
                    b: b.to_string(),
                })
                .collect(),
            ..Default::default()
        }
    }

    fn router_session(peer: &str, region: &str) -> SessionV1 {
        SessionV1 {
            peer_zid: peer.into(),
            whatami: "router".into(),
            region: region.into(),
            link_dst: vec![],
        }
    }

    fn json(st: &Store) -> serde_json::Value {
        serde_json::from_slice(&st.cached().unwrap().body).unwrap()
    }

    /// A live goatnet's shape, in miniature: one reporting router whose north
    /// group holds routers it has sessions with AND one it reaches only through
    /// a neighbor. Before this change the page drew one router.
    #[test]
    fn every_router_in_a_reporters_link_state_is_drawn_and_marked_silent() {
        let mut core = with_clients("core-1", "c0c0", 3);
        core.regions = vec![
            region(
                "north",
                &["c0c0", "e1e1", "e2e2", "beyond"],
                &[("e1e1", "c0c0"), ("e2e2", "c0c0"), ("beyond", "e2e2")],
            ),
            // peers are not routers, and must not be drawn as routers
            region("south:0:peer", &["c0c0", "p1", "p2"], &[]),
        ];
        core.sessions.push(router_session("e1e1", "north"));
        core.sessions.push(router_session("e2e2", "north"));

        let mut st = Store::new(0);
        ingest(&mut st, "s/core-1".into(), core, 1_000);
        let j = json(&st);

        assert_eq!(j["counts"]["routers"], 4);
        assert_eq!(j["counts"]["reporting"], 1);
        assert_eq!(j["counts"]["not_reporting"], 3);
        let rs = j["routers"].as_array().unwrap();
        let beyond = rs
            .iter()
            .find(|r| r["zid"] == "beyond")
            .expect("two hops away");
        assert_eq!(beyond["reporting"], false);
        assert_eq!(beyond["known_from"][0], "core-1");
        assert!(
            !rs.iter().any(|r| r["zid"] == "p1"),
            "a peer is not a router"
        );

        let edges: Vec<(String, String)> = j["edges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["a"].as_str().unwrap().into(),
                    e["b"].as_str().unwrap().into(),
                )
            })
            .collect();
        assert!(
            edges.contains(&("beyond".into(), "e2e2".into())),
            "{edges:?}"
        );
        assert!(
            edges.contains(&("core-1".into(), "e1e1".into())),
            "named, not hex: {edges:?}"
        );
        assert_eq!(
            edges.len(),
            3,
            "a session and its link-state edge draw one line"
        );
        assert!(j["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["id"] == "T-012"));
    }

    #[test]
    fn a_silent_router_becomes_named_once_its_publisher_reports() {
        let mut a = with_clients("a", "aaaa", 0);
        a.regions = vec![region("north", &["aaaa", "bbbb"], &[("aaaa", "bbbb")])];
        let mut st = Store::new(0);
        ingest(&mut st, "s/a".into(), a, 1_000);
        assert_eq!(json(&st)["counts"]["not_reporting"], 1);

        let mut b = with_clients("b", "bbbb", 0);
        b.regions = vec![region("north", &["aaaa", "bbbb"], &[("aaaa", "bbbb")])];
        ingest(&mut st, "s/b".into(), b, 2_000);
        let j = json(&st);
        assert_eq!(j["counts"]["routers"], 2);
        assert_eq!(j["counts"]["not_reporting"], 0);
        assert_eq!(j["edges"].as_array().unwrap().len(), 1);
        assert_eq!(j["edges"][0]["a"], "a");
        assert_eq!(j["edges"][0]["b"], "b");
    }

    #[test]
    fn a_pod_below_a_gateway_draws_as_a_terminated_link_from_either_side() {
        // The gateway files the pod under south:0:router; the pod files the
        // gateway under its own north. The gateway is right: the pod is below it.
        let mut gw = with_clients("gw", "g1", 0);
        gw.regions = vec![
            region("north", &["g1", "core"], &[("g1", "core")]),
            region("south:0:router", &["g1", "pod"], &[("g1", "pod")]),
        ];
        let mut pod = with_clients("pod-1", "pod", 0);
        pod.regions = vec![region("north", &["pod", "g1"], &[("pod", "g1")])];
        for order in [
            [("s/gw", gw.clone()), ("s/pod", pod.clone())],
            [("s/pod", pod), ("s/gw", gw)],
        ] {
            let mut st = Store::new(0);
            for (i, (id, snap)) in order.into_iter().enumerate() {
                ingest(&mut st, id.into(), snap, 1_000 + i as u64);
            }
            let j = json(&st);
            let e = j["edges"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["a"] == "gw" && e["b"] == "pod-1")
                .expect("gw-pod edge")
                .clone();
            assert_eq!(e["region"], "south:0:router");
            assert_eq!(j["counts"]["not_reporting"], 1, "core is silent");
        }
    }

    #[test]
    fn a_router_known_only_from_a_stale_reporter_is_itself_stale() {
        let mut a = with_clients("a", "aaaa", 0);
        a.regions = vec![region("north", &["aaaa", "zzzz"], &[("aaaa", "zzzz")])];
        let mut st = Store::new(0);
        ingest(&mut st, "s/a".into(), a, 1_000);
        crate::view::testing::refresh(&mut st, 1_000 + 1_000_000);
        let j = json(&st);
        let z = j["routers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["zid"] == "zzzz")
            .unwrap()
            .clone();
        assert_eq!(z["stale"], true);
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

    /// A router that sends no report is named from the mesh peer its session
    /// comes from, and the roster says who operates it. One known only through
    /// link state keeps its derived name, and the payload says why.
    #[test]
    fn a_silent_router_is_named_from_the_peer_it_connects_from() {
        use crate::directory::testing::{directory, ROSTER};
        let relay = r#"{"names": {
            "sha-01": {"name": "machine-anvil-pod-7-rack2", "ip": "100.127.0.21", "host": "anvil-pod-7", "aff": "ZAGROS-PARTNER"},
            "sha-02": {"name": "peer-okafor-n-zagros-2026-10-02-laptop", "ip": "100.127.0.22", "host": "nb-laptop", "aff": ""}
        }}"#;
        let mut st = Store::new(0);
        st.set_directory(directory(relay, ROSTER, true));
        let mut r = with_clients("hub", "aa00", 0);
        r.regions = vec![region(
            "north",
            &["aa00", "bb11", "cc22", "dd33"],
            &[("aa00", "bb11"), ("aa00", "cc22"), ("cc22", "dd33")],
        )];
        let mut s1 = router_session("bb11", "north");
        s1.link_dst = vec!["tls/100.127.0.21:50508".into()];
        let mut s2 = router_session("cc22", "north");
        s2.link_dst = vec!["tls/100.127.0.22:41000".into()];
        r.sessions = vec![s1, s2];
        ingest(&mut st, "s/hub".into(), r, 1_000);
        let v = json(&st);
        let by = |z: &str| {
            v["routers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["zid"] == z)
                .unwrap()
                .clone()
        };
        let b = by("bb11");
        assert_eq!(b["peer"]["name"], "machine-anvil-pod-7-rack2");
        assert_eq!(b["peer"]["groups"][0], "ZAGROS-PARTNER");
        assert_eq!(b["owner"]["org"], "org-anvil");
        assert_eq!(b["owner"]["org_nation"], "AAA");
        assert_eq!(b["owner"]["individual"], "rivera-j");
        assert_eq!(b["address"], "tls/100.127.0.21:50508");
        let c = by("cc22");
        assert_eq!(c["owner"]["kind"], "person");
        assert!(
            c["owner"]["individual_nationality"].is_null(),
            "not recorded"
        );
        let d = by("dd33");
        assert!(d["peer"].is_null());
        assert!(d["name_note"]
            .as_str()
            .unwrap()
            .contains("only through link state"));
        assert_eq!(v["directory"]["peers_configured"], true);
    }

    /// Per-router counts come from the census of a reporter in the same
    /// routing group; a reporter whose census failed contributes nothing, and
    /// a router no census covers has no counts rather than zeros.
    #[test]
    fn what_each_router_carries_comes_from_a_readable_census() {
        use crate::wire::{DeclarationsV1, NextHopV1, RouterDeclarationsV1};
        let mut st = Store::new(0);
        let mut r = with_clients("hub", "aa00", 0);
        r.regions = vec![region("north", &["aa00", "bb11"], &[("aa00", "bb11")])];
        r.sessions = vec![router_session("bb11", "north")];
        r.next_hops = vec![NextHopV1 {
            zid: "bb11".into(),
            routes: 2,
        }];
        r.declarations = Some(DeclarationsV1 {
            read_unix_ms: 900,
            session_subscriptions: 4,
            topics: 7,
            routers: vec![RouterDeclarationsV1 {
                zid: "bb11".into(),
                subscribed_keys: 3,
                sample_keys: vec!["rig/b/sub/0".into()],
                sample_truncated: true,
            }],
            ..Default::default()
        });
        ingest(&mut st, "s/hub".into(), r, 1_000);
        // A second reporter, outside bb11's group, whose census failed.
        let mut lone = with_clients("lone", "ee44", 0);
        lone.declarations = Some(DeclarationsV1 {
            read_unix_ms: 950,
            read_error: "not read: census read timed out after 10s".into(),
            ..Default::default()
        });
        ingest(&mut st, "s/lone".into(), lone, 1_000);
        let v = json(&st);
        let by = |z: &str| {
            v["routers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["zid"] == z)
                .unwrap()
                .clone()
        };
        let b = by("bb11");
        assert_eq!(b["held"]["by"], "hub");
        assert_eq!(b["held"]["subscribed_keys"], 3);
        let hub = by("aa00");
        assert_eq!(hub["census"]["session_subscriptions"], 4);
        assert_eq!(hub["census"]["topics"], 7);
        assert_eq!(hub["next_hops"][0]["routes"], 2);
        let lone = by("ee44");
        assert!(
            lone["held"].is_null(),
            "no census covers it: absent, not zero"
        );
        assert!(lone["census"]["read_error"]
            .as_str()
            .unwrap()
            .starts_with("not read"));
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
