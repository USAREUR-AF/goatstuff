//! The entity spine: resolve one selector (email / handle / anchor UUID /
//! alias) into every identifier the deployment can see, tagging each join
//! machine-recorded vs convention (design §"The entity spine").
//!
//! Read-only, computed at query time (ADR 1065 doctrine). The operator
//! manifests are NOT a service-side source — they are gitignored operator
//! material; the service resolves from the portal registry, Kratos, and
//! NetBird, and says so per join.

use panoscope_core::netbird::NetbirdClient;
use panoscope_core::portal::PortalClient;
use serde::Serialize;

use crate::config::Config;

#[derive(Debug, Clone, Serialize, Default)]
pub struct EntitySpine {
    /// The selector as given.
    pub selector: String,
    /// Kebab handle, when resolvable (portal alias `assigned_to` / roster).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Ory Kratos identity id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ory_id: Option<String>,
    /// Ory groups claim, verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ory_groups: Vec<String>,
    /// Zenoh write-anchor UUID(s) (ADR 1025 tri-binding hub).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anchors: Vec<String>,
    /// Slot aliases (display-only, ADR 1039).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// NetBird peers whose name carries the handle prefix — CONVENTION join.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub netbird_peers: Vec<String>,
    /// Peers this entity is recorded as having ADMINISTERED, derived from the
    /// mesh audit stream rather than from a name.
    ///
    /// The convention join above matches `peer-<handle>-<env>-<date>-<host>`,
    /// which only holds for peers created by onboarding. An operator's own
    /// laptop, or any host enrolled directly with a setup key, is named
    /// nothing like that and was therefore invisible — while the SAME page
    /// showed hundreds of that person's mesh events. NetBird's per-peer
    /// `user_id` is empty fleet-wide (peers enrol by setup key, not by user
    /// login), so there is no ownership field to read.
    ///
    /// But `peer.*` audit events carry `initiator_id` — the Ory UUID, a
    /// strong key — plus the peer's id and fqdn. That maps person → peer
    /// without guessing at names.
    ///
    /// WHAT THIS IS NOT: ownership. It records that the entity ACTED ON the
    /// peer (added a group, enrolled it), which an administrator does to other
    /// people's machines all the time. It is rendered as "administered", never
    /// as "their peer"; ADR 1096 custody edges remain the source for custody.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub netbird_peers_administered: Vec<String>,
    /// Peers enrolled by a setup key THIS entity created — "the devices I
    /// brought onto the mesh".
    ///
    /// A different relation from `netbird_peers_administered` and it was wrong
    /// to render them as one list. Administering a peer means changing its
    /// groups, which an operator does to other people's machines constantly;
    /// enrolling one means it exists on the mesh because of a key you minted.
    /// The first list contained both and called all of it "administered".
    ///
    /// Derived by chaining two audit records, both keyed on ids rather than
    /// names: `setupkey.add` (initiator = the person, target = the key id)
    /// then `peer.setupkey.add` (initiator = that key id, target = the peer).
    ///
    /// Still not ownership: a key can enrol a machine that is then handed to
    /// someone else, and custody is ADR 1096's job. It is "this exists because
    /// of you", which is the honest reading of the record.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub netbird_peers_enrolled: Vec<String>,
    /// Mesh-side resolution: what the selector matched in NetBird, and what
    /// the account(s) behind it have been doing to configuration.
    ///
    /// Populated for ANY selector, not just mesh-shaped ones — an operator
    /// examining a person by email wants their config-write history on the
    /// same page as their anchors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mesh: Option<crate::mesh::MeshView>,
    /// Joins we could not make on this deployment, with why — rendered, not hidden.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<String>,
    /// Did Kratos actually ANSWER for this entity?
    ///
    /// An empty `ory_groups` has two completely different meanings — "we asked
    /// and this person holds no groups" and "we never asked" — and the page
    /// rendered both as `(none)`. On an identity-administration surface that
    /// is a false negative the tool generates itself: an admin reads "no
    /// groups" and concludes the person is unprivileged, when the truth may be
    /// that Kratos is unset or that
    /// the lookup transiently failed. Only a successful lookup sets this.
    #[serde(default)]
    pub ory_resolved: bool,
    /// Did the portal answer for anchors? Same distinction as `ory_resolved`:
    /// "no write anchors" and "the portal client is disabled" are not the same
    /// claim, and only one of them is about the person.
    #[serde(default)]
    pub anchors_resolved: bool,
}

impl EntitySpine {
    /// Every identifier string that adapters may match an actor field against.
    pub fn match_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = Vec::new();
        let mut push = |v: Option<&String>| {
            if let Some(v) = v {
                if !v.is_empty() && !keys.contains(v) {
                    keys.push(v.clone());
                }
            }
        };
        push(Some(&self.selector));
        push(self.handle.as_ref());
        push(self.email.as_ref());
        push(self.ory_id.as_ref());
        for a in &self.anchors {
            push(Some(a));
        }
        for a in &self.aliases {
            push(Some(a));
        }
        keys
    }
}

/// Resolve the spine. Portal first (registry = the declared universe), then
/// Kratos by email, then NetBird by handle prefix. Every miss is recorded in
/// `unresolved` rather than silently dropped.
pub async fn resolve(cfg: &Config, selector: &str) -> EntitySpine {
    let mut spine = EntitySpine {
        selector: selector.trim().to_string(),
        ..Default::default()
    };
    let sel_lower = spine.selector.to_lowercase();
    if sel_lower.contains('@') {
        spine.email = Some(sel_lower.clone());
    }

    // --- mesh resolution FIRST, because it types the selector ------------
    //
    // Runs before the portal and Kratos joins on purpose. Those joins branch on
    // what the selector IS — handle vs anchor vs email — and until now that was
    // decided by SHAPE, which cannot separate a NetBird account id from a write
    // anchor, and silently took a mesh IP for a handle. Asking NetBird what the
    // string actually matches turns those guesses into lookups.
    //
    // It is one extra round trip on every examine. Worth it: the alternative is
    // a confident wrong answer, and the peer/account/event lists it fetches are
    // the same ones the joins below need anyway.
    let nb_probe = NetbirdClient::new(cfg.netbird_url.clone(), cfg.netbird_token.clone());
    let mesh_view = if nb_probe.is_enabled() {
        Some(crate::mesh::resolve(&nb_probe, &spine.selector).await)
    } else {
        None
    };
    let selector_is_mesh_object = matches!(
        mesh_view.as_ref().and_then(|m| m.kind),
        Some(crate::mesh::SelectorKind::MeshIp) | Some(crate::mesh::SelectorKind::PeerName)
    );

    // --- portal: org rows (anchor prefixes) + slot aliases ---------------
    // Vendor.prefix == the write-anchor (UUID on slot-pool goatnets, tag on
    // per-vendor ones); AliasRow maps slot UUID ↔ human alias (ADR 1039).
    let portal = PortalClient::new(cfg.portal_url.clone(), cfg.portal_api_key.clone());
    match portal.list_vendors().await {
        Ok(rows) => {
            // The portal answered — an empty anchor list is now a statement
            // about this entity, not about a disabled portal client.
            spine.anchors_resolved = true;
            for v in rows {
                let prefix_hit = v.prefix.to_lowercase() == sel_lower;
                let name_hit = v.display_name.to_lowercase() == sel_lower;
                if (prefix_hit || name_hit)
                    && !spine.anchors.contains(&v.prefix) {
                        spine.anchors.push(v.prefix.clone());
                    }
            }
        }
        Err(e) => spine
            .unresolved
            .push(format!("portal orgs unavailable: {e}")),
    }

    // Ask the portal's RESOLVER, not just its org list.
    //
    // The loop above matches a vendor row when the selector EQUALS its prefix
    // or display name. That only ever works when you already know the anchor
    // or type the org's display name exactly — it can never answer for an
    // email, because an org row is not keyed by a person. So examining
    // someone by email showed "write anchors (none)" for a person who owned
    // an anchor, which is a confident wrong answer on the one surface whose
    // job is saying what a person may do.
    //
    // GET /api/access/who resolves an email / handle / peer name / slot id to
    // the person and their anchors, and reports `anchor_linkage` — whether
    // that attribution was RECORDED at onboard or derived by a name join. It
    // is the same resolver `goat who` and panoscope consume, so all three
    // agree by construction instead of each re-deriving ownership.
    //
    // Additive: anything the vendor loop already found is kept, and this only
    // fills what the name match could not. The portal being unreachable
    // leaves anchors_resolved as the vendor call set it — never a silent
    // "(none)".
    match portal.access_who(&spine.selector).await {
        Ok(who) => {
            spine.anchors_resolved = true;
            if !who.vendor.is_empty() && !spine.anchors.contains(&who.vendor) {
                spine.anchors.push(who.vendor.clone());
            }
            // A write entry's topic is `<anchor>/**`; take the anchor half.
            for w in &who.writes {
                if let Some((anchor, _)) = w.topic.split_once('/') {
                    if !anchor.is_empty() && !spine.anchors.contains(&anchor.to_string()) {
                        spine.anchors.push(anchor.to_string());
                    }
                }
            }
            // Record HOW ownership was attributed. A name-join answer must
            // never read like a recorded one — that indistinguishability is
            // the whole reason identity_id exists (ADR 1051).
            if !who.vendor.is_empty() || !who.writes.is_empty() {
                let linkage = if who.anchor_linkage.is_empty() {
                    "unstated".to_string()
                } else {
                    who.anchor_linkage.clone()
                };
                if linkage != "recorded" {
                    spine.unresolved.push(format!(
                        "anchor attribution is `{linkage}` — DERIVED from names, not the linkage                          recorded at onboard; org-mates sharing a tag share this anchor"
                    ));
                }
            }
            if !who.on_mesh && who.mesh_attribution == "none" {
                spine.unresolved.push(
                    "mesh presence unattributed — no peer name carries this person's handle;                      absence here is NOT evidence they hold no device"
                        .into(),
                );
            }
        }
        Err(e) => spine
            .unresolved
            .push(format!("portal access resolver unavailable: {e}")),
    }
    match portal.list_aliases().await {
        Ok(rows) => {
            for r in rows {
                let alias_hit = r.alias.to_lowercase() == sel_lower
                    || r.alias.to_lowercase().contains(&sel_lower);
                let slot_hit =
                    r.slot_id.to_lowercase() == sel_lower || spine.anchors.contains(&r.slot_id);
                if alias_hit || slot_hit {
                    if !spine.anchors.contains(&r.slot_id) {
                        spine.anchors.push(r.slot_id.clone());
                    }
                    if !spine.aliases.contains(&r.alias) {
                        spine.aliases.push(r.alias.clone());
                    }
                }
            }
        }
        Err(e) => spine
            .unresolved
            .push(format!("portal aliases unavailable: {e}")),
    }
    // Anchor-shaped selector stands on its own (ADR 1025: the anchor IS the
    // machine identity) — the portal join enriches, never gates.
    //
    // GATED ON THE MESH LOOKUP, because shape alone cannot tell a write anchor
    // from a NetBird account id: both are dashed UUIDs. Claiming every account
    // id as an anchor is not a cosmetic mislabel — it answers a question about
    // a PERSON with a fabricated machine identity, on the one surface whose job
    // is saying who someone is. `5debdb57-f7c2-4a95-bc4b-0bc52750ab24` read
    // that way while being an administrator's Ory id.
    let selector_is_account = mesh_view
        .as_ref()
        .map(|m| m.kind == Some(crate::mesh::SelectorKind::NetbirdAccount))
        .unwrap_or(false);
    if looks_like_anchor(&sel_lower)
        && !selector_is_account
        && !spine.anchors.contains(&sel_lower)
    {
        spine.anchors.push(sel_lower.clone());
    }
    if selector_is_account {
        spine
            .unresolved
            .push("selector is a NetBird/Ory account id, not a Zenoh write anchor — \
                   the two share a UUID shape and only a lookup separates them".into());
    }
    if spine.handle.is_none()
        && !sel_lower.contains('@')
        && !looks_like_anchor(&sel_lower)
        && !selector_is_mesh_object
    {
        // Selector is handle-shaped even if the portal didn't know it.
        //
        // `selector_is_mesh_object` keeps a peer name or a mesh IP out of here.
        // Without it a mesh IP became the handle, and the convention join below
        // then searched for a peer whose NAME contains the IP. It
        // found none and reported that no peer carries this entity's handle —
        // a report of a search that could never have succeeded, for a selector
        // that names a peer exactly.
        spine.handle = Some(sel_lower.clone());
    }
    if spine.handle.is_none() {
        // EMAIL LOCAL PART as a last-resort handle candidate.
        //
        // Without this the peer join never RAN for an email selector: the
        // whole convention block is gated on a resolved handle, so a person
        // the portal has no handle for got "no peer name carries this
        // entity's handle prefix" — a report of a search that never happened.
        // `someone@example.org` read that way while
        // `peer-someone-galaxy-2030-01-05-host` sat in the peer list.
        //
        // The local part IS the handle the onboarding convention uses when it
        // mints `peer-<handle>-<env>-<date>-<host>`, so this is reading the
        // same rule the names were written by, not a new guess. It stays
        // Convention-graded like every other name match, and `+tag` suffixes
        // are stripped because they never appear in a minted peer name.
        let candidate = spine
            .email
            .as_deref()
            .or(Some(sel_lower.as_str()))
            .filter(|s| s.contains('@'))
            .map(|e| e.split('@').next().unwrap_or("").to_string())
            .map(|l| l.split('+').next().unwrap_or("").to_string())
            .filter(|l| !l.is_empty());
        if let Some(c) = candidate {
            spine.handle = Some(c);
        }
    }

    // --- Kratos: identity by email --------------------------------------
    if cfg.kratos_admin_url.is_empty() {
        spine
            .unresolved
            .push("kratos join skipped: MICROSCOPE_KRATOS_ADMIN_URL unset".into());
    } else if let Some(email) = spine.email.clone() {
        match kratos_identity_by_email(cfg, &email).await {
            Ok(Some((id, groups))) => {
                spine.ory_id = Some(id);
                spine.ory_groups = groups;
                // Kratos answered. Only here is an empty group list a fact
                // ABOUT THIS PERSON rather than a fact about the deployment.
                spine.ory_resolved = true;
            }
            Ok(None) => spine
                .unresolved
                .push(format!("no Ory identity for {email}")),
            Err(e) => spine.unresolved.push(format!("kratos lookup failed: {e}")),
        }
    } else {
        spine
            .unresolved
            .push("kratos join needs an email selector (handle→email map lives in operator manifests, not service-side)".into());
    }

    // --- NetBird peers by handle prefix (convention join) ----------------
    //
    // ONE peer list per request, shared by both joins below. They were
    // fetching it separately, which cost a second round-trip for a list that
    // cannot change between two calls in the same request.
    let nb = NetbirdClient::new(cfg.netbird_url.clone(), cfg.netbird_token.clone());
    let nb_peers: Option<Vec<panoscope_core::netbird::NbPeer>> = if nb.is_enabled() {
        match nb.list_peers().await {
            Ok(p) => Some(p),
            Err(e) => {
                spine.unresolved.push(format!("netbird peers failed: {e}"));
                None
            }
        }
    } else {
        None
    };
    if !nb.is_enabled() {
        spine
            .unresolved
            .push("netbird join skipped: MICROSCOPE_NETBIRD_URL/TOKEN unset".into());
    } else if let Some(handle) = spine.handle.clone() {
        match nb_peers.clone().ok_or("peers unread") {
            Ok(peers) => {
                // AUTHORITATIVE PASS FIRST: ask the portal's peer resolver
                // (peer-identity-resolution.md Option B, live since
                // ) which peers belong to this person, instead of
                // guessing from name shape. It parses the handle out of the
                // deterministic onboard-minted name and looks it up in the
                // registry — the same resolution `goat who`, panoscope and the
                // relay-health cards already use, so all four agree rather
                // than each re-deriving it.
                let all_names: Vec<String> = peers.iter().map(|p| p.name.clone()).collect();
                match portal.access_peers(&all_names).await {
                    Ok(resolved) => {
                        for (peer_name, info) in &resolved {
                            let h = info
                                .get("handle")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default();
                            let owns_id = info
                                .get("deep_links")
                                .and_then(|d| d.get("id_console_user"))
                                .and_then(|v| v.as_str())
                                .map(|s| {
                                    spine
                                        .ory_id
                                        .as_deref()
                                        .map(|id| s.contains(id))
                                        .unwrap_or(false)
                                })
                                .unwrap_or(false);
                            if ((!h.is_empty() && h.eq_ignore_ascii_case(&handle)) || owns_id)
                                && !spine.netbird_peers.contains(peer_name) {
                                    spine.netbird_peers.push(peer_name.clone());
                                }
                        }
                    }
                    Err(e) => spine
                        .unresolved
                        .push(format!("portal peer resolver unavailable: {e}")),
                }
                for p in peers {
                    let name = p.name.to_lowercase();
                    // The keyed leg goes through the generated grammar
                    // (ops/identity/peer-name-grammar.yaml), which anchors the
                    // handle on the field separator — `alex` no longer claims a
                    // `peer-alexander-…` peer. The loose `contains` leg stays:
                    // this is a diagnostic spine that wants recall, and a name
                    // it surfaces is a lead, never a cut selector.
                    if panoscope_core::peername::match_longest_key(&name, &[handle.as_str()])
                        .is_some()
                        || name.contains(&handle)
                    {
                        spine.netbird_peers.push(p.name);
                    }
                }
            }
            Err(_) => { /* already recorded above */ }
        }
    }

    // --- derived peer join, from the mesh audit --------------------------
    //
    // Runs whenever we have an Ory id, INDEPENDENT of whether the name join
    // found anything: the two are different evidence, and the weaker one
    // succeeding must not suppress the stronger one.
    if let Some(oid) = spine.ory_id.clone() {
        if !cfg.netbird_url.is_empty() && !cfg.netbird_token.is_empty() {
            let url = format!("{}/api/events", cfg.netbird_url.trim_end_matches('/'));
            let fetched = reqwest::Client::new()
                .get(&url)
                .header("Authorization", format!("Token {}", cfg.netbird_token))
                .send()
                .await;
            match fetched {
                Ok(r) => match r.json::<Vec<serde_json::Value>>().await {
                    Ok(events) => {
                        let peers = nb_peers.clone().unwrap_or_default();
                        let by_id: std::collections::HashMap<String, String> = peers
                            .iter()
                            .map(|p| (p.id.clone(), p.name.clone()))
                            .collect();
                        // Pass 1: setup keys this entity created. The key id
                        // is the JOIN KEY for enrolment below — NetBird records
                        // the enrolling KEY as the initiator of the peer event,
                        // not the person, so without this hop a person's own
                        // machines are invisible.
                        let own_keys: std::collections::HashSet<&str> = events
                            .iter()
                            .filter(|e| {
                                e["activity_code"].as_str() == Some("setupkey.add")
                                    && e["initiator_id"].as_str() == Some(oid.as_str())
                            })
                            .filter_map(|e| e["target_id"].as_str())
                            .collect();

                        for ev in &events {
                            let init = ev["initiator_id"].as_str().unwrap_or_default();
                            let code = ev["activity_code"].as_str().unwrap_or_default();
                            // Enrolment: initiated BY one of this entity's keys.
                            if code == "peer.setupkey.add" && own_keys.contains(init) {
                                let target = ev["target_id"].as_str().unwrap_or_default();
                                let n = by_id.get(target).cloned().or_else(|| {
                                    ev["meta"]["fqdn"].as_str().map(|f| {
                                        f.split('.').next().unwrap_or(f).to_string()
                                    })
                                });
                                if let Some(n) = n {
                                    if !n.is_empty() && !spine.netbird_peers_enrolled.contains(&n) {
                                        spine.netbird_peers_enrolled.push(n);
                                    }
                                }
                                continue;
                            }
                            if init != oid || !code.starts_with("peer.") {
                                continue;
                            }
                            // Prefer the live peer name; fall back to the fqdn
                            // the event recorded, so a peer since deleted still
                            // renders as something a reader can recognise.
                            let target = ev["target_id"].as_str().unwrap_or_default();
                            let name = by_id.get(target).cloned().or_else(|| {
                                ev["meta"]["peer_fqdn"].as_str().map(|f| {
                                    f.split('.').next().unwrap_or(f).to_string()
                                })
                            });
                            if let Some(n) = name {
                                if !n.is_empty()
                                    && !spine.netbird_peers.contains(&n)
                                    && !spine.netbird_peers_administered.contains(&n)
                                {
                                    spine.netbird_peers_administered.push(n);
                                }
                            }
                        }
                        spine.netbird_peers_administered.sort();
                    spine.netbird_peers_enrolled.sort();
                    // A peer you enrolled AND later re-grouped is an enrolment
                    // first; listing it twice would overstate both.
                    spine
                        .netbird_peers_administered
                        .retain(|p| !spine.netbird_peers_enrolled.contains(p));
                    }
                    Err(e) => spine
                        .unresolved
                        .push(format!("netbird peer-administration join failed: {e}")),
                },
                Err(e) => spine
                    .unresolved
                    .push(format!("netbird peer-administration join failed: {e}")),
            }
        }
    }

    // Carry the mesh view through, and lift its unresolved notes onto the
    // spine's own list so one page has one place that says what could not be
    // established.
    if let Some(m) = mesh_view {
        for u in &m.unresolved {
            if !spine.unresolved.contains(u) {
                spine.unresolved.push(u.clone());
            }
        }
        spine.mesh = Some(m);
    }

    spine
}

/// Anchor-shaped: dashed UUID (36) or the slot-pool bare 32-hex form.
fn looks_like_anchor(s: &str) -> bool {
    (s.len() == 36 && s.chars().filter(|c| *c == '-').count() == 4)
        || (s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit()))
}

async fn kratos_identity_by_email(
    cfg: &Config,
    email: &str,
) -> anyhow::Result<Option<(String, Vec<String>)>> {
    let url = format!(
        "{}/admin/identities?credentials_identifier={}",
        cfg.kratos_admin_url.trim_end_matches('/'),
        urlencode(email)
    );
    let ids: Vec<serde_json::Value> = reqwest::Client::new().get(&url).send().await?.json().await?;
    for id in ids {
        let traits_email = id["traits"]["email"].as_str().unwrap_or_default();
        if traits_email.eq_ignore_ascii_case(email) {
            return Ok(Some((
                id["id"].as_str().unwrap_or_default().to_string(),
                login_groups(&id),
            )));
        }
    }
    Ok(None)
}

/// The login groups of one Kratos admin-API identity. They live in
/// admin-managed `metadata_public`; a `groups` key there,
/// whatever it holds, marks the identity migrated, and its self-editable
/// `traits.groups` is never read. Only an identity the backfill has not
/// reached falls back to traits.
fn login_groups(id: &serde_json::Value) -> Vec<String> {
    let migrated = id["metadata_public"]
        .as_object()
        .is_some_and(|m| m.contains_key("groups"));
    let source = if migrated {
        &id["metadata_public"]["groups"]
    } else {
        &id["traits"]["groups"]
    };
    source
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|g| g.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_groups_never_reads_traits_once_migrated() {
        let self_grant = serde_json::json!({"groups": ["PLATFORM-ADMIN"]});
        let case = |meta: serde_json::Value| {
            login_groups(&serde_json::json!({"traits": self_grant, "metadata_public": meta}))
        };
        let iam = serde_json::json!({"groups": ["IAM-ADMIN"]});
        assert_eq!(case(iam), vec!["IAM-ADMIN"]);
        assert!(case(serde_json::json!({"groups": []})).is_empty());
        assert!(case(serde_json::json!({"groups": null})).is_empty());
        // Not migrated (no groups key, or no metadata_public): traits, the backfill window.
        assert_eq!(case(serde_json::json!({"org": {}})), vec!["PLATFORM-ADMIN"]);
        assert_eq!(case(serde_json::Value::Null), vec!["PLATFORM-ADMIN"]);
    }

    #[test]
    fn match_keys_dedupes_and_skips_empties() {
        let spine = EntitySpine {
            selector: "pat.example@example.org".into(),
            handle: Some("pat-example".into()),
            email: Some("pat.example@example.org".into()), // dup of selector
            anchors: vec!["abc-123".into()],
            ..Default::default()
        };
        let keys = spine.match_keys();
        assert_eq!(
            keys,
            vec!["pat.example@example.org", "pat-example", "abc-123"]
        );
    }

    #[test]
    fn anchor_shape_detection() {
        assert!(looks_like_anchor("00000000-0000-4000-8000-00000000cacf"));
        // THE TRAP, pinned. This IS a NetBird/Ory account id — an
        // administrator's — and shape alone calls it an anchor, because a
        // write anchor and an account id are the same 36-char dashed UUID.
        // Nothing here is wrong; the lesson is that this predicate CANNOT be
        // the thing that decides, which is why `resolve` gates its use on the
        // mesh lookup instead.
        assert!(looks_like_anchor("00000000-0000-4000-8000-00000000ab24"));
        // A mesh IP is not anchor-shaped, so before the mesh lookup existed it
        // fell through to the handle branch and the peer join then searched
        // for a peer NAME containing it. That search cannot succeed.
        assert!(!looks_like_anchor("100.127.24.133"));
        assert!(!looks_like_anchor("chrome-151-browser-client"));
        assert!(looks_like_anchor("00000000000040008000000000000eca"));
        assert!(!looks_like_anchor("pat-example"));
        assert!(!looks_like_anchor("pat.example@example.org"));
    }

    #[test]
    fn urlencode_escapes_at_sign() {
        assert_eq!(urlencode("a@b.c"), "a%40b.c");
        assert_eq!(urlencode("pat-example"), "pat-example");
    }
}

#[cfg(test)]
mod handle_fallback_tests {
    /// The rule the peer names were MINTED by: `peer-<handle>-<env>-…`, where
    /// handle is the email local part. Reading it back is not a new guess.
    fn local_part(email: &str) -> Option<String> {
        Some(email)
            .filter(|s| s.contains('@'))
            .map(|e| e.split('@').next().unwrap_or("").to_string())
            .map(|l| l.split('+').next().unwrap_or("").to_string())
            .filter(|l| !l.is_empty())
    }

    #[test]
    fn an_email_selector_yields_the_handle_the_peer_name_uses() {
        assert_eq!(local_part("someone@example.org").as_deref(), Some("someone"));
        // matches peer-someone-galaxy-2030-01-05-host
    }

    #[test]
    fn plus_tags_are_stripped_because_minted_names_never_carry_them() {
        assert_eq!(local_part("someone+zagros@example.test").as_deref(), Some("someone"));
    }

    #[test]
    fn a_non_email_yields_nothing_rather_than_a_bogus_handle() {
        assert_eq!(local_part("not-an-email"), None);
        assert_eq!(local_part("@nolocal.test"), None);
    }
}
