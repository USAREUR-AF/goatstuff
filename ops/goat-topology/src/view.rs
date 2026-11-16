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

use crate::directory::{Directory, Owner, OwnerRead, Peer};
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
    /// Where this router's owner stands (ADR 1249): `recorded` (the record
    /// or a declaration names a responsible person and organization), `lead`
    /// (the roster or the name only), `not_read` (the lookup failed; the
    /// reason says why), `no_record` (the record holds no owner: a finding),
    /// `excepted` (an owner exception the operator granted: shown with its
    /// reason, never as recorded), `incomplete` (the record names an owner
    /// but no organization, or a disabled person: a finding).
    pub owner_state: &'static str,
    pub owner_reason: String,
    /// For `not_read`, why: see [`Graded::cause`]. Empty otherwise.
    pub owner_cause: &'static str,
    /// This router's own declarations census. Reporting routers only; `None`
    /// from a publisher older than the census or run with it off.
    pub census: Option<&'a DeclarationsV1>,
    /// What a reporting router in the same routing group attributes to this
    /// one. `None` when no such reporter has a census.
    pub held: Option<HeldView<'a>>,
    /// Route-table entries per neighbor, keyed as edges are (node name or
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

/// One router's owner, graded: the state, why, and for `not_read` the cause.
pub struct Graded {
    pub state: &'static str,
    pub reason: String,
    /// For `not_read`: `link_state_only`, `no_directory`, `not_in_directory`,
    /// `directory_stale`, `old_format`, `lookup_failed`, `unknown_source`.
    /// Empty otherwise.
    pub cause: &'static str,
}

fn graded(state: &'static str, reason: impl Into<String>, cause: &'static str) -> Graded {
    Graded {
        state,
        reason: reason.into(),
        cause,
    }
}

/// Grade one router's owner from what the peer directory carried.
///
/// The order is the rule (goatcore ADR 1249): what could not be read is
/// `not_read`, never "no owner"; the record beats an exception (and says the
/// exception is stale); only a well-formed active exception excepts; a
/// malformed one is said and never applies; a roster answer is a lead.
fn owner_state(
    peer: &Option<Peer>,
    unseen: (&'static str, &'static str),
    dir: &Directory,
    now_ms: u64,
) -> Graded {
    let Some(p) = peer else {
        return graded("not_read", unseen.1, unseen.0);
    };
    // The reason names the limit, not the age, so the payload stays the same
    // while nothing happens; the page shows the age from `peers_generated_ms`.
    if dir.peers_too_old(now_ms).is_some() {
        return graded(
            "not_read",
            format!(
                "the peer directory is older than its {}-minute limit; its writer runs every 5 minutes, so its owners are not trusted",
                dir.peers_max_age_ms / 60_000
            ),
            "directory_stale",
        );
    }
    let Some(o) = &p.owner else {
        if let Some(r) = dir.owner_read.as_ref().filter(|r| r.state == "unread") {
            return graded(
                "not_read",
                format!("the owner lookup failed on this host: {}", r.reason),
                "lookup_failed",
            );
        }
        return graded(
            "not_read",
            "the peer directory carries no owner from the record (its writer predates owner-from-record)",
            "old_format",
        );
    };
    let malformed = p
        .exception
        .as_ref()
        .filter(|e| e.state == "malformed")
        .map(|e| {
            format!(
                "an owner exception is on file but malformed ({}); it does not apply",
                e.problem
            )
        });
    let with_malformed = |r: String| match &malformed {
        Some(m) if r.is_empty() => m.clone(),
        Some(m) => format!("{r}; {m}"),
        None => r,
    };
    match o.source.as_str() {
        "unread" => {
            return graded(
                "not_read",
                if o.reason.is_empty() {
                    "the owner could not be read".to_string()
                } else {
                    o.reason.clone()
                },
                "lookup_failed",
            )
        }
        "record" | "declared" => {
            let stale = p
                .exception
                .as_ref()
                .filter(|e| e.state != "malformed")
                .map(|_| {
                    "an owner exception is still on file for this peer although the record now names an owner: remove it"
                        .to_string()
                });
            let mut gaps = vec![];
            if !o.org_state.is_empty() && o.org_state != "recorded" {
                gaps.push(if o.org_reason.is_empty() {
                    "the record names no organization".to_string()
                } else {
                    o.org_reason.clone()
                });
            }
            if o.person_disabled {
                gaps.push("the responsible person's identity is disabled".to_string());
            }
            if !gaps.is_empty() {
                return graded("incomplete", with_malformed(gaps.join("; ")), "");
            }
            return graded("recorded", with_malformed(stale.unwrap_or_default()), "");
        }
        "roster" | "none" => {}
        other => {
            return graded(
                "not_read",
                format!(
                "the peer directory names an owner source this surface does not know ({other:?})"
            ),
                "unknown_source",
            )
        }
    }
    if let Some(e) = p.exception.as_ref().filter(|e| e.applies()) {
        return graded("excepted", format!("owner exception: {}", e.reason), "");
    }
    if o.source == "roster" {
        let r = if o.reason.is_empty() {
            "answered from the declared roster, not the record".to_string()
        } else {
            o.reason.clone()
        };
        return graded("lead", with_malformed(r), "");
    }
    graded("no_record", with_malformed(o.reason.clone()), "")
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
    /// When the writer generated the peer directory (Unix ms; 0 when the
    /// file does not say), and the age past which its owners are not read.
    pub peers_generated_ms: u64,
    pub peers_max_age_ms: u64,
    pub owner_read: &'a Option<OwnerRead>,
    /// The surface hides the individual: no person, nationality or grantor.
    pub individual_hidden: bool,
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
    type Who<'b> = (
        Option<&'b str>,
        Option<Peer>,
        Option<Owner>,
        (&'static str, &'static str),
    );
    let at = |a: &'a str| -> Who<'a> {
        if !dir.peers_configured {
            return (
                Some(a),
                None,
                None,
                (
                    "no_directory",
                    "no peer directory is configured on this surface",
                ),
            );
        }
        match dir.peer_at(a) {
            Some(p) => {
                let o = dir.owner_of(&p.name);
                (Some(a), Some(p.clone()), o, ("", ""))
            }
            None => (
                Some(a),
                None,
                None,
                (
                    "not_in_directory",
                    "the address it connects from is not in the peer directory",
                ),
            ),
        }
    };
    // A reporter is looked up first by the addresses its publisher states
    // (`self_addresses`); any router, then, by the address another reporter
    // sees its session come from.
    let who = |z: &'a str, own: &'a [String]| -> Who<'a> {
        if let Some(hit) = own.iter().map(|a| at(a.as_str())).find(|w| w.1.is_some()) {
            return hit;
        }
        if let Some((a, _)) = addr.get(z).copied() {
            return at(a);
        }
        if let Some(a) = own.first() {
            return at(a.as_str());
        }
        (
            None,
            None,
            None,
            (
                "link_state_only",
                "known only through link state: no reporting router holds a session with it, \
                 so the address it connects from is not seen",
            ),
        )
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
            reporting: true,
            known_from: vec![],
            client_admission: k.snap.admission().label(),
            address: None,
            peer: None,
            owner: None,
            name_note: "",
            owner_state: "not_read",
            owner_reason: String::new(),
            owner_cause: "",
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
    let own: BTreeMap<&str, &'a [String]> = known
        .iter()
        .map(|k| (k.snap.zid.as_str(), k.snap.self_addresses.as_slice()))
        .collect();
    for r in routers.iter_mut() {
        let (a, p, o, unseen) = who(r.zid, own.get(r.zid).copied().unwrap_or(&[]));
        let g = owner_state(&p, unseen, dir, now_ms);
        r.address = a;
        r.peer = p;
        r.owner = o;
        r.name_note = unseen.1;
        r.owner_state = g.state;
        r.owner_reason = g.reason;
        r.owner_cause = g.cause;
    }
    routers.extend(silent.into_iter().map(|(z, (by, fresh))| {
        let (address, peer, owner, unseen) = who(z, &[]);
        let name_note = unseen.1;
        let Graded {
            state: owner_state,
            reason: owner_reason,
            cause: owner_cause,
        } = owner_state(&peer, unseen, dir, now_ms);
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
            reporting: false,
            known_from: by.into_iter().collect(),
            client_admission: "unspecified",
            address,
            peer,
            owner,
            name_note,
            owner_state,
            owner_reason,
            owner_cause,
            census: None,
            held: held(z),
            next_hops: vec![],
        }
    }));

    // ADR 1249: a router in the fabric with no recorded owner is a finding,
    // never a gray label; one whose owner could not be looked up is a finding
    // of its own, so a failed lookup never reads as "no owner".
    let label = |r: &RouterView<'a>| -> String {
        if !r.node.is_empty() {
            r.node.to_string()
        } else if let Some(p) = &r.peer {
            p.name.clone()
        } else {
            r.zid.chars().take(8).collect()
        }
    };
    let pick = |states: &[&str]| -> Vec<(String, String)> {
        routers
            .iter()
            .filter(|r| states.contains(&r.owner_state))
            .map(|r| (label(r), r.owner_reason.clone()))
            .collect()
    };
    // A roster lead is unowned on the page as in the reconcile (ADR 1249).
    let unowned = pick(&["no_record", "lead"]);
    let incomplete = pick(&["incomplete"]);
    let excepted = pick(&["excepted"]);
    let unread: Vec<(String, String, &'static str)> = routers
        .iter()
        .filter(|r| r.owner_state == "not_read")
        .map(|r| (label(r), r.owner_reason.clone(), r.owner_cause))
        .collect();
    let mut added = false;
    if let Some(f) = findings::routers_under_owner_exception(&excepted) {
        findings.push(f);
        added = true;
    }
    if let Some(f) = findings::routers_without_owner(&unowned) {
        counts.critical += 1;
        findings.push(f);
        added = true;
    }
    if let Some(f) = findings::router_owner_not_read(&unread) {
        counts.warning += 1;
        findings.push(f);
        added = true;
    }
    if let Some(f) = findings::routers_with_incomplete_owner(&incomplete) {
        counts.warning += 1;
        findings.push(f);
        added = true;
    }
    if added {
        findings::sort(&mut findings);
    }

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
            peers_generated_ms: dir.peers_generated_ms,
            peers_max_age_ms: dir.peers_max_age_ms,
            owner_read: &dir.owner_read,
            individual_hidden: dir.individual_hidden,
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

    /// ADR 1249: owners come from the record the host writer carried. A
    /// router the record holds no owner for is a critical finding; one whose
    /// owner could not be looked up is a warning of its own; a roster answer
    /// is a lead, not the record.
    #[test]
    fn router_owners_come_from_the_record_and_unowned_routers_are_findings() {
        use crate::directory::testing::{directory, ROSTER};
        let relay = r#"{"owner_read": {"state": "read", "at": 1}, "names": {
            "a": {"name": "machine-anvil-pod-7-rack2", "ip": "100.127.0.21", "owner": {
                "source": "record", "via": "machine-custody", "matched_by": "peer-name",
                "person": {"handle": "rivera-j", "identity_id": "ory-1"},
                "org": {"id": "org-anvil", "short_tag": "anvil", "nation": "XAA", "state": "recorded"},
                "nationality": {"value": "", "state": "not_stamped"}, "machine_unit": "1st Anvil Bn"}},
            "b": {"name": "edge-gw-unlisted", "ip": "100.127.0.22", "owner": {
                "source": "none", "reason": "no enrolled machine and no slot handle matches this peer name"}},
            "c": {"name": "peer-okafor-n-zagros-2026-10-02-laptop", "ip": "100.127.0.23", "owner": {
                "source": "roster", "note": "answered from the declared roster"}}
        }}"#;
        let mut st = Store::new(0);
        st.set_directory(directory(relay, ROSTER, true));
        let mut r = with_clients("hub", "aa00", 0);
        r.regions = vec![region(
            "north",
            &["aa00", "bb11", "cc22", "dd33", "ee44"],
            &[
                ("aa00", "bb11"),
                ("aa00", "cc22"),
                ("aa00", "dd33"),
                ("dd33", "ee44"),
            ],
        )];
        let sess = |z: &str, ip: &str| {
            let mut s = router_session(z, "north");
            s.link_dst = vec![format!("tls/{ip}:50000")];
            s
        };
        r.sessions = vec![
            sess("bb11", "100.127.0.21"),
            sess("cc22", "100.127.0.22"),
            sess("dd33", "100.127.0.23"),
        ];
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
        assert_eq!(by("bb11")["owner_state"], "recorded");
        assert_eq!(by("bb11")["peer"]["owner"]["org_nation"], "XAA");
        assert_eq!(by("bb11")["peer"]["owner"]["person_handle"], "rivera-j");
        assert_eq!(by("cc22")["owner_state"], "no_record");
        assert_eq!(by("dd33")["owner_state"], "lead");
        assert_eq!(
            by("ee44")["owner_state"],
            "not_read",
            "link state only: no address to look up"
        );
        let f = |id: &str| {
            v["findings"]
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["id"] == id)
                .cloned()
        };
        let t14 = f("T-014").expect("an unowned router is a finding");
        assert_eq!(t14["severity"], "critical");
        assert!(t14["what"].as_str().unwrap().contains("edge-gw-unlisted"));
        let t15 = f("T-015").expect("an owner that could not be read is a finding of its own");
        assert_eq!(t15["severity"], "warning");
        assert!(v["counts"]["critical"].as_u64().unwrap() >= 1);
        assert_eq!(v["directory"]["owner_read"]["state"], "read");
    }

    /// A writer the portal refused: every router's owner is "not read", never
    /// "no owner", and no T-014 fires on a failed lookup.
    #[test]
    fn a_refused_owner_lookup_is_not_read_not_unowned() {
        use crate::directory::testing::{directory, ROSTER};
        let relay = r#"{"owner_read": {"state": "unread", "reason": "portal answered HTTP 401", "at": 1}, "names": {
            "a": {"name": "edge-gw-unlisted", "ip": "100.127.0.22",
                  "owner": {"source": "unread", "reason": "portal answered HTTP 401"}}}}"#;
        let mut st = Store::new(0);
        st.set_directory(directory(relay, ROSTER, true));
        let mut r = with_clients("hub", "aa00", 0);
        r.regions = vec![region("north", &["aa00", "cc22"], &[("aa00", "cc22")])];
        let mut s1 = router_session("cc22", "north");
        s1.link_dst = vec!["tls/100.127.0.22:50000".into()];
        r.sessions = vec![s1];
        ingest(&mut st, "s/hub".into(), r, 1_000);
        let v = json(&st);
        let c = v["routers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["zid"] == "cc22")
            .unwrap();
        assert_eq!(c["owner_state"], "not_read");
        assert!(c["owner_reason"].as_str().unwrap().contains("401"));
        assert!(v["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["id"] != "T-014"));
    }

    /// ADR 1249 decision 7: an owner exception is its own state, shown with
    /// its reason, never recorded and never T-014; a stale one (the record now
    /// names an owner) stays recorded and says to remove the exception.
    #[test]
    fn an_owner_exception_is_shown_and_counted_never_hidden() {
        use crate::directory::testing::{directory, ROSTER};
        let relay = r#"{"owner_read": {"state": "read", "at": 1}, "names": {
            "a": {"name": "edge-gw-unlisted", "ip": "100.127.0.22",
                  "owner": {"source": "none", "reason": "no slot handle matches"},
                  "owner_exception": {"reason": "present before the owner rule", "granted_by": "ops-lead",
                                      "granted_on": "2026-01-05", "state": "active"}},
            "b": {"name": "machine-anvil-pod-7", "ip": "100.127.0.21",
                  "owner": {"source": "record", "org": {"id": "org-anvil", "nation": "XAA"}},
                  "owner_exception": {"reason": "old", "granted_by": "ops-lead",
                                      "granted_on": "2026-01-05", "state": "stale"}}}}"#;
        let mut st = Store::new(0);
        st.set_directory(directory(relay, ROSTER, true));
        let mut r = with_clients("hub", "aa00", 0);
        r.regions = vec![region(
            "north",
            &["aa00", "bb11", "cc22"],
            &[("aa00", "bb11"), ("aa00", "cc22")],
        )];
        let sess = |z: &str, ip: &str| {
            let mut s = router_session(z, "north");
            s.link_dst = vec![format!("tls/{ip}:50000")];
            s
        };
        r.sessions = vec![sess("bb11", "100.127.0.21"), sess("cc22", "100.127.0.22")];
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
        assert_eq!(by("cc22")["owner_state"], "excepted");
        assert!(by("cc22")["owner_reason"]
            .as_str()
            .unwrap()
            .contains("present before the owner rule"));
        assert_eq!(by("cc22")["peer"]["exception"]["granted_by"], "ops-lead");
        assert_eq!(by("bb11")["owner_state"], "recorded");
        assert!(by("bb11")["owner_reason"]
            .as_str()
            .unwrap()
            .contains("remove it"));
        let ids: Vec<&str> = v["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].as_str().unwrap())
            .collect();
        assert!(
            ids.contains(&"T-016"),
            "an exception is listed, not hidden: {ids:?}"
        );
        assert!(
            !ids.contains(&"T-014"),
            "an excepted router does not raise T-014: {ids:?}"
        );
    }

    // --- review of #104 -------------------------------------------------

    fn by_zid(v: &serde_json::Value, z: &str) -> serde_json::Value {
        v["routers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["zid"] == z)
            .unwrap()
            .clone()
    }

    fn finding(v: &serde_json::Value, id: &str) -> Option<serde_json::Value> {
        v["findings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == id)
            .cloned()
    }

    /// One hub reporting, with one session to each named (zid, address).
    fn hub_with(st: &mut Store, peers: &[(&str, &str)], now_ms: u64) {
        let mut zids = vec!["aa00"];
        zids.extend(peers.iter().map(|(z, _)| *z));
        let links: Vec<(&str, &str)> = peers.iter().map(|(z, _)| ("aa00", *z)).collect();
        let mut r = with_clients("hub", "aa00", 0);
        r.regions = vec![region("north", &zids, &links)];
        r.sessions = peers
            .iter()
            .map(|(z, ip)| {
                let mut s = router_session(z, "north");
                s.link_dst = vec![format!("tls/{ip}:50000")];
                s
            })
            .collect();
        ingest(st, "s/hub".into(), r, now_ms);
    }

    const RECORD: &str = r#"{"owner_read": {"state": "read", "at": 1}, "generated_at": 1, "names": {
        "a": {"name": "machine-anvil-relay", "ip": "100.127.0.21", "owner": {
            "source": "record", "via": "machine-custody", "matched_by": "peer-name",
            "person": {"handle": "rivera-j", "identity_id": "ory-1", "disabled": false},
            "org": {"id": "org-anvil", "short_tag": "anvil", "nation": "XAA", "state": "recorded"},
            "nationality": {"value": "XCC", "state": "recorded"}},
            "owner_exception": {"reason": "r", "granted_by": "ops-lead", "granted_on": "2026-01-05", "state": "stale"}}}}"#;

    /// Item 1: the identity id is never sent; with the individual hidden, no
    /// person, nationality or grantor reaches the browser.
    #[test]
    fn the_individual_stays_hidden_and_the_identity_id_is_never_sent() {
        use crate::directory::testing::{directory, ROSTER};
        for show in [true, false] {
            let mut st = Store::new(0);
            st.set_directory(directory(RECORD, ROSTER, show));
            hub_with(&mut st, &[("bb11", "100.127.0.21")], 1_000);
            let v = json(&st);
            let body = v.to_string();
            assert!(!body.contains("ory-1"), "identity id sent (show={show})");
            assert!(
                !body.contains("identity_id"),
                "identity id field sent (show={show})"
            );
            let o = &by_zid(&v, "bb11")["peer"]["owner"];
            // The roster lead names the custodian too; hidden with the individual.
            if !show {
                assert!(by_zid(&v, "bb11")["owner"]["individual"].is_null());
            }
            assert_eq!(o["org_nation"], "XAA", "the organization is not individual");
            if show {
                assert_eq!(o["person_handle"], "rivera-j");
            } else {
                for leaked in ["rivera-j", "XCC", "ops-lead"] {
                    assert!(
                        !body.contains(leaked),
                        "{leaked} sent with the individual hidden"
                    );
                }
                assert_eq!(v["directory"]["individual_hidden"], true);
            }
        }
    }

    /// Item 2: a peer directory past its age limit downgrades every owner to
    /// not read, says how old it is, and raises no T-014.
    #[test]
    fn an_old_peer_directory_is_not_read() {
        use crate::directory::testing::{directory, ROSTER};
        let relay = r#"{"owner_read": {"state": "read", "at": 1}, "generated_at": 1, "names": {
            "b": {"name": "edge-gw-unlisted", "ip": "100.127.0.22", "owner": {"source": "none", "reason": "x"}}}}"#;
        let mut st = Store::new(0);
        st.set_directory(directory(relay, ROSTER, true));
        let thirty_min = 1_000 + 30 * 60 * 1000;
        hub_with(&mut st, &[("cc22", "100.127.0.22")], thirty_min);
        let v = json(&st);
        let c = by_zid(&v, "cc22");
        assert_eq!(c["owner_state"], "not_read");
        assert_eq!(c["owner_cause"], "directory_stale");
        assert!(
            c["owner_reason"]
                .as_str()
                .unwrap()
                .contains("older than its 20-minute limit"),
            "{c}"
        );
        assert!(finding(&v, "T-014").is_none());
        // The surface notices the age on its refresh tick, with no new report.
        let mut st2 = Store::new(0);
        st2.set_directory(directory(relay, ROSTER, true));
        let mut r = with_clients("hub", "aa00", 0);
        r.interval_secs = 3600; // the hub stays fresh: only the directory ages
        r.regions = vec![region("north", &["aa00", "cc22"], &[("aa00", "cc22")])];
        let mut s1 = router_session("cc22", "north");
        s1.link_dst = vec!["tls/100.127.0.22:50000".into()];
        r.sessions = vec![s1];
        ingest(&mut st2, "s/hub".into(), r, 1_000);
        assert_eq!(by_zid(&json(&st2), "cc22")["owner_state"], "no_record");
        crate::view::testing::refresh(&mut st2, 25 * 60 * 1000);
        assert_eq!(
            by_zid(&json(&st2), "cc22")["owner_cause"],
            "directory_stale"
        );
        // A fresh one grades normally.
        let mut st = Store::new(0);
        st.set_directory(directory(relay, ROSTER, true));
        hub_with(&mut st, &[("cc22", "100.127.0.22")], 1_000);
        assert_eq!(by_zid(&json(&st), "cc22")["owner_state"], "no_record");
    }

    /// Item 3: a reporter no other reporter holds a session with is looked
    /// up by the address its publisher states.
    #[test]
    fn a_lone_reporter_is_looked_up_by_its_own_address() {
        use crate::directory::testing::{directory, ROSTER};
        let mut st = Store::new(0);
        st.set_directory(directory(RECORD, ROSTER, true));
        let mut r = with_clients("hub", "aa00", 0);
        r.self_addresses = vec!["100.127.0.21".into()];
        ingest(&mut st, "s/hub".into(), r, 1_000);
        let v = json(&st);
        let h = by_zid(&v, "aa00");
        assert_eq!(h["owner_state"], "recorded", "{h}");
        assert_eq!(h["peer"]["name"], "machine-anvil-relay");
        assert!(finding(&v, "T-015").is_none());
    }

    /// Item 4: each cause of "not read" has its own words and steps.
    #[test]
    fn each_cause_of_an_unread_owner_says_itself() {
        use crate::directory::testing::{directory, ROSTER};
        // No directory configured.
        let mut st = Store::new(0);
        hub_with(&mut st, &[("cc22", "100.127.0.22")], 1_000);
        let v = json(&st);
        assert_eq!(by_zid(&v, "cc22")["owner_cause"], "no_directory");
        let t = finding(&v, "T-015").unwrap().to_string();
        assert!(t.contains("no peer directory is configured"), "{t}");
        assert!(t.contains("--peer-directory"), "{t}");
        assert!(
            !t.contains("refuses"),
            "a cause that is absent is not described: {t}"
        );
        // Old-format file (no owner blocks), even where the local roster matches.
        let old = r#"{"names": {"b": {"name": "machine-anvil-pod-7", "ip": "100.127.0.22"}}}"#;
        let mut st = Store::new(0);
        st.set_directory(directory(old, ROSTER, true));
        hub_with(&mut st, &[("cc22", "100.127.0.22")], 1_000);
        let v = json(&st);
        let c = by_zid(&v, "cc22");
        assert_eq!(
            (c["owner_state"].as_str(), c["owner_cause"].as_str()),
            (Some("not_read"), Some("old_format"))
        );
        assert!(
            finding(&v, "T-014").is_none(),
            "an old file is not an unowned router"
        );
        assert!(finding(&v, "T-015").unwrap()["steps"]
            .to_string()
            .contains("relay-health"));
        // Address not in the directory; unknown source.
        let relay = r#"{"owner_read": {"state": "read", "at": 1}, "generated_at": 1, "names": {
            "b": {"name": "edge", "ip": "100.127.0.22", "owner": {"source": "martian"}}}}"#;
        let mut st = Store::new(0);
        st.set_directory(directory(relay, ROSTER, true));
        hub_with(
            &mut st,
            &[("cc22", "100.127.0.22"), ("dd33", "100.127.0.99")],
            1_000,
        );
        let v = json(&st);
        assert_eq!(by_zid(&v, "cc22")["owner_cause"], "unknown_source");
        assert_eq!(by_zid(&v, "dd33")["owner_cause"], "not_in_directory");
        let t = finding(&v, "T-015").unwrap().to_string();
        assert!(t.contains("not in the peer directory"), "{t}");
    }

    /// Item 5: a recorded owner with no organization, or a disabled person,
    /// is a finding that says the recorded reason.
    #[test]
    fn an_incomplete_recorded_owner_is_a_finding() {
        use crate::directory::testing::{directory, ROSTER};
        let relay = r#"{"owner_read": {"state": "read", "at": 1}, "generated_at": 1, "names": {
            "a": {"name": "machine-x", "ip": "100.127.0.21", "owner": {"source": "record", "via": "machine-custody",
                  "person": {"handle": "rivera-j"}, "org": {"state": "missing", "reason": "the identity carries no organization"}}},
            "b": {"name": "machine-y", "ip": "100.127.0.22", "owner": {"source": "record", "via": "machine-custody",
                  "person": {"handle": "okafor-n", "disabled": true}, "org": {"id": "org-anvil", "state": "recorded"}}}}}"#;
        let mut st = Store::new(0);
        st.set_directory(directory(relay, ROSTER, true));
        hub_with(
            &mut st,
            &[("bb11", "100.127.0.21"), ("cc22", "100.127.0.22")],
            1_000,
        );
        let v = json(&st);
        assert_eq!(by_zid(&v, "bb11")["owner_state"], "incomplete");
        assert!(by_zid(&v, "bb11")["owner_reason"]
            .as_str()
            .unwrap()
            .contains("no organization"));
        assert_eq!(by_zid(&v, "cc22")["owner_state"], "incomplete");
        assert!(by_zid(&v, "cc22")["owner_reason"]
            .as_str()
            .unwrap()
            .contains("disabled"));
        let t = finding(&v, "T-017").expect("an incomplete owner is a finding");
        assert_eq!(t["severity"], "warning");
        assert!(t["what"].as_str().unwrap().contains("machine-x"));
    }

    /// Item 6: a router owned only by a roster lead is in T-014, as the
    /// reconcile counts it.
    #[test]
    fn a_router_with_only_a_lead_is_unowned() {
        use crate::directory::testing::{directory, ROSTER};
        let relay = r#"{"owner_read": {"state": "read", "at": 1}, "generated_at": 1, "names": {
            "c": {"name": "peer-okafor-n-zagros-2026-01-05-laptop", "ip": "100.127.0.23",
                  "owner": {"source": "roster", "note": "answered from the declared roster"}}}}"#;
        let mut st = Store::new(0);
        st.set_directory(directory(relay, ROSTER, true));
        hub_with(&mut st, &[("dd33", "100.127.0.23")], 1_000);
        let v = json(&st);
        assert_eq!(by_zid(&v, "dd33")["owner_state"], "lead");
        let t = finding(&v, "T-014").expect("a lead is not a record");
        assert!(t["what"].as_str().unwrap().contains("peer-okafor-n"));
    }

    /// Item 7: recorded beats an exception; an exception never beats an
    /// unread owner; a malformed exception is said and never applies.
    #[test]
    fn exceptions_never_outrank_the_record_or_an_unread_owner() {
        use crate::directory::testing::{directory, ROSTER};
        let exc = |state: &str, reason: &str| {
            format!(
                r#""owner_exception": {{"reason": "{reason}", "granted_by": "g", "granted_on": "2026-01-05", "state": "{state}"}}"#
            )
        };
        let relay = format!(
            r#"{{"owner_read": {{"state": "read", "at": 1}}, "generated_at": 1, "names": {{
            "a": {{"name": "m-a", "ip": "100.127.0.21", "owner": {{"source": "record", "org": {{"id": "o", "state": "recorded"}}}}, {}}},
            "b": {{"name": "m-b", "ip": "100.127.0.22", "owner": {{"source": "none", "reason": "x"}}, {}}},
            "c": {{"name": "m-c", "ip": "100.127.0.23", "owner": {{"source": "none", "reason": "x"}}, {}}},
            "d": {{"name": "m-d", "ip": "100.127.0.24", "owner": {{"source": "unread", "reason": "lost"}}, {}}}}}}}"#,
            exc("active", "r"),
            exc("", "r"),
            exc("active", ""),
            exc("active", "r")
        );
        let mut st = Store::new(0);
        st.set_directory(directory(&relay, ROSTER, true));
        hub_with(
            &mut st,
            &[
                ("bb11", "100.127.0.21"),
                ("cc22", "100.127.0.22"),
                ("dd33", "100.127.0.23"),
                ("ee44", "100.127.0.24"),
            ],
            1_000,
        );
        let v = json(&st);
        let a = by_zid(&v, "bb11");
        assert_eq!(a["owner_state"], "recorded");
        assert!(a["owner_reason"].as_str().unwrap().contains("remove it"));
        for z in ["cc22", "dd33"] {
            let r = by_zid(&v, z);
            assert_eq!(
                r["owner_state"], "no_record",
                "{z}: a malformed exception never applies"
            );
            assert!(
                r["owner_reason"].as_str().unwrap().contains("malformed"),
                "{r}"
            );
            assert_eq!(r["peer"]["exception"]["state"], "malformed");
        }
        assert_eq!(by_zid(&v, "ee44")["owner_state"], "not_read");
        assert!(finding(&v, "T-016").is_none(), "nothing here is excepted");
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
