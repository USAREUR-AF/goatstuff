//! Curated reachability answers.
//!
//! These are the questions an operator actually asks, answered directly. The
//! NetBird API can supply the raw material for all of them — groups, policies,
//! peers — but not one of them is a call you can make: every one requires
//! joining three endpoints and resolving group membership to peers yourself,
//! which is exactly the work people get wrong by hand.
//!
//! So this is a superset, not a mirror. It adds:
//!
//!   - resolution: group→group rules resolved down to peer→peer reality
//!   - attribution: not just "can they" but WHICH rule permits it, with the
//!     protocol and ports off the rule itself
//!   - the mediation distinction: reach via the pair you asked about versus
//!     reach a peer has through its other memberships — the thing that makes a
//!     group-scoped answer understate real reach
//!   - derived identity, so an answer is readable without a lookup table
//!
//! Every response carries the same honesty header as the visualization: an
//! unread source is never rendered as an empty answer. "Nothing is permitted"
//! and "we could not read what is permitted" must never serialize the same.

use serde::Serialize;
use serde_json::{json, Value};

use panoscope_core::meshgraph::netvis::Doc;
use panoscope_core::portsets::{IntentIndex, Vocabulary};

/// The warrant vocabulary (ADR 1129), compiled in HERE rather than in
/// panoscope-core — that crate is linked by microscope, stethoscope,
/// panoscope and flashlight, and none of them renders a port-set.
const PORT_SETS_YAML: &str = include_str!("../../../ops/warrant/port-sets.yaml");

/// Names the goatnet's rule-intents file (ops/warrant/rule-intents/
/// netbird-acl.yaml in its tree). The deployment's own, never compiled in;
/// unset or unreadable, rules carry no intent annotation.
pub const RULE_INTENTS_ENV: &str = "NETVIS_RULE_INTENTS_FILE";

/// The rule-intents text: the file `NETVIS_RULE_INTENTS_FILE` names.
#[cfg(not(test))]
pub(crate) fn rule_intents_yaml() -> String {
    let Some(p) = std::env::var_os(RULE_INTENTS_ENV).filter(|v| !v.is_empty()) else {
        eprintln!("netvis: no rule intents: set {RULE_INTENTS_ENV} to the goatnet's rule-intents file; rules carry no intent annotation until then");
        return String::new();
    };
    std::fs::read_to_string(&p).unwrap_or_else(|e| {
        eprintln!("netvis: {RULE_INTENTS_ENV}={} cannot be read: {e}; rules carry no intent annotation", std::path::Path::new(&p).display());
        String::new()
    })
}

/// Tests read the rule intents this repository carries, as a fixture.
#[cfg(test)]
pub(crate) fn rule_intents_yaml() -> String {
    include_str!("../../../ops/warrant/rule-intents/netbird-acl.yaml").to_string()
}

/// Parsed once per process, not per request.
///
/// ADR 1123: client demand must never amplify into load. The files are read
/// once per process, so the lens adds no NetBird call, no fabric query and no
/// per-request I/O — a pair view costs exactly what it cost
/// before, plus two map lookups.
fn warrant_vocab() -> &'static (Vocabulary, IntentIndex) {
    static CELL: std::sync::OnceLock<(Vocabulary, IntentIndex)> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        // A registry that fails to parse degrades to no annotation. The CI
        // gate (ops/ci/check-warrant-registry.py) is what refuses a broken
        // registry; a rendering surface must not go dark over a defect it did
        // not cause and cannot fix.
        (
            Vocabulary::parse(PORT_SETS_YAML).unwrap_or_default(),
            IntentIndex::parse(&rule_intents_yaml()),
        )
    })
}

/// Everything an answer says about how much it trusts itself.
#[derive(Serialize)]
pub struct Provenance {
    pub site: String,
    pub generated_at: String,
    pub policies_read: bool,
    pub groups_read: bool,
    pub peers_read: bool,
    /// Verbatim upstream failure reasons. `[]` is a claim that nothing failed.
    pub notes: Vec<String>,
    /// True when every source answered. A caller automating against this
    /// should gate on it rather than re-deriving the three booleans.
    pub complete: bool,
}

impl Provenance {
    pub fn of(d: &Doc) -> Self {
        Self {
            site: d.site.clone(),
            generated_at: d.generated_at.clone(),
            policies_read: d.policies_read,
            groups_read: d.groups_read,
            peers_read: d.peers_read,
            notes: d.notes.clone(),
            complete: d.policies_read && d.groups_read && d.peers_read,
        }
    }
}

/// Resolve a peer selector to an index: exact NetBird name, then id, then IP,
/// then unique case-insensitive substring. Ambiguity is an error, never a
/// silent first-match — on a reachability answer the wrong peer is worse than
/// no answer.
pub fn find_peer(d: &Doc, sel: &str) -> Result<usize, Value> {
    let peers = match d.peers.as_ref() {
        Some(p) => p,
        None => {
            return Err(json!({
                "error": "the peer list was not read",
                "note": "This is not a claim that the goatnet has no peers.",
            }))
        }
    };
    let s = sel.trim();
    if let Some(i) = peers.iter().position(|p| p.name == s || p.id == s || p.ip == s) {
        return Ok(i);
    }
    let lc = s.to_lowercase();
    let hits: Vec<usize> = peers
        .iter()
        .enumerate()
        .filter(|(_, p)| p.name.to_lowercase().contains(&lc))
        .map(|(i, _)| i)
        .collect();
    match hits.len() {
        1 => Ok(hits[0]),
        0 => Err(json!({ "error": format!("no peer matches {s:?}") })),
        _ => Err(json!({
            "error": format!("{s:?} matches {} peers — be specific", hits.len()),
            "matches": hits.iter().map(|i| peers[*i].name.clone()).collect::<Vec<_>>(),
        })),
    }
}

/// Every group→group edge that permits `a → b`, with its constraint and the
/// rules behind it. The whole point of the surface, as a function.
pub fn paths(d: &Doc, a: usize, b: usize) -> Vec<Value> {
    let peers = match d.peers.as_ref() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let (pa, pb) = (&peers[a], &peers[b]);
    d.edges
        .iter()
        .filter(|e| pa.groups.contains(&e.s) && pb.groups.contains(&e.d))
        .map(|e| {
            json!({
                "via_group_pair": format!("{} → {}", d.nodes[e.s].name, d.nodes[e.d].name),
                "source_group": d.nodes[e.s].name,
                "destination_group": d.nodes[e.d].name,
                // Constraints come off the RULE, never off a policy name — a
                // policy called "p2p → gear (any port)" is a label, not a fact.
                "constraint": e.constraints,
                "unrestricted": e.unrestricted,
                "direction": e.dir,
                "rules": e.rules.iter().filter_map(|ri| d.rules.get(*ri)).map(|r| {
                    let (vocab, intents) = warrant_vocab();
                    // The warrant lens (ADR 1129). Names the port-set this
                    // rule's ports ARE, so a reader sees "substrate" and its
                    // stated purpose instead of four bare numbers they have
                    // to recognise. `null` where the shape is not in the
                    // vocabulary — which is a real answer, not a gap: it
                    // means this rule is not expressible in the registry yet.
                    let set = vocab.resolve(&r.protocol, &r.ports);
                    let meta = intents.get(r.rule_name.as_str());
                    json!({
                        "policy": r.policy,
                        "policy_id": r.policy_id,
                        "protocol": r.protocol,
                        "ports": r.ports,
                        "constraint": r.constraint,
                        "bidirectional": r.bidirectional,
                        "port_set": set.map(|s| s.name.clone()),
                        "port_set_purpose": set.map(|s| s.purpose.trim().to_string()),
                        "port_set_owner": set.map(|s| s.owner.clone()),
                        // Registry-only metadata, stripped before the ACL is
                        // applied, so it exists NOWHERE in the live plane —
                        // which is exactly why it is worth surfacing here.
                        "expires": meta.and_then(|m| m.expires.clone()),
                        "grant_owner": meta.and_then(|m| m.owner.clone()),
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect()
}

#[cfg(test)]
mod warrant_lens_tests {
    use super::*;

    const TAXONOMY: &str = include_str!("../../../ops/ansible/netbird_acl_taxonomy.yml");
    const MATRIX: &str = include_str!("../../../ops/ansible/netbird_acl_matrix.yml");

    /// The lens is only worth rendering if it actually names real rules.
    ///
    /// Every port-carrying permit in the shipped declaration must resolve to a
    /// port-set. This holds by construction — `ops/warrant/extract.py` refuses
    /// to produce the registry if any `(protocol, ports)` is unnamed — but
    /// "by construction" is an argument, and this is the measurement. If a
    /// future rule introduces an unnamed shape, the lens would quietly render
    /// `port_set: null` on it and nobody would notice; this fails instead.
    #[test]
    fn every_port_carrying_rule_in_the_real_declaration_resolves_to_a_set() {
        if !std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ops/goatbuilder/goatbuilder.yaml").is_file() {
            return; // checks this deployment's own declaration; a public clone carries an example one
        }
        let (vocab, _) = warrant_vocab();
        assert!(!vocab.all().is_empty(), "vocabulary parsed");

        let decl = panoscope_core::aclcheck::Declaration::parse(TAXONOMY, MATRIX)
            .expect("the shipped declaration parses");

        let mut checked = 0usize;
        let mut unnamed: Vec<String> = Vec::new();
        for scope in decl.scopes() {
            let model = decl.model(&scope).expect("scope resolves");
            for permit in &model.permits {
                if permit.ports.is_empty() {
                    continue; // `all` / icmp carry no ports and name no set
                }
                checked += 1;
                if vocab.resolve(&permit.protocol, &permit.ports).is_none() {
                    unnamed.push(format!(
                        "[{scope}] {} {:?} (rule: {})",
                        permit.protocol, permit.ports, permit.rule
                    ));
                }
            }
        }

        assert!(checked > 0, "found port-carrying permits to check");
        assert!(
            unnamed.is_empty(),
            "{} of {checked} port-carrying permits resolve to no port-set:\n  {}",
            unnamed.len(),
            unnamed.join("\n  ")
        );
    }

    /// `protocol: all` and icmp rules carry no ports and must stay unnamed —
    /// inventing a set for them would claim the registry expresses something
    /// it does not.
    #[test]
    fn portless_rules_resolve_to_no_set() {
        let (vocab, _) = warrant_vocab();
        assert!(vocab.resolve("all", &[]).is_none());
        assert!(vocab.resolve("icmp", &[]).is_none());
    }
}

// ===========================================================================
// SCOPE STEWARDSHIP — who to ask about a machine that owns no person
// ===========================================================================
//
// The ownership lens answers "who is responsible for this machine" by resolving
// the peer NAME to a person. That works for operator-onboarded peers and says
// nothing at all about a machine: most machines — Orins, relays, breakglass, gear
// at a forward scope — carry no principal in the name, and every one of them would
// render as `no principal in the name`. True, and useless: the viewer still has
// nobody to ask.
//
// But a machine is never scopeless. Its GROUPS carry the scope it belongs to
// (`SITE-A-KID`, `GALAXY-PROD-INFRA`), and the mesh already names who administers
// a scope: `<SCOPE>-PLATFORM-ADMIN`. That is not an arbitrary pick of suffix —
// ADR 1124 makes PLATFORM-ADMIN the ONLY qualified source permitted to reach
// inward across scopes (`acl_scope.py` QUALIFIED_SOURCE_SUFFIXES), i.e. it is
// definitionally the group that may act on another scope's machines. If anyone
// is answerable for a box, it is that group.
//
// PROVENANCE, and it is a different one from the person lens. This is derived
// from the MESH plane — the ACL groups netvis already read — not from the
// portal's registry. It is a weaker claim than a resolved identity and must
// never be dressed as the same thing: it says "these principals administer the
// scope this machine sits in", not "this person owns this machine". The two
// travel in separate fields for that reason (two facts, two channels), and the
// renderer labels this one `scope`.
//
// It costs nothing: no call, no new source, no PII. Everything below is read
// off the Doc that was already built to draw the matrix.

/// One principal who administers a scope.
#[derive(Debug, Clone, Serialize)]
pub struct ScopeAdmin {
    /// The admin peer's NetBird name — the join key the ownership lens takes,
    /// so a caller can resolve it to a person with the same resolver.
    pub netbird_name: String,
    pub connected: bool,
}

/// One PERSON who administers a scope, with the devices they hold in the admin
/// group folded under them.
///
/// The group's members are PEERS — machines — so listing them raw answers "who
/// do I ask" with a list of laptops. A group's members resolve to fewer people,
/// because one person can hold two or three devices in the group at once. A reader chasing an
/// unowned machine wants the human.
///
/// Do not name specific peers here as belonging to one person. A comment is not
/// a custody record and cannot be checked by anything — the resolver below is the
/// answer, and it reads the registry.
#[derive(Debug, Clone, Serialize)]
pub struct AdminPrincipal {
    /// Resolved person, when the ownership lens could name one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person_name: Option<String>,
    /// Operator handle parsed from the peer name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id_console_path: Option<String>,
    /// The devices this principal holds IN THE ADMIN GROUP — not all their
    /// machines, only the ones that carry the administering membership.
    pub devices: Vec<String>,
    pub devices_online: usize,
    /// True when nothing could be resolved and these devices are grouped only
    /// by "we could not name anyone". NEVER merged with a named principal.
    pub unresolved: bool,
}

/// One additional scope a machine sits in, and who administers it.
#[derive(Debug, Clone, Serialize)]
pub struct ScopeAdminGroup {
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admin_group: Option<String>,
    pub admins: Vec<ScopeAdmin>,
    #[serde(default)]
    pub principals: Vec<AdminPrincipal>,
    /// The goatnet scope hosting this one, when this is a site rung below a
    /// goatnet. See `Stewardship::host_scope`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_scope: Option<String>,
}

/// Who administers the scope a machine belongs to.
#[derive(Debug, Clone, Serialize)]
pub struct Stewardship {
    /// The scope family, e.g. `SITE-A`, `GALAXY-PROD`. Absent when the peer holds
    /// no scoped group at all.
    pub scope: Option<String>,
    /// The group consulted, named so the answer is checkable rather than
    /// magic — an operator can go read its membership.
    pub admin_group: Option<String>,
    pub admins: Vec<ScopeAdmin>,
    /// The same members folded to PEOPLE. Filled after the ownership lens has
    /// run, so it is empty on the pure mesh-derived pass.
    #[serde(default)]
    pub principals: Vec<AdminPrincipal>,
    /// The OTHER scopes this machine belongs to, each with its own
    /// administering group. Present because a machine in two scopes has two
    /// sets of people who may act on it, and naming only one is the arbitrary
    /// pick this field exists to stop.
    #[serde(default)]
    pub also_scopes: Vec<ScopeAdminGroup>,
    /// True when this scope is the ONLY scope on the goatnet — i.e. "scope
    /// GALAXY" means the whole goatnet and every machine gets the same answer.
    ///
    /// Reported because it changes what the row MEANS. On a multi-scope goatnet
    /// (a forward site vs the goatnet's own scope) naming the scope narrows the question
    /// usefully. On a single-scope one it is tautological — "who administers
    /// this machine? the people who administer everything" — and the word
    /// "scope" implies a subdivision that does not exist. Saying so is better
    /// than letting a reader hunt for the distinction.
    pub scope_is_whole_goatnet: bool,
    /// The goatnet scope that HOSTS this one, from `netbird_exercises`.
    /// `EX-DEMO` → `GALAXY`, `SITE-A` → `GALAXY-PROD`. `None` for a goatnet's own
    /// scope, which nothing hosts.
    ///
    /// THIS IS WHY AN EMPTY ADMIN GROUP IS NOT A GAP. The canonical group set
    /// is complete-by-prefix so every scope has the same shape whether or not
    /// each group is populated, and a scope-local `PLATFORM-ADMIN` exists so an
    /// activity CAN appoint an admin for its duration without anyone becoming a
    /// permanent goatnet-wide one. Most scopes never appoint one. Reporting
    /// that as "nobody is named" made a deliberate design read as a finding,
    /// and a surface that cries about declared-and-empty trains people to
    /// ignore it — the same lesson `GearFamily::slots_absent` carries.
    ///
    /// The host does NOT inherit authority over the scope. `SITE-A-PLATFORM-
    /// ADMIN` reaches `SITE-A-INFRA` and nothing in the stamped rule set names
    /// `GALAXY-PROD-INFRA`; that boundary is deliberate. What the host's admins
    /// can act on is whatever host-scope groups the machine ALSO holds — which
    /// is how a machine in an exercise scope stays reachable, via
    /// `<HOST>-EXTERNAL-INFRA`. The row says where authority sits; it does not
    /// grant any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_scope: Option<String>,
    /// Why this is what it is, in the renderer's words. Always present:
    /// "no admins" and "no scope" are different facts and both need saying.
    pub basis: String,
}

/// Derive scope stewardship for one peer from the graph already in hand.
///
/// `All` is excluded deliberately: it is every peer on the goatnet, carries no
/// scope, and treating it as one would name the whole fleet as a machine's
/// stewards. A peer in several scopes reports the first by panel order, which
/// is the same order the matrix draws — the picture and the answer agree.
pub fn stewardship(
    d: &Doc,
    peer_index: usize,
    scope_hosts: &std::collections::BTreeMap<String, String>,
) -> Stewardship {
    let peers = match d.peers.as_ref() {
        Some(p) => p,
        None => {
            return Stewardship {
                scope: None,
                admin_group: None,
                admins: Vec::new(),
                principals: Vec::new(),
                also_scopes: Vec::new(),
                scope_is_whole_goatnet: false,
                host_scope: None,
                basis: "the peer list was not read — this is unknown, not absent".into(),
            }
        }
    };
    let p = &peers[peer_index];

    // EVERY scope this peer belongs to, in panel order — not the first one.
    //
    // Taking the first was arbitrary ordering deciding a responsibility answer,
    // and it is wrong on the peers that matter: a machine holding both
    // <HOST>-EXTERNAL-INFRA and EX-DEMO-POD is genuinely in the exercise scope,
    // and would report the host purely because the goatnet group sorts first in
    // its membership list. Pointing
    // a forward-scope machine at the goatnet's admins instead of the
    // exercise's is precisely the discrimination this exists to provide.
    //
    // A machine in two scopes has two sets of people who may act on it. That
    // is a fact about the machine, not an ambiguity to resolve by picking one.
    let mut scopes: Vec<String> = Vec::new();
    for g in &p.groups {
        if let Some(n) = d.nodes.get(*g) {
            if !n.family.is_empty() && n.family != "<unscoped>" && !scopes.contains(&n.family) {
                scopes.push(n.family.clone());
            }
        }
    }
    let scope = scopes.first().cloned();

    let scope = match scope {
        Some(s) => s,
        None => {
            return Stewardship {
                scope: None,
                admin_group: None,
                admins: Vec::new(),
                principals: Vec::new(),
                also_scopes: Vec::new(),
                scope_is_whole_goatnet: false,
                host_scope: None,
                basis: "this machine holds no scoped group, so no scope claims it".into(),
            }
        }
    };
    let host_scope = scope_hosts.get(&scope).cloned();

    // Is this the only scope on the goatnet? Counted from the declared families
    // rather than assumed from the site label: a goatnet grows scopes without
    // renaming itself, so this must be read each time.
    let mut families: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for n in &d.nodes {
        if !n.family.is_empty() && n.family != "<unscoped>" {
            families.insert(n.family.as_str());
        }
    }
    let only_scope = families.len() <= 1;

    // The admin group for that scope, found by (family, suffix) rather than by
    // string-building "<scope>-PLATFORM-ADMIN": the separator and the scope
    // prefix are conventions the graph has already parsed, and rebuilding the
    // name here would be another implementation of the peer-name grammar.
    let admin_node = d
        .nodes
        .iter()
        .find(|n| n.family == scope && n.suffix == "PLATFORM-ADMIN");

    let admin_node = match admin_node {
        Some(n) => n,
        None => {
            return Stewardship {
                scope: Some(scope.clone()),
                admin_group: None,
                admins: Vec::new(),
                principals: Vec::new(),
                also_scopes: Vec::new(),
                scope_is_whole_goatnet: only_scope,
                host_scope: host_scope.clone(),
                basis: match &host_scope {
                    Some(h) => format!(
                        "scope {scope} declares no PLATFORM-ADMIN group of its own; it is hosted on {h}, so authority sits with {h}-PLATFORM-ADMIN over whatever {h} groups this machine also holds"
                    ),
                    None => format!(
                        "scope {scope} declares no PLATFORM-ADMIN group, and nothing hosts it — nobody is named as able to act on its machines"
                    ),
                },
            }
        }
    };

    let admins: Vec<ScopeAdmin> = peers
        .iter()
        .filter(|q| q.groups.contains(&admin_node.i))
        .map(|q| ScopeAdmin { netbird_name: q.name.clone(), connected: q.connected })
        .collect();

    let basis = if admins.is_empty() {
        // AN EMPTY SCOPE-LOCAL ADMIN GROUP IS THE ORDINARY CASE, NOT A FINDING.
        //
        // This used to read "is DECLARED BUT EMPTY — names an administering
        // group with no members", which framed the design as a defect. Two
        // facts make it the design: the canonical set is complete-by-prefix so
        // every scope has the same shape, and a scope-local PLATFORM-ADMIN
        // exists so an activity CAN appoint an admin for its duration without
        // making anyone a permanent goatnet-wide one. Most never appoint one.
        //
        // What the row owes a reader is where authority DOES sit, which is the
        // host goatnet's admins acting through whatever host-scope groups the
        // machine also holds. Only a scope nothing hosts is genuinely
        // unanswerable, and that case still says so.
        match &host_scope {
            Some(h) => format!(
                "{} has no members — a scope appoints a local admin only when an activity needs one. {scope} is hosted on {h}, so authority sits with {h}-PLATFORM-ADMIN over whatever {h} groups this machine also holds",
                admin_node.name
            ),
            None => format!(
                "{} has no members, and nothing hosts scope {scope} — nobody is named as able to act on its machines",
                admin_node.name
            ),
        }
    } else {
        format!(
            "{} administers scope {scope} (ADR 1124: PLATFORM-ADMIN is the only qualified cross-scope source)",
            admin_node.name
        )
    };

    // The remaining scopes, each resolved to its own administering group.
    let also_scopes: Vec<ScopeAdminGroup> = scopes
        .iter()
        .skip(1)
        .map(|fam| {
            let node = d.nodes.iter().find(|n| &n.family == fam && n.suffix == "PLATFORM-ADMIN");
            let admins = node
                .map(|an| {
                    peers
                        .iter()
                        .filter(|q| q.groups.contains(&an.i))
                        .map(|q| ScopeAdmin { netbird_name: q.name.clone(), connected: q.connected })
                        .collect()
                })
                .unwrap_or_default();
            ScopeAdminGroup {
                scope: fam.clone(),
                admin_group: node.map(|n| n.name.clone()),
                admins,
                principals: Vec::new(),
                host_scope: scope_hosts.get(fam).cloned(),
            }
        })
        .collect();

    Stewardship {
        scope: Some(scope),
        admin_group: Some(admin_node.name.clone()),
        admins,
        principals: Vec::new(),
        also_scopes,
        scope_is_whole_goatnet: only_scope,
        host_scope,
        basis,
    }
}

/// Fold a scope's admin DEVICES into the PEOPLE who hold them.
///
/// The mesh half of stewardship is pure — group membership, no lookups. This is
/// the half that needs the ownership lens, so it runs after `resolve_owners`
/// and is kept separate rather than threaded through `stewardship()`: one
/// function stays testable without a portal, and the provenance of each half
/// stays legible (mesh-derived membership, registry-resolved people).
///
/// WHY IT MATTERS AND NOT JUST COSMETICALLY. The group's members are peers, so
/// the raw list answers "who do I ask" with hardware. A group's members resolve
/// to fewer people, because one person can hold two or three devices in the group
/// at once. A reader chasing an unowned machine at 3am wants a human, and a list
/// of laptop names is a worse answer than a shorter list of people — it looks like
/// more information and carries less.
///
/// Custody comes from the registry and the admitting key, never from reading a
/// machine's name, and a comment asserting custody is unverifiable by anything.
///
/// Devices that resolve to nobody are grouped into ONE `unresolved` principal
/// and never merged into a named one: "we could not name who holds these" is a
/// different fact from "this person holds them", and collapsing the two would
/// attribute machines to a person who may not hold them.
pub fn fold_admin_principals(
    st: &mut Stewardship,
    owners: &std::collections::BTreeMap<String, panoscope_core::peerowner::PeerOwner>,
) {
    use std::collections::BTreeMap;
    // Keyed by the resolved identity id when there is one, else the handle —
    // NOT the display name, which two people can share.
    let mut by_key: BTreeMap<String, AdminPrincipal> = BTreeMap::new();
    let mut unresolved: Vec<String> = Vec::new();
    let mut unresolved_online = 0usize;

    for a in &st.admins {
        let o = owners.get(&a.netbird_name);
        let key = o.and_then(|o| {
            o.ory_identity_id.clone().or_else(|| o.handle.clone())
        });
        match key {
            Some(k) => {
                let e = by_key.entry(k).or_insert_with(|| AdminPrincipal {
                    person_name: o.and_then(|o| o.person_name.clone()),
                    handle: o.and_then(|o| o.handle.clone()),
                    id_console_path: o.and_then(|o| o.id_console_path.clone()),
                    devices: Vec::new(),
                    devices_online: 0,
                    unresolved: false,
                });
                e.devices.push(a.netbird_name.clone());
                if a.connected {
                    e.devices_online += 1;
                }
            }
            None => {
                unresolved.push(a.netbird_name.clone());
                if a.connected {
                    unresolved_online += 1;
                }
            }
        }
    }

    let mut out: Vec<AdminPrincipal> = by_key.into_values().collect();
    // Named principals first, most devices first — the person most present in
    // the admin group is the most likely useful contact.
    out.sort_by_key(|p| std::cmp::Reverse(p.devices.len()));
    if !unresolved.is_empty() {
        out.push(AdminPrincipal {
            person_name: None,
            handle: None,
            id_console_path: None,
            devices_online: unresolved_online,
            devices: unresolved,
            unresolved: true,
        });
    }
    st.principals = out;

    // The additional scopes get the same treatment — a second scope's admins
    // are people for exactly the same reason the first scope's are.
    for extra in st.also_scopes.iter_mut() {
        let mut tmp = Stewardship {
            scope: Some(extra.scope.clone()),
            admin_group: extra.admin_group.clone(),
            admins: extra.admins.clone(),
            principals: Vec::new(),
            also_scopes: Vec::new(),
            scope_is_whole_goatnet: false,
            host_scope: extra.host_scope.clone(),
            basis: String::new(),
        };
        fold_admin_principals(&mut tmp, owners);
        extra.principals = tmp.principals;
    }
}

// ===========================================================================
// GEAR ONTOLOGY — a box's classes as ONE thing with facets, not N suffixes
// ===========================================================================
//
// ADR 1116 §2: a class is a FUNCTION, and a box holds one class per function it
// performs. §3 says every gear family answers the same five questions —
// element / data-plane / telemetry / command-intake / arbiter — and a class is
// the answer to one of them.
//
// netvis rendered none of that. A box holding SENSOR-FEED and SENSOR-TASKING
// showed two unrelated suffixes on an axis, with nothing saying they are the
// data-plane and command-intake facets of ONE sensor, and nothing saying the
// arbiter slot is empty. The structure existed only in prose and in the rules
// that happened to fall out of it.
//
// WHAT THIS IS NOT, and it is load-bearing (§4). NetBird groups cannot nest —
// `groups` has no `parent_id` — and because ACLs are allow-lists an extra
// membership only ever UNIONS reach: a subgroup can widen its parent, never
// narrow it. So this is a DISPLAY AND REASONING grouping over a FLAT group
// set. It must never read as an enforcement relationship, and no rule may be
// written against a family: the mesh does not know families exist. The struct
// carries that sentence to the client rather than trusting a renderer to
// remember it.
//
// The ontology is READ FROM THE DECLARATION (`doctrine.gear_ontology`, itself
// read from netbird_acl_taxonomy.yml). Rebuilding the table here would be a
// second implementation of it, which is precisely how the peer-name grammar
// ended up with five.

/// One slot of one family, as held by a machine.
#[derive(Debug, Clone, Serialize)]
pub struct GearFacet {
    /// `data-plane`, `telemetry`, `command-intake`, `arbiter`, `element`.
    pub slot: String,
    /// The group actually held, e.g. `SITE-A-SENSOR-FEED`.
    pub group: String,
    /// The class suffix, e.g. `SENSOR-FEED`.
    pub class: String,
}

/// A machine's membership in one gear family, decomposed into slots.
#[derive(Debug, Clone, Serialize)]
pub struct GearFamily {
    pub family: String,
    pub facets: Vec<GearFacet>,
    /// Slots of the five-slot pattern this machine does not hold.
    ///
    /// THIS IS NOT A GAP LIST. §2: a box holds one class per function it
    /// PERFORMS, so an absent slot means the box does not perform that
    /// function — which is the ordinary case, not a defect. An effector that
    /// takes no commands legitimately holds no command-intake class.
    ///
    /// It is reported because it completes the picture: reading
    /// "data-plane + arbiter, does not: telemetry, command-intake" tells you
    /// what the box IS in one line, where four unrelated suffixes told you
    /// only what it was tagged with. Renderers must phrase it as "does not",
    /// never "missing" — this tree has already learned once this week that a
    /// surface which cries about declared-and-empty trains people to ignore
    /// it (the exercise templates instantiate every group by design, and
    /// SENSOR/EFFECTOR are RESERVED AND EMPTY in the taxonomy itself).
    ///
    /// `element` is excluded for exactly that reason: it is reserved-and-empty
    /// by design, so listing it absent would manufacture noise out of the
    /// taxonomy working as intended.
    pub slots_absent: Vec<String>,
}

/// The five slots, in the order ADR 1116 §3 asks them.
const SLOT_ORDER: [&str; 5] = ["element", "data-plane", "telemetry", "command-intake", "arbiter"];

/// Decompose a machine's gear groups into families and slots.
///
/// Returns empty for a machine holding no gear class at all — most peers are
/// people's laptops and infra, and inventing a family for them would be noise.
pub fn gear_families(
    d: &Doc,
    peer_index: usize,
    ontology: &std::collections::BTreeMap<String, (String, String)>,
) -> Vec<GearFamily> {
    use std::collections::BTreeMap;
    let peers = match d.peers.as_ref() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let p = &peers[peer_index];

    let mut by_family: BTreeMap<String, Vec<GearFacet>> = BTreeMap::new();
    for g in &p.groups {
        let node = match d.nodes.get(*g) {
            Some(n) => n,
            None => continue,
        };
        // The class suffix is what the taxonomy keys on; the group carries the
        // scope prefix. Match on the suffix the graph already parsed rather
        // than re-splitting the name.
        if let Some((fam, slot)) = ontology.get(&node.suffix) {
            by_family.entry(fam.clone()).or_default().push(GearFacet {
                slot: slot.clone(),
                group: node.name.clone(),
                class: node.suffix.clone(),
            });
        }
    }

    let mut out: Vec<GearFamily> = by_family
        .into_iter()
        .map(|(family, mut facets)| {
            facets.sort_by_key(|f| {
                SLOT_ORDER.iter().position(|s| *s == f.slot).unwrap_or(usize::MAX)
            });
            let held: std::collections::BTreeSet<&str> =
                facets.iter().map(|f| f.slot.as_str()).collect();
            // Only the families the five-slot pattern actually decomposes.
            // CONTROL, FUSION, KID and friends are families of one; reporting
            // four "absent" slots for them would describe a decomposition that
            // does not apply to them.
            let decomposed = family == "SENSOR" || family == "EFFECTOR";
            let slots_absent = if decomposed {
                SLOT_ORDER
                    .iter()
                    .filter(|s| **s != "element" && !held.contains(**s))
                    .map(|s| s.to_string())
                    .collect()
            } else {
                Vec::new()
            };
            GearFamily { family, facets, slots_absent }
        })
        .collect();
    out.sort_by(|a, b| a.family.cmp(&b.family));
    out
}

#[cfg(test)]
mod stewardship_posture_tests {
    use super::*;
    use std::collections::BTreeMap;

    const FIXTURE: &str = include_str!("../tests/fixtures/reach-doc.json");

    /// The fixture plus an EMPTY `SITE-A-PLATFORM-ADMIN` — the shape every
    /// scope has the moment it is stamped, because the canonical group set is
    /// complete-by-prefix and an activity appoints a local admin only when it
    /// needs one.
    fn doc_with_unappointed_admin() -> Doc {
        let mut v: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture parses");
        let next = v["nodes"].as_array().expect("nodes is a list").len();
        // Cloned from a real node so every field the schema requires is
        // present; only the four `stewardship` reads are overwritten.
        let mut admin = v["nodes"][0].clone();
        admin["i"] = serde_json::json!(next);
        admin["name"] = serde_json::json!("SITE-A-PLATFORM-ADMIN");
        admin["family"] = serde_json::json!("SITE-A");
        admin["suffix"] = serde_json::json!("PLATFORM-ADMIN");
        v["nodes"].as_array_mut().unwrap().push(admin);
        serde_json::from_value(v).expect("doc rebuilds")
    }

    /// A HOSTED scope whose local admin is unappointed must send the reader to
    /// the host, and must not read as an accusation.
    ///
    /// This is a posture the operator corrected by hand: the surface used to
    /// call a declared-and-empty group "DECLARED BUT EMPTY … nobody is named",
    /// which made the design read as a defect. A surface that cries about
    /// declared-and-empty trains people to ignore it, and the same tree had
    /// already learned that lesson on `GearFamily::slots_absent`.
    #[test]
    fn an_unappointed_local_admin_names_its_host_and_does_not_accuse() {
        let d = doc_with_unappointed_admin();
        let hosts: BTreeMap<String, String> =
            [("SITE-A".to_string(), "HOSTNET".to_string())].into_iter().collect();
        let st = stewardship(&d, 0, &hosts);

        assert_eq!(st.scope.as_deref(), Some("SITE-A"));
        assert!(st.admins.is_empty(), "the fixture admin group has no members");
        assert_eq!(st.host_scope.as_deref(), Some("HOSTNET"));
        assert!(
            st.basis.contains("HOSTNET-PLATFORM-ADMIN"),
            "an unappointed local admin must name where authority sits; got: {}",
            st.basis
        );
        for accusation in ["DECLARED BUT EMPTY", "nobody is named", "missing"] {
            assert!(
                !st.basis.contains(accusation),
                "a hosted scope with no appointed local admin is the ordinary case, not a \
                 finding, and must not say {accusation:?}; got: {}",
                st.basis
            );
        }
    }

    /// The case that IS worth saying out loud survives the softening.
    ///
    /// A scope nothing hosts has nowhere to send a reader, so an empty
    /// PLATFORM-ADMIN there really does mean no one is named. Collapsing both
    /// cases into the gentle wording would hide the only version anyone needs
    /// to act on — which would be the same failure in the other direction.
    #[test]
    fn a_scope_nothing_hosts_still_reads_as_unanswerable() {
        let d = doc_with_unappointed_admin();
        let st = stewardship(&d, 0, &BTreeMap::new());
        assert_eq!(st.host_scope, None);
        assert!(
            st.basis.contains("nobody is named"),
            "a scope with no host and no admins is genuinely unanswerable; got: {}",
            st.basis
        );
    }

    /// A populated admin group is untouched by any of this.
    #[test]
    fn an_appointed_admin_group_still_names_the_group_and_adr_1124() {
        let mut v: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture parses");
        let next = v["nodes"].as_array().unwrap().len();
        let mut admin = v["nodes"][0].clone();
        admin["i"] = serde_json::json!(next);
        admin["name"] = serde_json::json!("SITE-A-PLATFORM-ADMIN");
        admin["family"] = serde_json::json!("SITE-A");
        admin["suffix"] = serde_json::json!("PLATFORM-ADMIN");
        v["nodes"].as_array_mut().unwrap().push(admin);
        // Peer 1 joins it, so the group is appointed.
        v["peers"][1]["groups"].as_array_mut().unwrap().push(serde_json::json!(next));
        let d: Doc = serde_json::from_value(v).expect("doc rebuilds");

        let hosts: BTreeMap<String, String> =
            [("SITE-A".to_string(), "HOSTNET".to_string())].into_iter().collect();
        let st = stewardship(&d, 0, &hosts);
        assert_eq!(st.admins.len(), 1, "one appointed admin");
        assert!(st.basis.contains("SITE-A-PLATFORM-ADMIN administers scope SITE-A"));
        assert!(st.basis.contains("1124"), "the basis cites what makes it authoritative");
    }
}
