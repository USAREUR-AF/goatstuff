// SPDX-License-Identifier: Apache-2.0
//! The findings engine — what turns the topology picture into advice.
//!
//! # Written to be read cold
//!
//! A finding is read by someone who has just opened this page, mid-incident,
//! possibly having never heard of a "subregion". So each one is built in a
//! fixed order that answers the reader's questions as they occur:
//!
//! | Field | Answers |
//! |---|---|
//! | `headline` | **What is the problem, in plain words?** Consequence first, no jargon, no identifiers. |
//! | `what` | What did we actually see? The specific evidence, with the names. |
//! | `why` | Why does that matter? Mechanism, in short sentences. |
//! | `impact` | What happens if I do nothing? |
//! | `steps` | What do I do? Imperative, ordered, one action each. |
//!
//! The `id` stays so a finding can be quoted in a ticket, but it is not the
//! headline: "T-001" tells a cold reader nothing.
//!
//! **A branch belongs in the steps, not in a sentence.** The earlier version of
//! T-001 buried "if the intent is routing… if the intent is access…" inside a
//! paragraph, and a reader could not tell which half applied to them.
//!
//! # Rules, in order of how often they are broken elsewhere
//!
//! 1. **Every finding names its subject.** A router, or a pair. Never "the
//!    fabric" — an operator cannot act on that.
//! 2. **A finding states what was OBSERVED.** Where one rests on a projection,
//!    the finding says so itself (only `T-009` does).
//! 3. **Never recommend `region_names` or `zids` as a way to scope anything.**
//!    They are peer-asserted (ADR 1041 citing ADR 0844). `T-001` flags their
//!    use as the hazard and points at certificate identity instead.
//! 4. **Silence is never health.** A router with no recent snapshot is
//!    `unknown` and says so; it is never dropped from the list, because an
//!    omitted router reads as "no problem" (ADR 1036).

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::wire::{GatewayMode, RouterTopologyV1};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Acting on this is not optional: a security hazard, or a config that
    /// silently does the opposite of its intent.
    Critical,
    /// Something is wrong or inconsistent and wants an operator's eyes.
    Warning,
    /// Worth knowing before it becomes a problem.
    Advisory,
    /// Stated so the picture is not misread. Not a defect.
    Info,
}

#[derive(Clone, Debug, Serialize)]
pub struct Finding {
    /// Quotable in a ticket. Never the headline — it tells a cold reader nothing.
    pub id: &'static str,
    pub severity: Severity,
    /// The router, or `a ↔ b` pair, this is about.
    pub subject: String,
    /// Plain words, consequence first. No jargon, no identifiers, no ID.
    pub headline: String,
    /// The specific observation, with names and numbers.
    pub what: String,
    /// The mechanism, in short sentences.
    pub why: String,
    /// What happens if nothing is done.
    pub impact: String,
    /// Ordered, imperative, one action each. A branch lives here as separate
    /// steps, never buried inside a sentence.
    pub steps: Vec<String>,
}

/// Small helper so every construction site reads the same way.
fn steps(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// A router the surface knows about, with how stale its snapshot is.
pub struct Known<'a> {
    pub snap: &'a RouterTopologyV1,
    pub age_secs: u64,
}

impl Known<'_> {
    /// Three intervals without a snapshot and the router's view is unknown.
    /// A publisher that never declared an interval is graded against 30s.
    pub fn is_stale(&self) -> bool {
        let interval = if self.snap.interval_secs == 0 {
            30
        } else {
            self.snap.interval_secs as u64
        };
        self.age_secs > interval.saturating_mul(3)
    }

    fn name(&self) -> &str {
        if self.snap.node.is_empty() {
            &self.snap.zid
        } else {
            &self.snap.node
        }
    }
}

/// Grade a whole goatnet's routers.
pub fn evaluate(routers: &[Known<'_>]) -> Vec<Finding> {
    let mut out = Vec::new();
    for r in routers {
        stale(r, &mut out);
        gateway_config(r, &mut out);
        route_table(r, &mut out);
        isolated(r, &mut out);
    }
    mode_drift(routers, &mut out);
    asymmetry(routers, &mut out);
    version_skew(routers, &mut out);
    growth(routers, &mut out);

    // Most severe first; then stably by id and subject so the list does not
    // reshuffle between refreshes for no reason.
    out.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| a.id.cmp(b.id))
            .then_with(|| a.subject.cmp(&b.subject))
    });
    out
}

// --- T-006 -----------------------------------------------------------------

fn stale(r: &Known<'_>, out: &mut Vec<Finding>) {
    if !r.is_stale() {
        return;
    }
    out.push(Finding {
        id: "T-006",
        severity: Severity::Warning,
        subject: r.name().into(),
        headline: "This router has gone quiet, so we cannot see what it is doing.".into(),
        // No live seconds here on purpose: a finding whose text changed every
        // tick would churn the payload, the ETag and the DOM while saying
        // nothing new. The routers table carries the live age.
        what: format!(
            "{} has stopped reporting. It should report every {}s.",
            r.name(),
            if r.snap.interval_secs == 0 { 30 } else { r.snap.interval_secs }
        ),
        why: "Everything shown about a router comes from that router reporting on itself. When \
              the reports stop, the last picture is all we have, and it gets older every second."
            .into(),
        impact: "It stays on the list marked stale rather than disappearing, because a router \
                 that vanishes from a screen reads as 'no problem'. Treat its numbers as \
                 unknown, not as current."
            .into(),
        steps: steps(&[
            "Check the goat-topology publisher on that host first, not the router itself.",
            "The router can be perfectly healthy while its reporter is down — they are separate processes.",
            "If the publisher is running, check it can still reach its own router on 127.0.0.1:8000.",
        ]),
    });
}

// --- T-001, T-002, T-010 ----------------------------------------------------

fn gateway_config(r: &Known<'_>, out: &mut Vec<Finding>) {
    let s = r.snap;
    let name = r.name();

    if s.mode() == GatewayMode::Unreadable {
        out.push(Finding {
            id: "T-011",
            severity: Severity::Warning,
            subject: name.into(),
            headline: "We could not read this router's settings file, so its setting is unknown."
                .into(),
            what: format!("Reading the config file failed: {}", s.gateway_read_error),
            why: "There are two ways a router ends up with no custom routing groups: it was \
                  never given any, or it was given a list that matches nothing. Those look \
                  identical from the outside. Only the settings file tells them apart."
                .into(),
            impact: "The setting is shown as unreadable rather than guessed. Guessing would \
                     report the second case — a real misconfiguration — as if it were the \
                     first, which is fine."
                .into(),
            steps: steps(&[
                "Check the path given to the publisher (--config-path) points at this router's real config file.",
                "Check the publisher can read that file; it runs as a non-root user.",
                "Until then, treat this router's routing configuration as unverified.",
            ]),
        });
    }

    for sub in &s.custom_subregions {
        if sub.is_inert() {
            out.push(Finding {
                id: "T-002",
                severity: Severity::Critical,
                subject: name.into(),
                headline: "A routing group is set up to match nothing, so it does nothing."
                    .into(),
                what: format!(
                    "On {name}, routing group {} has an empty list of match rules.",
                    sub.index
                ),
                why: "An empty list of rules matches no one. The group exists in the config but \
                      never takes a member. Note the trap: leaving the rules out ENTIRELY has \
                      the opposite effect — it matches everyone."
                    .into(),
                impact: "Every peer this group was meant to hold is being handled as if the \
                         group were not there. Nothing logs an error, so the config looks \
                         applied while doing the opposite of its intent."
                    .into(),
                steps: steps(&[
                    "Decide what this group was meant to hold, and write the match rules for it.",
                    "Or delete the group entry if it is left over.",
                    "If you delete it, check every later group: they are numbered by position, so removing one renumbers the rest — and those numbers appear in live region names.",
                ]),
            });
        }

        for f in &sub.filters {
            if !f.uses_peer_asserted_axis() {
                continue;
            }
            let axes = match (f.uses_zids, f.uses_region_names) {
                (true, true) => "`zids` and `region_names`",
                (true, false) => "`zids`",
                _ => "`region_names`",
            };
            out.push(Finding {
                id: "T-001",
                severity: Severity::Critical,
                subject: name.into(),
                headline: "A routing rule is matching on something the other side can simply \
                           claim — it cannot keep anyone out."
                    .into(),
                what: format!("On {name}, routing group {} matches peers by {axes}.", sub.index),
                why: "Those values are chosen and announced by the peer being matched. Nothing \
                      verifies them. A peer that announces a different one is routed \
                      differently and gains no access — but equally, none is denied."
                    .into(),
                impact: "If anyone believes this rule is keeping a party away from data, that \
                         belief is wrong, and nothing anywhere will signal it. Access is decided \
                         by the certificate trust store and the fabric access rules, neither of \
                         which knows these routing groups exist."
                    .into(),
                steps: steps(&[
                    "Decide which this rule is for: steering traffic, or controlling access.",
                    "Steering traffic — this is correct. Leave a comment in the config saying it is routing-only, so the next reader does not mistake it for a control.",
                    "Controlling access — move it to the fabric access rules, matched on the peer's certificate identity, which is issued and verified rather than announced.",
                    "Either way, never widen or narrow a routing group in order to grant or deny access.",
                ]),
            });
        }
    }

    // T-010 — the declared config and the observed regions disagree.
    //
    // Only decidable under Custom. Under Preset(Auto) there is nothing declared
    // to diverge FROM: Auto computes the region set at runtime, south regions
    // included, so a populated `south:<id>:<mode>` is Auto working normally.
    // Keying off "a south region exists" would fire Critical on any Auto router —
    // `south:0:peer`, V=3, from an empty config. A rig whose Auto arms have no
    // southern peers to form a region from cannot show this.
    //
    // The divergence that IS real: zenoh assigns region ids from list POSITION
    // (`south:<index>:<mode>`, region.rs:124-136), so the declared indices and
    // the running ones must be the same set. They part when the file is edited
    // without a restart — the router read it once, at start.
    let declared: BTreeSet<u32> = s.custom_subregions.iter().map(|r| r.index).collect();
    let observed: BTreeSet<u32> = s
        .regions
        .iter()
        .filter_map(|reg| reg.name.strip_prefix("south:"))
        .filter_map(|rest| rest.split(':').next())
        .filter_map(|id| id.parse::<u32>().ok())
        .collect();
    if s.mode() == GatewayMode::Custom && declared != observed {
        let only_file: Vec<String> = declared
            .difference(&observed)
            .map(|i| i.to_string())
            .collect();
        let only_running: Vec<String> = observed
            .difference(&declared)
            .map(|i| i.to_string())
            .collect();
        out.push(Finding {
            id: "T-010",
            severity: Severity::Critical,
            subject: name.into(),
            headline: "The settings file on disk does not match what this router is actually doing."
                .into(),
            what: format!(
                "{name}'s config file declares routing groups {declared:?}, but the router is \
                 running {observed:?}. In the file only: [{}]. Running only: [{}].",
                only_file.join(", "),
                only_running.join(", "),
            ),
            why: "A router reads its config once, at start. Editing the file afterwards changes \
                  the file and nothing else."
                .into(),
            impact: "Anyone reading the file to understand this router will reach the wrong \
                     conclusion. A restart — planned or from a crash — will change the routing \
                     behaviour without anyone intending it."
                .into(),
            steps: steps(&[
                "Decide which one is right: the file, or what is running.",
                "If the file is right, restart the router so it adopts the file — and expect the routing shape to change.",
                "If what is running is right, fix the file to say so before someone restarts it.",
                "Either way, find out how they diverged: an edit without a restart, or an out-of-band change.",
            ]),
        });
    }
}

// --- T-005 -----------------------------------------------------------------

fn route_table(r: &Known<'_>, out: &mut Vec<Finding>) {
    let s = r.snap;
    if s.regions.is_empty() || !s.observe_error.is_empty() {
        return;
    }
    let expected = s.expected_successors();
    if expected == s.route_successor_count {
        return;
    }
    out.push(Finding {
        id: "T-005",
        severity: Severity::Warning,
        subject: r.name().into(),
        headline: "This router is holding a different number of routes than its own shape predicts."
            .into(),
        what: format!(
            "{} holds {} route entries. Its regions predict {expected}.",
            r.name(),
            s.route_successor_count
        ),
        why: "A router keeps one next-hop entry for every ordered pair of routers in each region \
              it belongs to. That arithmetic matched exactly at every point we have measured, so \
              a difference means something is moving or something is not understood."
            .into(),
        impact: "Usually harmless and brief: the router is still settling after a topology \
                 change. If it does not settle, the model we use to predict routing cost is \
                 wrong for this shape, and the planner on this page would mislead."
            .into(),
        steps: steps(&[
            "Wait one reporting interval and look again — settling is the common cause.",
            "If it persists across several intervals, write it down: a lasting counter-example to this arithmetic is worth a finding of its own.",
            "Do not act on the planner's numbers for this router while the two disagree.",
        ]),
    });
}

// --- T-008 -----------------------------------------------------------------

fn isolated(r: &Known<'_>, out: &mut Vec<Finding>) {
    let s = r.snap;
    if !s.observe_error.is_empty() {
        return;
    }
    let peers = s.sessions.iter().filter(|x| x.whatami != "client").count();
    let north_v = s.region("north").map(|x| x.link_state_v).unwrap_or(0);
    if peers == 0 && north_v <= 1 {
        out.push(Finding {
            id: "T-008",
            severity: Severity::Info,
            subject: r.name().into(),
            headline: "This router has no other routers to talk to.".into(),
            what: format!(
                "{} holds no router-to-router connections. {} applications connect to it.",
                r.name(),
                s.sessions.len()
            ),
            why: "A goatnet with one router is a normal shape, not a broken one. Applications \
                  connect to it and it routes between them locally."
                .into(),
            impact: "None, if this environment is meant to have one router. This is here so the \
                     empty graph is not read as a fault."
                .into(),
            steps: steps(&[
                "If this environment is meant to have one router, there is nothing to do.",
                "If it is meant to peer with others, this is where to start: check that the other routers are up and that this one is configured to connect to them.",
            ]),
        });
    }
}

// --- T-003 -----------------------------------------------------------------

fn mode_drift(routers: &[Known<'_>], out: &mut Vec<Finding>) {
    let modes: BTreeSet<&str> = routers
        .iter()
        .filter(|r| !r.is_stale() && r.snap.mode() != GatewayMode::Unreadable)
        .map(|r| r.snap.mode().label())
        .collect();
    if modes.len() < 2 {
        return;
    }
    let mut by_mode: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for r in routers.iter().filter(|r| !r.is_stale()) {
        by_mode
            .entry(r.snap.mode().label())
            .or_default()
            .push(r.name());
    }
    let detail = by_mode
        .iter()
        .map(|(m, ns)| format!("{m}: {}", ns.join(", ")))
        .collect::<Vec<_>>()
        .join(" | ");
    out.push(Finding {
        id: "T-003",
        severity: Severity::Warning,
        subject: "this environment".into(),
        headline: "Routers here are configured differently from one another.".into(),
        what: format!("Split across modes — {detail}."),
        why: "Routing configuration is set per router, so nothing forces them to agree. A split \
              is exactly what a half-finished rollout looks like, and also exactly what a \
              forgotten one looks like."
            .into(),
        impact: "This page cannot tell those two apart. If it is a rollout it will resolve on \
                 its own; if it is not, the routers will keep disagreeing about the shape of the \
                 network indefinitely."
            .into(),
        steps: steps(&[
            "If a rollout is in progress, no action — check back when it finishes.",
            "If not, pick the intended mode and bring the routers named above onto it.",
            "Remember a router only adopts its config file at start, so each change needs a restart to take effect.",
        ]),
    });
}

// --- T-004 -----------------------------------------------------------------

fn asymmetry(routers: &[Known<'_>], out: &mut Vec<Finding>) {
    let fresh: Vec<&Known<'_>> = routers.iter().filter(|r| !r.is_stale()).collect();
    let by_zid: BTreeMap<&str, &Known<'_>> =
        fresh.iter().map(|r| (r.snap.zid.as_str(), *r)).collect();

    for a in &fresh {
        let Some(a_north) = a.snap.region("north") else {
            continue;
        };
        for member in &a_north.member_zids {
            if member == &a.snap.zid {
                continue;
            }
            let Some(b) = by_zid.get(member.as_str()) else {
                continue;
            };
            let Some(b_north) = b.snap.region("north") else {
                continue;
            };
            if b_north.member_zids.contains(&a.snap.zid) {
                continue;
            }
            out.push(Finding {
                id: "T-004",
                severity: Severity::Warning,
                subject: format!("{} ↔ {}", a.name(), b.name()),
                headline: "Two routers disagree about whether they are connected to each other."
                    .into(),
                what: format!(
                    "{} lists {} as a neighbour. {} does not list {}.",
                    a.name(),
                    b.name(),
                    b.name(),
                    a.name()
                ),
                why: "Routers tell each other who they are connected to, so in a settled network \
                      the two views agree. A one-sided view means one of them has not caught up, \
                      or a connection dropped in one direction only."
                    .into(),
                impact: "The two routers are working from different maps. Traffic between them \
                         may take an unexpected path, or none. Only one of them can be right."
                    .into(),
                steps: steps(&[
                    "Wait one reporting interval and look again — catching up is the common cause.",
                    "If it persists, check the network path between the two hosts from BOTH ends, not just one.",
                    "A one-way drop often means a firewall or route change on the side that stopped seeing the other.",
                ]),
            });
        }
    }
}

// --- T-007 -----------------------------------------------------------------

fn version_skew(routers: &[Known<'_>], out: &mut Vec<Finding>) {
    let mut by_version: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for r in routers.iter().filter(|r| !r.is_stale()) {
        let v = r.snap.zenoh_version.trim();
        if !v.is_empty() {
            by_version.entry(v).or_default().push(r.name());
        }
    }
    if by_version.len() < 2 {
        return;
    }
    let detail = by_version
        .iter()
        .map(|(v, ns)| format!("{v}: {}", ns.join(", ")))
        .collect::<Vec<_>>()
        .join(" | ");
    out.push(Finding {
        id: "T-007",
        severity: Severity::Warning,
        subject: "this environment".into(),
        headline: "Routers here are running different software versions.".into(),
        what: format!("Versions in use — {detail}."),
        why: "One version is meant to run everywhere. Mixed versions are pinned against \
              deliberately, because a connection between two versions can open successfully and \
              then behave incorrectly, rather than failing in an obvious way."
            .into(),
        impact: "Faults from a version mismatch tend to appear as unrelated symptoms elsewhere, \
                 which makes them expensive to chase. This is worth closing before it costs \
                 someone a day."
            .into(),
        steps: steps(&[
            "Check which version is the current one of record in ops/pins/fleet-pins.yaml.",
            "Bring the routers named above onto it — a version change is a coordinated bump, not a per-host edit.",
            "See docs/design/zenoh-version-pinning.md for that procedure.",
        ]),
    });
}

// --- T-009 -----------------------------------------------------------------

fn growth(routers: &[Known<'_>], out: &mut Vec<Finding>) {
    let fresh: Vec<&Known<'_>> = routers.iter().filter(|r| !r.is_stale()).collect();
    if fresh.is_empty() {
        return;
    }
    let worst = fresh
        .iter()
        .max_by_key(|r| r.snap.expected_successors())
        .expect("non-empty");
    let v = worst
        .snap
        .region("north")
        .map(|r| r.link_state_v)
        .unwrap_or(0);
    let next = (v + 1).saturating_mul(v);

    out.push(Finding {
        id: "T-009",
        severity: Severity::Advisory,
        subject: worst.name().into(),
        headline: "How routing cost here will grow as routers are added.".into(),
        what: format!(
            "The largest group holds {v} routers and {} route entries on {}. Adding one more \
             router to it would make {next}.",
            worst.snap.expected_successors(),
            worst.name()
        ),
        why: "Every router keeps one entry for each ordered pair of routers in its group, so \
              cost grows with the SQUARE of the router count. Applications are free — a hundred \
              of them added nothing to these tables when we measured it."
            .into(),
        impact: format!(
            "Nothing to act on at {v} routers; these numbers are tiny. It is here so the curve \
             is visible well before it matters, rather than discovered at the point it does."
        ),
        steps: steps(&[
            "No action needed at this size.",
            "If this group is ever planned to grow past roughly a dozen routers, use the planner below to see what splitting it into groups would cost.",
            "Treat the planner as a projection: the arithmetic comes from a measurement taken at two groups of three routers on one host, and it counts routing state only — it predicts nothing about speed.",
        ]),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{GatewayFilterV1, GatewaySubregionV1, RegionV1, SessionV1};

    fn router(node: &str, zid: &str) -> RouterTopologyV1 {
        RouterTopologyV1 {
            zid: zid.into(),
            node: node.into(),
            site: "zagros-sandbox".into(),
            zenoh_version: "v1.9.0".into(),
            interval_secs: 30,
            gateway_mode: GatewayMode::Auto as i32,
            ..Default::default()
        }
    }
    fn north(v: u32, members: &[&str]) -> RegionV1 {
        RegionV1 {
            name: "north".into(),
            link_state_v: v,
            link_state_e: v.saturating_sub(1),
            member_zids: members.iter().map(|s| s.to_string()).collect(),
            session_count: v.saturating_sub(1),
        }
    }
    fn fresh(s: &RouterTopologyV1) -> Known<'_> {
        Known {
            snap: s,
            age_secs: 5,
        }
    }
    fn has(f: &[Finding], id: &str) -> bool {
        f.iter().any(|x| x.id == id)
    }

    /// Build one of every finding, so the contract below is checked against all
    /// of them rather than whichever the other tests happen to construct.
    fn one_of_everything() -> Vec<Finding> {
        let mut all = Vec::new();

        let mut carved = router("gw", "aaaa");
        carved.gateway_mode = GatewayMode::Custom as i32;
        carved.custom_subregions = vec![
            GatewaySubregionV1 {
                index: 0,
                filters_present: true,
                filters: vec![GatewayFilterV1 {
                    uses_zids: true,
                    ..Default::default()
                }],
            },
            GatewaySubregionV1 {
                index: 1,
                filters_present: true,
                filters: vec![],
            },
        ];
        carved.regions = vec![
            north(2, &["aaaa", "bbbb"]),
            RegionV1 {
                name: "south:0:router".into(),
                link_state_v: 3,
                link_state_e: 2,
                member_zids: vec!["aaaa".into(), "cccc".into(), "dddd".into()],
                session_count: 2,
            },
        ];
        carved.route_successor_count = 99; // forces T-005
        carved.zenoh_version = "v1.9.0-other".into();

        let mut plain = router("b", "bbbb");
        plain.regions = vec![north(1, &["bbbb"])];

        let mut unreadable = router("c", "cccc");
        unreadable.gateway_mode = GatewayMode::Unreadable as i32;
        unreadable.gateway_read_error = "permission denied".into();

        let stale_snap = router("d", "dddd");

        all.extend(evaluate(&[
            fresh(&carved),
            fresh(&plain),
            fresh(&unreadable),
            Known {
                snap: &stale_snap,
                age_secs: 9000,
            },
        ]));
        all
    }

    /// Emits the real engine output as JSON for the design preview, so the
    /// preview can never drift from what the code actually says.
    #[test]
    #[ignore = "run explicitly: cargo test -- --ignored dump_for_preview --nocapture"]
    fn dump_for_preview() {
        let all = one_of_everything();
        println!(
            "PREVIEW_FINDINGS_JSON {}",
            serde_json::to_string(&all).unwrap()
        );
    }

    #[test]
    fn every_finding_reads_cold() {
        let all = one_of_everything();
        assert!(all.len() >= 6, "expected a broad sample, got {}", all.len());

        for f in &all {
            // A headline must be a sentence a newcomer can act on, not an
            // identifier and not a fragment.
            assert!(
                f.headline.len() > 25 && f.headline.ends_with('.'),
                "{}: headline is not a plain sentence: {:?}",
                f.id,
                f.headline
            );
            assert!(
                !f.headline.contains(f.id),
                "{}: the headline must not lead with the id",
                f.id
            );

            // Jargon a cold reader cannot be assumed to know does not belong in
            // the headline. It is fine further down, where context exists.
            for word in [
                "subregion",
                "zid",
                "linkstate",
                "link-state",
                "Σ",
                "gateway.south",
            ] {
                assert!(
                    !f.headline.to_lowercase().contains(&word.to_lowercase()),
                    "{}: headline uses jargon {word:?}: {:?}",
                    f.id,
                    f.headline
                );
            }

            assert!(!f.what.is_empty(), "{}: no observation", f.id);
            assert!(!f.why.is_empty(), "{}: no mechanism", f.id);
            assert!(!f.impact.is_empty(), "{}: no consequence stated", f.id);
            assert!(!f.steps.is_empty(), "{}: no steps", f.id);

            // A branch belongs in the steps, not buried in a sentence.
            for step in &f.steps {
                assert!(step.len() > 12, "{}: step too thin: {step:?}", f.id);
            }
        }
    }

    #[test]
    fn a_zid_filter_is_critical_and_the_advice_is_never_use_a_region() {
        let mut s = router("thor-01", "aaaa");
        s.gateway_mode = GatewayMode::Custom as i32;
        s.custom_subregions = vec![GatewaySubregionV1 {
            index: 0,
            filters_present: true,
            filters: vec![GatewayFilterV1 {
                uses_zids: true,
                ..Default::default()
            }],
        }];
        let f = evaluate(&[fresh(&s)]);
        let t1 = f.iter().find(|x| x.id == "T-001").expect("T-001");
        assert_eq!(t1.severity, Severity::Critical);
        assert!(
            t1.steps.iter().any(|s| s.contains("certificate identity")),
            "the fix must point at certificate identity: {:?}",
            t1.steps
        );
        // Rule 3: the engine must never propose regions as a scoping mechanism.
        for x in &f {
            for step in &x.steps {
                let r = step.to_lowercase();
                assert!(
                    !(r.contains("scope by region") || r.contains("use region_names")),
                    "a step recommended scoping by region: {step}"
                );
            }
        }
    }

    #[test]
    fn an_empty_filter_list_is_critical_and_an_absent_one_is_silent() {
        let mut empty = router("a", "aaaa");
        empty.gateway_mode = GatewayMode::Custom as i32;
        empty.custom_subregions = vec![GatewaySubregionV1 {
            index: 0,
            filters_present: true,
            filters: vec![],
        }];
        assert!(has(&evaluate(&[fresh(&empty)]), "T-002"));

        let mut absent = router("a", "aaaa");
        absent.gateway_mode = GatewayMode::Custom as i32;
        absent.custom_subregions = vec![GatewaySubregionV1 {
            index: 0,
            filters_present: false,
            filters: vec![],
        }];
        assert!(!has(&evaluate(&[fresh(&absent)]), "T-002"));
    }

    #[test]
    fn a_stale_router_is_reported_not_dropped() {
        let s = router("thor-01", "aaaa");
        let f = evaluate(&[Known {
            snap: &s,
            age_secs: 500,
        }]);
        assert!(has(&f, "T-006"), "silence must never read as health");
    }

    #[test]
    fn a_stale_router_does_not_manufacture_a_mode_drift() {
        // One fresh custom + one STALE auto is not evidence of a split.
        let mut a = router("a", "aaaa");
        a.gateway_mode = GatewayMode::Custom as i32;
        let b = router("b", "bbbb");
        let f = evaluate(&[
            fresh(&a),
            Known {
                snap: &b,
                age_secs: 900,
            },
        ]);
        assert!(!has(&f, "T-003"));
    }

    #[test]
    fn a_one_way_link_state_view_is_flagged_as_a_pair() {
        let mut a = router("a", "aaaa");
        a.regions = vec![north(2, &["aaaa", "bbbb"])];
        a.route_successor_count = 2;
        let mut b = router("b", "bbbb");
        b.regions = vec![north(1, &["bbbb"])];
        b.route_successor_count = 0;

        let f = evaluate(&[fresh(&a), fresh(&b)]);
        let t4 = f.iter().find(|x| x.id == "T-004").expect("T-004");
        assert_eq!(t4.subject, "a ↔ b", "a pair finding must name both ends");
    }

    #[test]
    fn a_settled_shape_raises_nothing_alarming() {
        // A settled router: north V=3, 6 route entries, 13 clients.
        let mut s = router("mgmt-01", "8b9a");
        s.regions = vec![north(3, &["8b9a", "4b54", "fd12"])];
        s.route_successor_count = 6;
        s.sessions = (0..13)
            .map(|i| SessionV1 {
                peer_zid: format!("c{i}"),
                whatami: "client".into(),
                region: "south:0:client".into(),
                link_dst: vec![],
            })
            .collect();
        let f = evaluate(&[fresh(&s)]);
        assert!(!has(&f, "T-005"), "6 == 3*2, the M-112 invariant holds");
        assert!(!has(&f, "T-008"), "three routers is not an isolated router");
        assert!(
            f.iter().all(|x| x.severity >= Severity::Advisory),
            "a healthy goatnet should raise nothing above advisory"
        );
    }

    #[test]
    fn a_route_table_that_contradicts_the_invariant_is_flagged() {
        let mut s = router("a", "aaaa");
        s.regions = vec![north(3, &["aaaa", "bbbb", "cccc"])];
        s.route_successor_count = 30; // the `@/**` fan-out shape: inflated
        assert!(has(&evaluate(&[fresh(&s)]), "T-005"));
    }

    #[test]
    fn a_config_that_disagrees_with_the_running_router_is_critical() {
        // Custom declaring subregions 0 and 1, but only 0 is running: the file
        // was edited and the router never restarted to read it.
        let mut s = router("a", "aaaa");
        s.gateway_mode = GatewayMode::Custom as i32;
        s.custom_subregions = vec![
            GatewaySubregionV1 {
                index: 0,
                filters_present: true,
                filters: vec![],
            },
            GatewaySubregionV1 {
                index: 1,
                filters_present: true,
                filters: vec![],
            },
        ];
        s.regions = vec![
            north(2, &["aaaa", "bbbb"]),
            RegionV1 {
                name: "south:0:router".into(),
                link_state_v: 3,
                link_state_e: 2,
                member_zids: vec!["aaaa".into(), "dddd".into(), "eeee".into()],
                session_count: 2,
            },
        ];
        s.route_successor_count = 8;
        let f = evaluate(&[fresh(&s)]);
        let t10 = f.iter().find(|x| x.id == "T-010").expect("T-010");
        assert_eq!(t10.severity, Severity::Critical);
    }

    /// Regression pin. `Preset(Auto)` with an empty config produces
    /// `south:0:peer` V=3 E=0 alongside `north`, and T-010 must not call that a
    /// Critical config/runtime divergence. Auto computes its regions at runtime —
    /// there is nothing declared for them to diverge from — so the only honest answer
    /// under Auto is silence.
    #[test]
    fn auto_with_a_populated_south_region_is_not_a_divergence() {
        let mut s = router("a", "aaaa");
        s.gateway_mode = GatewayMode::Auto as i32;
        s.custom_subregions = vec![];
        s.regions = vec![
            north(2, &["aaaa", "bbbb"]),
            RegionV1 {
                name: "south:0:peer".into(),
                link_state_v: 3,
                link_state_e: 0,
                member_zids: vec!["aaaa".into(), "dddd".into(), "eeee".into()],
                session_count: 2,
            },
        ];
        s.route_successor_count = 2; // north only; the edgeless region adds none
        assert!(!has(&evaluate(&[fresh(&s)]), "T-010"));
    }

    #[test]
    fn unreadable_config_is_reported_and_never_silently_treated_as_auto() {
        let mut s = router("a", "aaaa");
        s.gateway_mode = GatewayMode::Unreadable as i32;
        s.gateway_read_error = "/etc/x/config.json5: permission denied".into();
        assert!(has(&evaluate(&[fresh(&s)]), "T-011"));
    }

    #[test]
    fn growth_advisory_states_its_own_bound() {
        let mut s = router("a", "aaaa");
        s.regions = vec![north(3, &["aaaa", "bbbb", "cccc"])];
        s.route_successor_count = 6;
        let f = evaluate(&[fresh(&s)]);
        let t9 = f.iter().find(|x| x.id == "T-009").expect("T-009");
        assert_eq!(t9.severity, Severity::Advisory);
        assert!(
            t9.steps
                .iter()
                .any(|s| s.contains("two groups of three routers")),
            "a projection must state the measurement that bounds it: {:?}",
            t9.steps
        );
    }
}
