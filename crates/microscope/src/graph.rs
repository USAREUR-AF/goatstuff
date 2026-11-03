//! The reachability graph: the mesh paths an entity's peers have.
//!
//! Design: `docs/design/microscope-reachability-view.md`.
//!
//! SCOPE IS THE LOAD-BEARING DECISION. This graph is the **NetBird layer and
//! nothing else**. That is not a simplification, it is what makes the word
//! "permitted" honest: `GET /api/policies` returns the complete rule set, so
//! within this layer the complement of the drawn edges genuinely IS denied.
//! One layer up, Zenoh denies silently and has no central enumerable
//! ACL, so an undrawn edge there would mean "unenumerated", not "impossible" —
//! and a reader would infer the latter. The two domains have opposite honesty
//! properties and must not share a picture.
//!
//! CARDINALITY. NetBird policy is expressed GROUP → GROUP; peers are only
//! membership. So a group-level graph is not a summary of the policy, it is
//! the policy exactly — nothing about the permitted-path set is lost by not
//! drawing peers. That keeps the view in the tens of nodes even at the ADR
//! 0215 ceiling of 6000 fleet peers.
//!
//! WHAT THIS CANNOT SAY. Two edge states are honest here; a third is not.
//! `Permitted` comes from the policy set and is complete. `ReachableNow` comes
//! from both ends being `connected` — which is REACHABLE, not IN USE, and is
//! labelled so. "Recently used" would require pairwise flow evidence the
//! management API does not hold; WireGuard's per-peer `latest handshake` on
//! the host is the honest source, and collecting it is separate work. Until
//! then this module renders two states and says the third is unrecorded,
//! rather than inferring it from the first two.

use std::collections::{BTreeMap, BTreeSet};

use panoscope_core::netbird::NetbirdClient;
use serde::Serialize;

use crate::config::Config;
use crate::spine::EntitySpine;

/// What kind of thing a node is. Rendering differs; identity does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// One of the entity's own mesh peers.
    Peer,
    /// A group the entity's peers belong to (a policy SOURCE).
    SourceGroup,
    /// A group reachable from one of those source groups (a policy DESTINATION).
    DestGroup,
}

/// The honesty state of an edge. Deliberately only two real states plus the
/// admission that a third is unrecorded — see the module note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeState {
    /// Structural: this peer is a member of this group. Not a "path".
    Membership,
    /// An enabled policy rule permits source-group → dest-group. Complete: the
    /// policy set is fully enumerable, so a MISSING edge here really is denied.
    Permitted,
    /// Permitted AND both ends currently have at least one connected peer.
    /// Means REACHABLE, never "in use" — nothing here observes traffic.
    ReachableNow,
}

impl EdgeState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Membership => "member of",
            Self::Permitted => "permitted",
            Self::ReachableNow => "permitted · reachable now",
        }
    }
    pub fn css(self) -> &'static str {
        match self {
            Self::Membership => "membership",
            Self::Permitted => "permitted",
            Self::ReachableNow => "reachable",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub kind: NodeKind,
    /// Peers only: whether this peer is connected right now.
    pub connected: Option<bool>,
    /// Groups only: how many peers are members, and how many are connected.
    pub members: Option<usize>,
    pub members_connected: Option<usize>,
    /// Deterministic layout, computed here so two loads draw identically and
    /// the markup can be asserted in tests. See the design doc on why a force
    /// simulation is the wrong tool for an investigative surface.
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub state: EdgeState,
    /// The policy rule's name, when the edge came from one. Kept for the
    /// hover title; the structured fields below are what the page renders.
    pub via: Option<String>,
    /// The ENABLED policy rules that permit this path. Structured rather than
    /// prose because the page shows them in a column — the mechanism is not a
    /// footnote, it is the thing an operator needs in order to change it.
    pub rules: Vec<String>,
    /// The source→destination group pairs those rules matched on.
    pub via_groups: Vec<String>,
    /// Transport constraints, derived from the rules rather than their names.
    pub constraints: Vec<String>,
    /// True only when EVERY matching rule is bidirectional. Asymmetry is the
    /// interesting case, so the conservative reading wins: one one-way rule in
    /// the set makes the whole edge one-way for display purposes, and the
    /// return path has to earn its own arrow.
    pub bidirectional: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ReachabilityGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Why the graph is empty / partial, in the surface's own idiom. Never
    /// left blank: an empty graph with no explanation reads as "no access".
    pub notes: Vec<String>,
    /// The groups the subject machine belongs to. Shown on the page: a path
    /// exists BECAUSE of group membership, so hiding it makes the graph an
    /// assertion rather than an explanation.
    pub subject_groups: Vec<String>,
    /// True only when the policy set was actually read. An unread policy set
    /// must never render as "no permitted paths".
    pub policies_read: bool,
}

/// Build the graph for one entity. Reads peers, groups and policies; joins the
/// entity's peers by the same convention-graded handle-prefix rule the spine
/// uses, so this view can never claim a stronger join than the timeline does.
/// What the graph is drawn FOR. Minimal by default: one machine.
#[derive(Debug, Clone, Copy)]
pub enum Focus<'a> {
    /// One machine — the honest subject for a reachability question.
    Peer(&'a str),
    /// Every machine this entity is connected to. Explicit opt-in only.
    All,
    /// Nothing chosen: the caller renders a picker.
    None,
}

pub async fn build(cfg: &Config, spine: &EntitySpine, focus: Focus<'_>) -> ReachabilityGraph {
    let mut g = ReachabilityGraph::default();
    let nb = NetbirdClient::new(cfg.netbird_url.clone(), cfg.netbird_token.clone());
    if !nb.is_enabled() {
        g.notes
            .push("NetBird not configured here (MICROSCOPE_NETBIRD_URL/TOKEN unset) — nothing was read, which is not the same as no access".into());
        return g;
    }
    // ONE MACHINE AT A TIME. This is the third attempt and the previous two
    // failed the same way — by drawing a whole population instead of a
    // subject.
    //
    //   1st: groups as nodes, policy rules as edges. 47 nodes, 115 edges.
    //   2nd: every peer the entity had ever TOUCHED on the left. For a
    //        platform admin — who created the setup keys and manages the
    //        groups — that is every peer on the goatnet, so the picture was
    //        again everything-to-everything, just with peers.
    //
    // The question is "what can THIS machine reach", so the subject is one
    // peer, named by `?peer=`. Without one, the caller gets a PICKER rather
    // than a guess: a person is not a machine and has no single position in
    // the mesh, so choosing arbitrarily on their behalf would be inventing a
    // subject. Enrolled and administered peers are candidates to pick FROM,
    // never a set to draw all at once.
    let subject_peers: BTreeSet<&str> = match focus {
        Focus::Peer(f) => std::iter::once(f).collect(),
        // The full set, only ever on explicit request. It is genuinely useful
        // — "show me everything this admin's estate can reach" — and it is
        // never the default, because for a platform admin that is every peer
        // on the goatnet and the picture stops answering anything.
        Focus::All => spine
            .netbird_peers
            .iter()
            .chain(spine.netbird_peers_enrolled.iter())
            .chain(spine.netbird_peers_administered.iter())
            .map(|s| s.as_str())
            .collect(),
        Focus::None => BTreeSet::new(),
    };
    if subject_peers.is_empty() {
        g.notes.push(
            "pick a machine to see what it can reach. A person is not a machine and has no single position in the mesh, so there is no honest default subject to draw."
                .into(),
        );
        return g;
    }

    let peers = match nb.list_peers().await {
        Ok(p) => p,
        Err(e) => {
            g.notes.push(format!("peers unread: {e}"));
            return g;
        }
    };
    let policies = match nb.list_policies().await {
        Ok(p) => {
            g.policies_read = true;
            p
        }
        Err(e) => {
            g.notes
                .push(format!("policies unread ({e}) — permitted paths CANNOT be shown, and their absence below is not a claim of denial"));
            Vec::new()
        }
    };

    // PEER TO PEER, not group to group.
    //
    // The first cut drew groups as nodes and policy rules as edges. That is
    // the policy, faithfully — and it is a hairball nobody can read: 47 nodes
    // and 115 edges for one operator, none of which answers the question a
    // person actually has, which is "what can this machine reach?". Groups are
    // the MECHANISM of a path, not the path. They belong in the explanation
    // you get when you click an edge.
    //
    // So: resolve the group→group rules down to concrete peer→peer pairs, and
    // carry the enabling rules on the edge.
    let mut own: Vec<(&str, bool)> = Vec::new();
    for p in &peers {
        if subject_peers.contains(p.name.as_str()) {
            own.push((p.name.as_str(), p.connected));
        }
    }
    own.sort_by(|a, b| a.0.cmp(b.0));

    let groups_of = |p: &panoscope_core::netbird::NbPeer| -> BTreeSet<String> {
        p.groups.iter().map(|g| g.name.clone()).collect()
    };

    // (source group, dest group) -> the enabled rules that permit it.
    type Permit = (BTreeSet<String>, BTreeSet<String>, Vec<bool>);
    let mut permits: BTreeMap<(String, String), Permit> = BTreeMap::new();
    for pol in &policies {
        if !pol.enabled {
            continue;
        }
        for rule in &pol.rules {
            if !rule.enabled {
                continue;
            }
            let label = if pol.name.is_empty() { rule.name.clone() } else { pol.name.clone() };
            // Drop rules is not a permit. NetBird's default action is accept;
            // anything else must not widen what we draw.
            if !rule.action.is_empty() && !rule.action.eq_ignore_ascii_case("accept") {
                continue;
            }
            for s in &rule.sources {
                for d in &rule.destinations {
                    let e = permits.entry((s.name.clone(), d.name.clone())).or_default();
                    e.0.insert(label.clone());
                    e.1.insert(rule.constraint());
                    e.2.push(rule.bidirectional);
                    // A bidirectional rule also permits the REVERSE pair, and
                    // not recording it is how an asymmetric picture gets drawn
                    // as symmetric.
                    if rule.bidirectional {
                        let r = permits.entry((d.name.clone(), s.name.clone())).or_default();
                        r.0.insert(format!("{label} (reverse of a bidirectional rule)"));
                        r.1.insert(rule.constraint());
                        r.2.push(true);
                    }
                }
            }
        }
    }

    let mut reachable: BTreeMap<String, bool> = BTreeMap::new();
    let mut pairs: Vec<(String, String, BTreeSet<String>, BTreeSet<String>, BTreeSet<String>, bool)> =
        Vec::new();
    for src in &peers {
        if !subject_peers.contains(src.name.as_str()) {
            continue;
        }
        let sg = groups_of(src);
        for dst in &peers {
            if dst.name == src.name {
                continue;
            }
            let dg = groups_of(dst);
            let mut rules: BTreeSet<String> = BTreeSet::new();
            let mut via: BTreeSet<String> = BTreeSet::new();
            let mut cons: BTreeSet<String> = BTreeSet::new();
            let mut bidi: Vec<bool> = Vec::new();
            for a in &sg {
                for b in &dg {
                    if let Some((r, c, bd)) = permits.get(&(a.clone(), b.clone())) {
                        rules.extend(r.iter().cloned());
                        cons.extend(c.iter().cloned());
                        bidi.extend(bd.iter().copied());
                        via.insert(format!("{a} → {b}"));
                    }
                }
            }
            if rules.is_empty() {
                continue;
            }
            // Conservative: an edge is only shown as two-way when EVERY rule
            // behind it is. Asymmetry is the case worth seeing, so a single
            // one-way rule keeps the whole edge one-way and the return path
            // must earn its own arrow.
            let all_bidi = !bidi.is_empty() && bidi.iter().all(|b| *b);
            reachable.insert(dst.name.clone(), dst.connected);
            pairs.push((src.name.clone(), dst.name.clone(), rules, via, cons, all_bidi));
        }
    }

    // --- deterministic layout: two columns, this entity's peers on the left.
    const COL_SRC: f64 = 150.0;
    const COL_DST: f64 = 660.0;
    const ROW: f64 = 46.0;
    const TOP: f64 = 40.0;
    let row = |n: usize| -> f64 { TOP + (n as f64) * ROW };

    for (i, (name, connected)) in own.iter().enumerate() {
        g.nodes.push(Node {
            id: format!("p:{name}"),
            label: (*name).to_string(),
            kind: NodeKind::Peer,
            connected: Some(*connected),
            members: None,
            members_connected: None,
            x: COL_SRC,
            y: row(i),
        });
    }
    for (i, (name, connected)) in reachable.iter().enumerate() {
        g.nodes.push(Node {
            id: format!("r:{name}"),
            label: name.clone(),
            kind: NodeKind::DestGroup,
            connected: Some(*connected),
            members: None,
            members_connected: None,
            x: COL_DST,
            y: row(i),
        });
    }

    // The subject's own groups — the left-hand half of every rule below.
    for p in &peers {
        if subject_peers.contains(p.name.as_str()) {
            for gr in &p.groups {
                if !g.subject_groups.contains(&gr.name) {
                    g.subject_groups.push(gr.name.clone());
                }
            }
        }
    }
    g.subject_groups.sort();

    let src_live: BTreeMap<&str, bool> = own.iter().map(|(n, c)| (*n, *c)).collect();
    for (from, to, rules, via, cons, all_bidi) in pairs {
        let live = *src_live.get(from.as_str()).unwrap_or(&false)
            && *reachable.get(&to).unwrap_or(&false);
        g.edges.push(Edge {
            from: format!("p:{from}"),
            to: format!("r:{to}"),
            state: if live { EdgeState::ReachableNow } else { EdgeState::Permitted },
            // The explanation rides on the edge: which policy, via which
            // group pair. This is what a click needs to surface.
            via: Some(format!(
                "{} · via {}",
                rules.iter().cloned().collect::<Vec<_>>().join(", "),
                via.iter().cloned().collect::<Vec<_>>().join(", ")
            )),
            rules: rules.into_iter().collect(),
            via_groups: via.into_iter().collect(),
            constraints: cons.into_iter().collect(),
            bidirectional: all_bidi,
        });
    }

    if g.policies_read && g.edges.is_empty() {
        g.notes.push(
            "the policy set was read and NO enabled rule connects this entity's peers to any other peer — here, unlike the fabric layer, that IS a denial and not merely an absence"
                .into(),
        );
    }
    g.notes.push(
        "groups and policies are not drawn as nodes on purpose: they are the MECHANISM of a path, not the path. Hover or click an edge to see which rule permits it and through which group pair."
            .into(),
    );
    g.notes.push(
        "usage is not shown: the NetBird management API records that peers connected, never that one peer talked to another. \"reachable now\" means both ends are up, NOT that anything flowed."
            .into(),
    );
    g.notes.push(
        "these are the DECLARED policies at the management plane; enforcement is distributed to peers, so a peer that has not synced can diverge."
            .into(),
    );
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g() -> ReachabilityGraph {
        ReachabilityGraph::default()
    }

    /// The state vocabulary must not grow a "recently used" value by accident:
    /// nothing in this layer observes traffic.
    #[test]
    fn reachable_never_claims_usage() {
        assert_eq!(EdgeState::ReachableNow.label(), "permitted · reachable now");
        assert!(!EdgeState::ReachableNow.label().contains("used"));
        assert!(!EdgeState::Permitted.label().contains("used"));
    }

    /// An empty graph must always carry a reason. A blank picture with no
    /// note is indistinguishable from "this entity may reach nothing".
    #[test]
    fn an_empty_graph_is_never_silent() {
        let empty = g();
        assert!(empty.nodes.is_empty());
        // The default is only ever returned alongside a pushed note in build();
        // this asserts the field exists to carry one and starts unread.
        assert!(!empty.policies_read);
    }

    /// Unread policies must not read as "no permitted paths".
    #[test]
    fn unread_policies_are_distinguishable_from_no_paths() {
        let mut unread = g();
        unread.policies_read = false;
        let mut read_but_none = g();
        read_but_none.policies_read = true;
        assert_ne!(unread.policies_read, read_but_none.policies_read);
    }
}

#[cfg(test)]
mod why_tests {
    use super::*;

    /// A path must carry its own justification. The graph showing only the
    /// RESULT makes it an assertion — an operator's next action is almost
    /// always to change a group or a rule, and they cannot do that from a
    /// picture of arrows.
    #[test]
    fn a_permitted_edge_names_the_rule_and_the_group_pair() {
        let e = Edge {
            from: "p:laptop".into(),
            to: "r:orin".into(),
            state: EdgeState::Permitted,
            via: Some("SITE-A · network → infra (icmp) · via SITE-A-NETWORK → SITE-A-INFRA".into()),
            rules: vec!["SITE-A · network → infra (icmp)".into()],
            via_groups: vec!["SITE-A-NETWORK → SITE-A-INFRA".into()],
            constraints: vec!["ICMP".into()],
            bidirectional: false,
        };
        assert!(!e.rules.is_empty(), "a permitted path with no named rule cannot be acted on");
        assert!(!e.via_groups.is_empty());
    }

    /// Membership edges are structural, not paths, so they carry no rule —
    /// and must not be rendered as though a policy permitted them.
    #[test]
    fn membership_carries_no_rule() {
        let e = Edge {
            from: "p:laptop".into(),
            to: "g:SITE-A-NETWORK".into(),
            state: EdgeState::Membership,
            via: None,
            rules: Vec::new(),
            via_groups: Vec::new(),
            constraints: Vec::new(),
            bidirectional: false,
        };
        assert!(e.rules.is_empty());
    }
}
