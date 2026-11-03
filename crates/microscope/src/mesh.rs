//! Mesh forensics: take any identifier an operator has in front of them —
//! a peer name, a mesh IP, a NetBird account id — and answer two questions
//! the entity spine could not.
//!
//! **1. Whose is this?** The spine resolves email / handle / anchor / alias.
//! Those are all things you know once you already know who you are asking
//! about. What an operator actually has when something breaks is the string
//! on the screen: a peer name or a mesh IP. Neither
//! resolved to anything, and a mesh IP was silently taken for a handle — the
//! peer join then searched for a peer whose name contained that IP,
//! found none, and reported that no peer carries this entity's handle. A
//! report of a search that could never have succeeded.
//!
//! **2. Is this account changing configuration?** The spine's audit join reads
//! only `peer.*` and `setupkey.add`. Every rule-bearing code — `policy.add`,
//! `policy.update`, `policy.delete`, `group.*`, `route.*` — was outside it. So
//! the surface whose job is "what is this identity doing" could show a person
//! who had rewritten the goatnet's access policy nineteen times and say
//! nothing about it.
//!
//! ## What the joins are worth
//!
//! Every claim here is tagged with how it was made, because they are not
//! equally strong and a reader cannot tell by looking:
//!
//! - `recorded` — the field is on the object. `NbPeer.user_id` populated means
//!   NetBird stored that account at enrolment. This is the strong case and it
//!   only happens for an interactive (OIDC / browser) enrolment.
//! - `key-chain` — the peer carries no `user_id`, so ownership is chased
//!   through the audit log: `peer.setupkey.add` names the KEY, and the
//!   `setupkey.add` row that created that key names the account. Sound, but it
//!   attributes the peer to whoever minted the key, which for a shared or
//!   reusable key is not whoever is sitting at the machine.
//! - `none` — neither worked. Said out loud; never rendered as "unowned".
//!
//! ## Two things this deliberately will not do
//!
//! **It does not describe the peer.** A peer's groups say what it is
//! authorized to reach. They do not say what it is, where it is, or whose job
//! it does — repurposed machines keep their old groups, and reading a group
//! back as a description is how a stale label becomes a stated fact. Groups
//! appear here only inside a grant ("this policy admits SITE-A-CONTROL"),
//! never as a predicate about the machine.
//!
//! **It does not grade behaviour.** `AdminActions` counts and timestamps what
//! an account did to which named object. Whether nineteen policy writes in an
//! evening is an operator doing their job or someone taking the fence down is
//! not a thing an audit log knows, and a surface that guessed would be trusted
//! for the guess.

use std::collections::{BTreeMap, BTreeSet};

use panoscope_core::netbird::{NbEvent, NbPeer, NbPolicy, NbUser, NetbirdClient};
use serde::Serialize;

/// What the operator typed, once checked against live state.
///
/// Resolved by LOOKUP, not by shape. A NetBird account id and a Zenoh write
/// anchor are both dashed UUIDs, so shape alone cannot separate them — the
/// spine's `looks_like_anchor` claimed every account id as an anchor, which
/// turned "who is this admin" into "here is a write anchor nobody owns".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SelectorKind {
    /// Matched `NbPeer.ip` exactly.
    MeshIp,
    /// Matched `NbPeer.name` or `.hostname` exactly (case-insensitive).
    PeerName,
    /// Matched an id in `GET /api/users`.
    NetbirdAccount,
    /// A mesh identifier that matched no live object.
    NoMeshMatch,
}

/// One peer the selector resolved to, with its attribution.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PeerHit {
    pub name: String,
    pub id: String,
    pub ip: String,
    pub connected: bool,
    pub last_seen: String,
    pub os: String,
    /// True when NetBird marks the peer ephemeral — it disappears on its own.
    pub ephemeral: bool,
    /// The account this peer is attributed to, when one could be established.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// `recorded` | `key-chain` | `none` — see the module docs. Never omitted.
    pub attribution: String,
    /// For `key-chain`, the setup key that enrolled it, so the reader can see
    /// what the attribution actually rests on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via_setup_key: Option<String>,
    /// Groups held, carried ONLY so the policy join below can be read. Not a
    /// description of the peer — see the module docs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
}

/// A grant that admits traffic to this peer, and who last wrote it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Grant {
    pub policy_id: String,
    pub policy_name: String,
    pub enabled: bool,
    /// How the peer is named by the rule: `resource` (the rule targets this
    /// exact peer) or the group name that carried it in.
    pub via: String,
    /// Who may use it, rendered from the rule.
    pub sources: String,
    /// Protocol + ports, derived from the rule — never from the policy name.
    /// A policy called "SSH Access" whose rule permits every protocol on every
    /// port is exactly the case this exists to make visible.
    pub constraint: String,
    /// True when the rule limits neither protocol nor port.
    pub unrestricted: bool,
    /// Last audit row touching this policy: `<code> by <account> at <ts>`.
    /// Absent when the window held nothing — which is a statement about the
    /// window, not about the policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_write: Option<String>,
}

/// One bucket of configuration writes by a single account.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ActionBucket {
    /// `policy` | `group-membership` | `group` | `setup-key` | `account` |
    /// `network` | `peer-lifecycle` | or the raw code prefix when unrecognised.
    pub class: String,
    pub count: usize,
    pub first: String,
    pub last: String,
    /// Distinct activity codes seen, so a bucket never hides which verb ran.
    pub codes: Vec<String>,
    /// Named objects touched, from each event's `meta`. Capped, and the cap is
    /// reported rather than applied silently.
    pub targets: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub targets_truncated: Option<usize>,
}

/// Everything one account did to configuration, over whatever window the
/// activity log still holds.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AdminActions {
    pub account_id: String,
    /// `owner` / `admin` / `user`, or empty when the account list was unread.
    pub role: String,
    pub is_service_user: bool,
    /// Blank for every OIDC account — NetBird does not persist the claim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_name: Option<String>,
    pub last_login: String,
    /// Writes that change what the mesh permits, newest bucket first.
    pub buckets: Vec<ActionBucket>,
    /// Total across every bucket.
    pub total: usize,
    /// The most recent write, verbatim: `<ts> <code> <target>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_action: Option<String>,
    /// The oldest and newest rows the log served, so a zero count reads as
    /// "nothing in THIS window" rather than "nothing ever".
    pub window_first: String,
    pub window_last: String,
}

/// The whole answer for one selector.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MeshView {
    pub selector: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<SelectorKind>,
    /// Every peer the selector matched. A list, not an option: one name can
    /// cover many peers (a browser client mints a fresh ephemeral peer per
    /// session), and answering with the first would name one session out of
    /// eight and look definitive.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<PeerHit>,
    /// Accounts reached from those peers, or the selector itself when it named
    /// an account directly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<AdminActions>,
    /// Grants admitting traffic to the matched peers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<Grant>,
    /// What could not be established, and why. Never silently empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<String>,
}

/// Cap on named objects listed per bucket. Exceeding it is REPORTED
/// (`targets_truncated`), because a list that quietly stops reads as complete.
const TARGET_CAP: usize = 25;

/// Codes that change what the mesh permits, mapped to the class an operator
/// thinks in. Order matters: the first prefix that matches wins, so
/// `peer.group.` must be tested before the bare `peer.`.
fn classify(code: &str) -> Option<&'static str> {
    const TABLE: &[(&str, &str)] = &[
        ("policy.", "policy"),
        ("peer.group.", "group-membership"),
        ("setupkey.group.", "group-membership"),
        ("group.", "group"),
        ("setupkey.", "setup-key"),
        ("personal.access.token.", "account"),
        ("service.user.", "account"),
        ("user.peer.", "peer-lifecycle"),
        ("user.", "account"),
        ("account.", "account"),
        ("route.", "network"),
        ("nameserver.", "network"),
        ("dns.", "network"),
        ("network.", "network"),
        ("resource.", "network"),
        ("posture.", "policy"),
        ("integration.", "account"),
        ("peer.", "peer-lifecycle"),
    ];
    TABLE
        .iter()
        .find(|(p, _)| code.starts_with(p))
        .map(|(_, c)| *c)
}

/// Pull the human-readable object name out of an event's `meta`.
///
/// NetBird uses a different key per activity code. Trying them in order and
/// falling back to the target id means an unrecognised code still renders
/// something a reader can look up, instead of an empty row.
fn target_label(ev: &NbEvent) -> String {
    for k in ["name", "peer_fqdn", "group", "setupkey", "key", "username"] {
        if let Some(v) = ev.meta.get(k).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                // fqdns are long and all share a suffix; the short name is the
                // part an operator recognises.
                return v.split('.').next().unwrap_or(v).to_string();
            }
        }
    }
    if ev.target_id.is_empty() {
        "(unnamed)".to_string()
    } else {
        ev.target_id.clone()
    }
}

/// Roll one account's configuration writes up into buckets.
///
/// `peer-lifecycle` is counted but deliberately kept in its own class: adding
/// a peer is not the same kind of act as rewriting a policy, and merging them
/// would let enrolment volume bury a single `policy.delete`.
pub fn admin_actions(account: &NbUser, events: &[NbEvent]) -> AdminActions {
    let mut by_class: BTreeMap<&str, (usize, String, String, BTreeSet<String>, Vec<String>)> =
        BTreeMap::new();
    let mut latest: Option<&NbEvent> = None;

    for ev in events {
        if ev.initiator_id != account.id {
            continue;
        }
        let Some(class) = classify(&ev.activity_code) else {
            continue;
        };
        let e = by_class.entry(class).or_insert_with(|| {
            (
                0,
                ev.timestamp.clone(),
                ev.timestamp.clone(),
                BTreeSet::new(),
                Vec::new(),
            )
        });
        e.0 += 1;
        if ev.timestamp < e.1 {
            e.1 = ev.timestamp.clone();
        }
        if ev.timestamp > e.2 {
            e.2 = ev.timestamp.clone();
        }
        e.3.insert(ev.activity_code.clone());
        let label = target_label(ev);
        if !e.4.contains(&label) {
            e.4.push(label);
        }
        if latest.map(|l| ev.timestamp > l.timestamp).unwrap_or(true) {
            latest = Some(ev);
        }
    }

    let mut buckets: Vec<ActionBucket> = by_class
        .into_iter()
        .map(|(class, (count, first, last, codes, mut targets))| {
            let truncated = if targets.len() > TARGET_CAP {
                let n = targets.len() - TARGET_CAP;
                targets.truncate(TARGET_CAP);
                Some(n)
            } else {
                None
            };
            ActionBucket {
                class: class.to_string(),
                count,
                first,
                last,
                codes: codes.into_iter().collect(),
                targets,
                targets_truncated: truncated,
            }
        })
        .collect();
    // Most recently active class first — the question is always "what has this
    // account been doing lately", not "what has it done most of".
    buckets.sort_by(|a, b| b.last.cmp(&a.last));

    let total = buckets.iter().map(|b| b.count).sum();
    AdminActions {
        account_id: account.id.clone(),
        role: account.role.clone(),
        is_service_user: account.is_service_user,
        account_name: if account.name.is_empty() {
            None
        } else {
            Some(account.name.clone())
        },
        last_login: account.last_login.clone(),
        buckets,
        total,
        last_action: latest.map(|e| {
            format!(
                "{} {} {}",
                e.timestamp,
                e.activity_code,
                target_label(e)
            )
        }),
        window_first: events
            .iter()
            .map(|e| e.timestamp.as_str())
            .min()
            .unwrap_or_default()
            .to_string(),
        window_last: events
            .iter()
            .map(|e| e.timestamp.as_str())
            .max()
            .unwrap_or_default()
            .to_string(),
    }
}

/// Which policies admit traffic TO these peers, and who last wrote each.
///
/// Both shapes of rule endpoint are read. A rule can name a GROUP the peer
/// belongs to, or name the peer directly as a `destinationResource` — and the
/// second shape is invisible to any group-to-group view. A surface that draws
/// only the group graph shows nothing while an all-protocol grant to a named
/// host is live.
pub fn grants_for(
    peers: &[PeerHit],
    policies: &[NbPolicy],
    events: &[NbEvent],
    accounts: &[NbUser],
) -> Vec<Grant> {
    let peer_ids: BTreeSet<&str> = peers.iter().map(|p| p.id.as_str()).collect();
    let peer_groups: BTreeSet<&str> = peers
        .iter()
        .flat_map(|p| p.groups.iter().map(|g| g.as_str()))
        .collect();
    let account_role: BTreeMap<&str, String> = accounts
        .iter()
        .map(|u| {
            let label = if u.is_service_user {
                format!("{} (service)", if u.name.is_empty() { &u.id } else { &u.name })
            } else {
                format!("{} ({})", u.id, u.role)
            };
            (u.id.as_str(), label)
        })
        .collect();

    // Newest audit row per policy id. NetBird serves events newest-first but
    // that is not promised, so pick by timestamp rather than by position.
    let mut last_write: BTreeMap<&str, &NbEvent> = BTreeMap::new();
    for ev in events {
        if !ev.activity_code.starts_with("policy.") || ev.target_id.is_empty() {
            continue;
        }
        last_write
            .entry(ev.target_id.as_str())
            .and_modify(|cur| {
                if ev.timestamp > cur.timestamp {
                    *cur = ev;
                }
            })
            .or_insert(ev);
    }

    let mut out = Vec::new();
    for pol in policies {
        for rule in &pol.rules {
            let mut via: Option<String> = None;
            if let Some(res) = &rule.destination_resource {
                if peer_ids.contains(res.id.as_str()) {
                    via = Some("resource".to_string());
                }
            }
            if via.is_none() {
                if let Some(g) = rule
                    .destinations
                    .iter()
                    .find(|g| peer_groups.contains(g.name.as_str()))
                {
                    via = Some(g.name.clone());
                }
            }
            let Some(via) = via else { continue };

            let mut sources: Vec<String> =
                rule.sources.iter().map(|g| g.name.clone()).collect();
            if let Some(res) = &rule.source_resource {
                // A rule sourced from a single peer is scoped to THAT peer's
                // id. An ephemeral peer's id dies with it, which is why a
                // browser-session grant stops working without ever being
                // deleted — render the id so that is visible.
                sources.push(format!("{} {}", res.kind, res.id));
            }
            if sources.is_empty() {
                sources.push("(none stated)".to_string());
            }

            out.push(Grant {
                policy_id: pol.id.clone(),
                policy_name: pol.name.clone(),
                enabled: pol.enabled && rule.enabled,
                via,
                sources: sources.join(", "),
                constraint: rule.constraint(),
                unrestricted: rule.is_unrestricted(),
                last_write: last_write.get(pol.id.as_str()).map(|e| {
                    let who = account_role
                        .get(e.initiator_id.as_str())
                        .cloned()
                        .unwrap_or_else(|| e.initiator_id.clone());
                    format!("{} by {} at {}", e.activity_code, who, e.timestamp)
                }),
            });
        }
    }
    // Widest grants first: an unrestricted enabled accept is the row an
    // operator needs to see before any narrow one.
    out.sort_by_key(|g| (!(g.unrestricted && g.enabled), g.policy_name.clone()));
    out
}

/// Resolve one selector against live NetBird state.
///
/// Ordering is by strength, not by convenience: an exact account id, then an
/// exact mesh IP, then an exact peer name, then a name substring. The first
/// exact match wins outright — a substring pass that ran first would let
/// `100.127.0.8` claim `100.127.0.43`.
pub async fn resolve(nb: &NetbirdClient, selector: &str) -> MeshView {
    let sel = selector.trim();
    let mut view = MeshView {
        selector: sel.to_string(),
        ..Default::default()
    };
    if sel.is_empty() {
        return view;
    }
    if !nb.is_enabled() {
        view.unresolved
            .push("mesh join skipped: MICROSCOPE_NETBIRD_URL/TOKEN unset".into());
        return view;
    }

    let peers: Vec<NbPeer> = match nb.list_peers().await {
        Ok(p) => p,
        Err(e) => {
            view.unresolved.push(format!("netbird peers unread: {e}"));
            Vec::new()
        }
    };
    let users: Vec<NbUser> = match nb.list_users().await {
        Ok(u) => u,
        Err(e) => {
            view.unresolved.push(format!("netbird accounts unread: {e}"));
            Vec::new()
        }
    };
    let events: Vec<NbEvent> = match nb.list_events().await {
        Ok(e) => e,
        Err(e) => {
            view.unresolved.push(format!("netbird activity log unread: {e}"));
            Vec::new()
        }
    };

    let low = sel.to_lowercase();

    // --- classify by lookup ---------------------------------------------
    let account_hit = users.iter().find(|u| u.id.eq_ignore_ascii_case(sel));
    let ip_hits: Vec<&NbPeer> = peers.iter().filter(|p| p.ip == sel).collect();
    let exact_name: Vec<&NbPeer> = peers
        .iter()
        .filter(|p| {
            p.name.eq_ignore_ascii_case(sel) || p.hostname.eq_ignore_ascii_case(sel)
        })
        .collect();

    let matched: Vec<&NbPeer> = if account_hit.is_some() {
        view.kind = Some(SelectorKind::NetbirdAccount);
        // Every peer NetBird records as this account's. Deliberately not
        // widened with a name join: this branch answers "what does the account
        // own", and a name guess mixed in would make the two indistinguishable.
        peers.iter().filter(|p| p.user_id == account_hit.unwrap().id).collect()
    } else if !ip_hits.is_empty() {
        view.kind = Some(SelectorKind::MeshIp);
        view.unresolved.push(
            "a mesh IP is a lease, not an identity — it names whoever holds it right now, \
             and NetBird reassigns it when a peer is deleted and re-enrolled"
                .into(),
        );
        ip_hits
    } else if !exact_name.is_empty() {
        view.kind = Some(SelectorKind::PeerName);
        if exact_name.len() > 1 {
            view.unresolved.push(format!(
                "{} peers share this name — each is a separate enrolment with its own id \
                 and its own grants; they are not one machine",
                exact_name.len()
            ));
        }
        exact_name
    } else {
        let loose: Vec<&NbPeer> = peers
            .iter()
            .filter(|p| {
                p.name.to_lowercase().contains(&low) || p.hostname.to_lowercase().contains(&low)
            })
            .collect();
        if loose.is_empty() {
            view.kind = Some(SelectorKind::NoMeshMatch);
            view.unresolved.push(format!(
                "no peer, and no NetBird account, matches `{sel}` on this goatnet \
                 ({} peers, {} accounts read)",
                peers.len(),
                users.len()
            ));
            return view;
        }
        view.kind = Some(SelectorKind::PeerName);
        view.unresolved
            .push("matched on a name SUBSTRING — a lead, not an identification".into());
        loose
    };

    // --- attribution, strongest evidence first ---------------------------
    //
    // Pass 1 builds the key→account map once: `setupkey.add` names the account
    // that minted a key, and `peer.setupkey.add` names the key that enrolled a
    // peer. Chaining the two is the only route to a person for a peer that
    // carries no `user_id`, which is every setup-key enrolment.
    let key_owner: BTreeMap<&str, &str> = events
        .iter()
        .filter(|e| e.activity_code == "setupkey.add")
        .map(|e| (e.target_id.as_str(), e.initiator_id.as_str()))
        .collect();
    let mut enrolled_by: BTreeMap<&str, (&str, String)> = BTreeMap::new();
    for ev in &events {
        if ev.activity_code != "peer.setupkey.add" {
            continue;
        }
        if let Some(acct) = key_owner.get(ev.initiator_id.as_str()) {
            let key_name = ev
                .meta
                .get("setupkey")
                .and_then(|v| v.as_str())
                .unwrap_or(ev.initiator_id.as_str())
                .to_string();
            enrolled_by.insert(ev.target_id.as_str(), (acct, key_name));
        }
    }

    for p in &matched {
        let (account_id, attribution, via_key) = if !p.user_id.is_empty() {
            (Some(p.user_id.clone()), "recorded", None)
        } else if let Some((acct, key)) = enrolled_by.get(p.id.as_str()) {
            (Some((*acct).to_string()), "key-chain", Some(key.clone()))
        } else {
            (None, "none", None)
        };
        view.peers.push(PeerHit {
            name: p.name.clone(),
            id: p.id.clone(),
            ip: p.ip.clone(),
            connected: p.connected,
            last_seen: p.last_seen.clone(),
            os: p.os.trim().to_string(),
            ephemeral: p.ephemeral,
            account_id,
            attribution: attribution.to_string(),
            via_setup_key: via_key,
            groups: p.group_names().iter().map(|g| g.to_string()).collect(),
        });
    }
    if view.peers.iter().all(|p| p.attribution == "none") && !view.peers.is_empty() {
        view.unresolved.push(
            "no peer here carries an account: enrolled by a setup key whose creating row has \
             aged out of the activity log, or by a key since deleted. Absence of attribution \
             is not evidence the peer is unowned"
                .into(),
        );
    }

    // --- the accounts behind them ----------------------------------------
    let mut seen: BTreeSet<String> = BTreeSet::new();
    if let Some(u) = account_hit {
        seen.insert(u.id.clone());
        view.accounts.push(admin_actions(u, &events));
    }
    for hit in &view.peers {
        let Some(id) = &hit.account_id else { continue };
        if !seen.insert(id.clone()) {
            continue;
        }
        match users.iter().find(|u| &u.id == id) {
            Some(u) => view.accounts.push(admin_actions(u, &events)),
            None => view.unresolved.push(format!(
                "peer `{}` is attributed to account {id}, which is not in the account list \
                 — deleted since, or the token cannot see it",
                hit.name
            )),
        }
    }

    // --- what admits traffic to them --------------------------------------
    match nb.list_policies().await {
        Ok(policies) => {
            view.grants = grants_for(&view.peers, &policies, &events, &users);
            if view.grants.is_empty() {
                view.unresolved.push(
                    "no policy names these peers or their groups as a destination — under a \
                     default-deny cutover that means nothing reaches them"
                        .into(),
                );
            }
        }
        Err(e) => view.unresolved.push(format!("netbird policies unread: {e}")),
    }

    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(ts: &str, code: &str, init: &str, target: &str, meta: serde_json::Value) -> NbEvent {
        NbEvent {
            id: format!("{ts}-{code}"),
            timestamp: ts.into(),
            activity_code: code.into(),
            initiator_id: init.into(),
            target_id: target.into(),
            meta,
            ..Default::default()
        }
    }

    fn user(id: &str) -> NbUser {
        NbUser {
            id: id.into(),
            role: "admin".into(),
            ..Default::default()
        }
    }

    #[test]
    fn group_membership_is_not_filed_as_peer_lifecycle() {
        // `peer.group.add` starts with `peer.` and would fall into the
        // lifecycle bucket if the table were tested in the wrong order —
        // hiding every membership change behind enrolment volume.
        assert_eq!(classify("peer.group.add"), Some("group-membership"));
        assert_eq!(classify("peer.setupkey.add"), Some("peer-lifecycle"));
        assert_eq!(classify("setupkey.group.add"), Some("group-membership"));
        assert_eq!(classify("setupkey.add"), Some("setup-key"));
        assert_eq!(classify("group.delete"), Some("group"));
        assert_eq!(classify("user.peer.delete"), Some("peer-lifecycle"));
        assert_eq!(classify("user.invite"), Some("account"));
    }

    #[test]
    fn policy_writes_are_counted_and_named() {
        // The case the spine could not answer: an account that rewrote access
        // policy repeatedly showed nothing, because the audit join read only
        // `peer.*` codes.
        let u = user("acct-1");
        let events = vec![
            ev("2026-08-18T16:57:05Z", "policy.add", "acct-1", "p1", json!({"name": "SSH Access"})),
            ev("2026-08-18T17:21:14Z", "policy.add", "acct-1", "p2", json!({"name": "SSH end nodes"})),
            ev("2026-08-19T20:35:22Z", "policy.update", "acct-1", "p1", json!({"name": "SSH Access"})),
            ev("2026-08-19T09:33:43Z", "setupkey.add", "other", "k1", json!({"name": "not mine"})),
        ];
        let a = admin_actions(&u, &events);
        assert_eq!(a.total, 3, "only this account's writes");
        let pol = a.buckets.iter().find(|b| b.class == "policy").unwrap();
        assert_eq!(pol.count, 3);
        assert!(pol.targets.contains(&"SSH Access".to_string()));
        assert!(pol.targets.contains(&"SSH end nodes".to_string()));
        assert_eq!(a.last_action.unwrap(), "2026-08-19T20:35:22Z policy.update SSH Access");
        // A zero-write account must still show the window, so "nothing" reads
        // as "nothing in the log we have".
        assert_eq!(a.window_first, "2026-08-18T16:57:05Z");
        assert_eq!(a.window_last, "2026-08-19T20:35:22Z");
    }

    #[test]
    fn a_peer_targeted_by_resource_is_found() {
        // A rule whose destination is a single peer is invisible to a
        // group-to-group view; that shape carried a live all-protocol grant.
        let peers = vec![PeerHit {
            id: "peer-a".into(),
            name: "radar-ctl".into(),
            groups: vec!["ZAGROS-PROD-CONTROL".into()],
            ..Default::default()
        }];
        let pol = NbPolicy {
            id: "pol-1".into(),
            name: "SSH Access".into(),
            enabled: true,
            rules: vec![panoscope_core::netbird::NbPolicyRule {
                enabled: true,
                sources: vec![panoscope_core::netbird::NbGroup {
                    id: "g-all".into(),
                    name: "All".into(),
                    peers: vec![],
                }],
                destination_resource: Some(panoscope_core::netbird::NbResourceRef {
                    id: "peer-a".into(),
                    kind: "peer".into(),
                }),
                protocol: "all".into(),
                ..Default::default()
            }],
        };
        let events = vec![ev(
            "2026-08-19T20:18:52Z",
            "policy.update",
            "acct-1",
            "pol-1",
            json!({"name": "SSH Access"}),
        )];
        let g = grants_for(&peers, &[pol], &events, &[user("acct-1")]);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].via, "resource");
        assert!(g[0].unrestricted, "protocol=all, no ports");
        // The constraint must come from the RULE. The policy is named
        // "SSH Access" and permits every protocol on every port.
        assert_eq!(g[0].constraint, "all protocols · all ports");
        assert_eq!(
            g[0].last_write.as_deref(),
            Some("policy.update by acct-1 (admin) at 2026-08-19T20:18:52Z")
        );
    }

    #[test]
    fn an_ephemeral_browser_peer_reads_as_recorded_not_unowned() {
        // Each is a fresh ephemeral peer from one dashboard session. They carry `user_id`, so
        // attribution is RECORDED — the strong case — and must never render as
        // an unattributed peer just because the name looks like nobody's.
        let p = NbPeer {
            id: "da30q07439rs73buutag".into(),
            name: "chrome-151-browser-client".into(),
            ip: "100.127.0.77".into(),
            user_id: "5debdb57-f7c2-4a95-bc4b-0bc52750ab24".into(),
            ephemeral: true,
            ..Default::default()
        };
        assert!(!p.user_id.is_empty(), "recorded attribution, not a name guess");
        assert!(p.ephemeral, "the peer is disposable; its grants are not");
    }

    #[test]
    fn truncation_is_reported_not_silent() {
        let u = user("acct-1");
        let events: Vec<NbEvent> = (0..TARGET_CAP + 4)
            .map(|i| {
                ev(
                    &format!("2026-08-19T20:00:{i:02}Z"),
                    "policy.add",
                    "acct-1",
                    &format!("p{i}"),
                    json!({ "name": format!("policy-{i}") }),
                )
            })
            .collect();
        let a = admin_actions(&u, &events);
        let b = &a.buckets[0];
        assert_eq!(b.count, TARGET_CAP + 4, "the COUNT is never truncated");
        assert_eq!(b.targets.len(), TARGET_CAP);
        assert_eq!(b.targets_truncated, Some(4));
    }
}
