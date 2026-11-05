//! Typeahead candidates for the search box: the set of selectors this
//! deployment can actually examine, matched on a substring ANYWHERE in the
//! string — start, middle, or end.
//!
//! WHY A SERVER-SIDE CACHE AND NOT A PER-KEYSTROKE QUERY. The candidate
//! universe is one portal call (`GET /api/access/roster`) plus two cheap
//! ones, and the roster call measured **2.2 s** against zagros's portal on
//! (387 rows, 105 KB). That is fine once a minute and absurd once
//! a keystroke, so the snapshot is built behind a mutex — which also makes it
//! single-flight: ten keystrokes arriving during a refresh wait for that one
//! refresh instead of starting ten.
//!
//! HONESTY (the rule this whole surface exists for). An empty match list is
//! not the same claim as "the roster could not be read", and the UI must
//! never render the second as the first — an admin reading "no matches" for a
//! person who IS on the roster is exactly the false negative microscope was
//! built to stop making. Every source that fails leaves a note on the
//! snapshot, the response carries `roster_read`, and a stale-but-served
//! snapshot says how old it is.
//!
//! Suggestions are LEADS, never authority: each one is a selector the entity
//! page then resolves properly through [`crate::spine`]. Nothing here decides
//! anything about a person.

use std::sync::Arc;
use std::time::{Duration, Instant};

use panoscope_core::netbird::NetbirdClient;
use panoscope_core::portal::PortalClient;
use serde::Serialize;

use crate::config::Config;

/// One thing a reader can examine.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Candidate {
    /// What goes into `/entity?id=` — always a selector the spine accepts.
    pub selector: String,
    /// What the reader sees.
    pub label: String,
    /// `person` | `org` | `alias` | `peer`.
    pub kind: String,
    /// Secondary line: the org a person belongs to, a slot's alias, a peer's
    /// mesh IP. Context for telling two similar rows apart, not a claim.
    pub detail: String,
    /// Extra strings that MATCH but are not displayed — an Ory UUID, an
    /// anchor, a mesh IP. An operator who pastes one of those gets the row,
    /// without the list turning into a wall of UUIDs.
    #[serde(skip)]
    pub keys: Vec<String>,
}

impl Candidate {
    fn kind_rank(&self) -> u8 {
        match self.kind.as_str() {
            "person" => 0,
            "org" => 1,
            "alias" => 2,
            _ => 3,
        }
    }
}

/// The candidate universe as of one read, plus what could not be read.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub entries: Vec<Candidate>,
    /// Notes from sources that failed — rendered, never swallowed.
    pub notes: Vec<String>,
    /// Did the portal roster (the person/org universe) answer? `false` means
    /// "we do not know who exists", which is not "nobody matched".
    pub roster_read: bool,
    /// When this snapshot was built. `None` = never successfully built.
    pub built_at: Option<Instant>,
}

/// TTL-cached snapshot with single-flight refresh.
pub struct Cache {
    inner: tokio::sync::Mutex<Arc<Snapshot>>,
    ttl: Duration,
}

impl Cache {
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            inner: tokio::sync::Mutex::new(Arc::new(Snapshot::default())),
            ttl: Duration::from_secs(ttl_secs.max(5)),
        }
    }

    /// Fresh snapshot, rebuilding if the TTL has expired.
    ///
    /// A failed rebuild KEEPS the previous snapshot and appends the failure as
    /// a note. Serving a stale list with its age stated beats serving an empty
    /// one that reads like "nobody here".
    pub async fn get(&self, cfg: &Config) -> Arc<Snapshot> {
        let mut cur = self.inner.lock().await;
        let fresh = cur
            .built_at
            .map(|t| t.elapsed() < self.ttl)
            .unwrap_or(false);
        if fresh {
            return cur.clone();
        }
        let (entries, notes, roster_read) = build(cfg).await;
        if !roster_read && cur.built_at.is_some() {
            // Keep the last good list; say how old it is and why.
            let mut kept = Snapshot {
                entries: cur.entries.clone(),
                notes: cur.notes.clone(),
                roster_read: cur.roster_read,
                built_at: cur.built_at,
            };
            let age = kept.built_at.map(|t| t.elapsed().as_secs()).unwrap_or(0);
            kept.notes.extend(notes);
            kept.notes.push(format!(
                "serving a snapshot {age}s old — the refresh above failed; a name minted since then is missing from these suggestions"
            ));
            let snap = Arc::new(kept);
            *cur = snap.clone();
            return snap;
        }
        let snap = Arc::new(Snapshot {
            entries,
            notes,
            roster_read,
            built_at: Some(Instant::now()),
        });
        *cur = snap.clone();
        snap
    }
}

/// Read every candidate source. Each failure becomes a note; none is fatal.
async fn build(cfg: &Config) -> (Vec<Candidate>, Vec<String>, bool) {
    let mut out: Vec<Candidate> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut roster_read = false;

    let portal = PortalClient::new(cfg.portal_url.clone(), cfg.portal_api_key.clone());

    // --- slot aliases first: they name the orgs the roster then lists ------
    let mut alias_of: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    match portal.list_aliases().await {
        Ok(rows) => {
            for r in rows {
                alias_of
                    .entry(r.slot_id.clone())
                    .or_insert_with(|| r.alias.clone());
                out.push(Candidate {
                    selector: r.alias.clone(),
                    label: r.alias,
                    kind: "alias".into(),
                    detail: short(&r.slot_id),
                    keys: vec![r.slot_id],
                });
            }
        }
        Err(e) => notes.push(format!("slot aliases unread: {e}")),
    }

    // --- the roster: every person + org the portal knows ------------------
    match portal.access_roster().await {
        Ok(roster) => {
            roster_read = true;
            // org label per anchor, so a person's row can say which org they
            // belong to instead of showing a 32-hex string.
            let mut org_label: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            for r in &roster.rows {
                if r.kind == "vendor" && !r.label.is_empty() {
                    org_label.insert(r.id.clone(), r.label.clone());
                }
            }
            for r in roster.rows {
                match r.kind.as_str() {
                    "person" => {
                        // The EMAIL is the selector: it is what resolves the
                        // Kratos join, and the roster's `id` is an Ory UUID no
                        // reader types. When a row carries no email, fall back
                        // to the id rather than dropping the person.
                        let selector = if r.label.contains('@') {
                            r.label.clone()
                        } else {
                            r.id.clone()
                        };
                        // A person row's `vendor` is the org TAG on tag
                        // goatnets (`usareur-af`) and an anchor on slot-pool
                        // ones — 169 of zagros's 228 people carry one. Look it
                        // up as an anchor first, then fall back to printing
                        // the tag itself rather than dropping the only org
                        // context the row has.
                        let org = org_label
                            .get(&r.vendor)
                            .cloned()
                            .or_else(|| alias_of.get(&r.vendor).cloned())
                            .unwrap_or_else(|| r.vendor.clone());
                        // The kind is already a badge in the UI; the detail
                        // line carries only what the badge cannot say.
                        let detail = org;
                        out.push(Candidate {
                            selector,
                            label: r.label.clone(),
                            kind: "person".into(),
                            detail,
                            keys: vec![r.id, r.write_anchor, r.vendor]
                                .into_iter()
                                .filter(|k| !k.is_empty())
                                .collect(),
                        });
                    }
                    "vendor" => {
                        let alias = alias_of.get(&r.id).cloned().unwrap_or_default();
                        let detail = if alias.is_empty() {
                            short(&r.id)
                        } else {
                            alias
                        };
                        out.push(Candidate {
                            selector: r.id.clone(),
                            label: if r.label.is_empty() {
                                r.id.clone()
                            } else {
                                r.label.clone()
                            },
                            kind: "org".into(),
                            detail,
                            keys: vec![r.id],
                        });
                    }
                    _ => {}
                }
            }
        }
        Err(e) => notes.push(format!(
            "portal roster unread: {e} — the person/org universe is UNKNOWN, not empty"
        )),
    }

    // --- mesh peers -------------------------------------------------------
    let nb = NetbirdClient::new(cfg.netbird_url.clone(), cfg.netbird_token.clone());
    if nb.is_enabled() {
        match nb.list_peers().await {
            Ok(peers) => {
                for p in peers {
                    if p.name.is_empty() {
                        continue;
                    }
                    let detail = p.ip.clone();
                    out.push(Candidate {
                        selector: p.name.clone(),
                        label: p.name,
                        kind: "peer".into(),
                        detail,
                        keys: vec![p.ip, p.hostname]
                            .into_iter()
                            .filter(|k| !k.is_empty())
                            .collect(),
                    });
                }
            }
            Err(e) => notes.push(format!("mesh peers unread: {e}")),
        }
    } else {
        notes.push("mesh peers skipped: MICROSCOPE_NETBIRD_URL/TOKEN unset".into());
    }

    (out, notes, roster_read)
}

/// First 12 chars of an anchor — enough to recognise, short enough to read.
fn short(s: &str) -> String {
    if s.len() <= 12 {
        s.to_string()
    } else {
        format!("{}…", &s[..12])
    }
}

/// Where `q` sits in `hay`, as a rank: 0 exact, 1 prefix, 2 at a word
/// boundary (after `-._@+/ ` etc.), 3 anywhere else. `None` = no match.
///
/// The operator asked for start / middle / end, which is plain `contains` —
/// the ranks only decide ORDER, never membership. A middle-of-word hit still
/// appears; it just sorts below a prefix hit.
fn score(hay: &str, q: &str) -> Option<u8> {
    let h = hay.to_lowercase();
    if h == q {
        return Some(0);
    }
    match h.find(q) {
        None => None,
        Some(0) => Some(1),
        Some(i) => {
            let boundary = h[..i]
                .chars()
                .next_back()
                .map(|c| !c.is_alphanumeric())
                .unwrap_or(true);
            Some(if boundary { 2 } else { 3 })
        }
    }
}

/// Best `limit` candidates for `q`, best first. Case-insensitive; matches
/// anywhere in the selector, the label, or a hidden key (UUID / anchor / IP).
pub fn rank(entries: &[Candidate], q: &str, limit: usize) -> Vec<Candidate> {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(u8, u8, usize, &Candidate)> = Vec::new();
    for c in entries {
        let mut best: Option<u8> = None;
        for hay in [c.selector.as_str(), c.label.as_str()] {
            if let Some(s) = score(hay, &q) {
                best = Some(best.map_or(s, |b: u8| b.min(s)));
            }
        }
        // A hidden key match is real but never outranks a visible one: the
        // reader cannot see WHY that row matched, so it sorts last.
        if best.is_none() && c.keys.iter().any(|k| score(k, &q).is_some()) {
            best = Some(4);
        }
        if let Some(s) = best {
            hits.push((s, c.kind_rank(), c.label.len(), c));
        }
    }
    hits.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
            .then(a.3.label.to_lowercase().cmp(&b.3.label.to_lowercase()))
    });
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (_, _, _, c) in hits {
        if !seen.insert(c.selector.to_lowercase()) {
            continue;
        }
        out.push(c.clone());
        if out.len() >= limit {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(selector: &str, label: &str, kind: &str) -> Candidate {
        Candidate {
            selector: selector.into(),
            label: label.into(),
            kind: kind.into(),
            detail: String::new(),
            keys: Vec::new(),
        }
    }

    fn universe() -> Vec<Candidate> {
        vec![
            c("sam.sample@example.org", "sam.sample@example.org", "person"),
            c("alex.doe@example.org", "alex.doe@example.org", "person"),
            c("benwidget1@example.org", "benwidget1@example.org", "person"),
            c("demo-unit-lee", "demo-unit-lee", "alias"),
            c("peer-benwidget1-zagros-2026-08-05-Garnet", "peer-benwidget1-zagros-2026-08-05-Garnet", "peer"),
        ]
    }

    /// The whole ask: start, middle, AND end all match.
    #[test]
    fn matches_at_start_middle_and_end() {
        let u = universe();
        assert!(!rank(&u, "sam", 10).is_empty(), "start");
        assert!(!rank(&u, "sample", 10).is_empty(), "middle");
        assert!(!rank(&u, "example.org", 10).is_empty(), "end");
        // mid-WORD, not just mid-string: "widget" sits inside "benwidget1"
        assert_eq!(rank(&u, "widget", 10).len(), 2); // the person and their peer
    }

    #[test]
    fn matching_is_case_insensitive_both_ways() {
        let u = universe();
        assert_eq!(rank(&u, "SAM", 10).len(), 1);
        assert_eq!(rank(&u, "garnet", 10).len(), 1);
    }

    /// Ranking is about ORDER, never membership — a middle hit still shows.
    #[test]
    fn prefix_outranks_middle_but_middle_still_appears() {
        let u = vec![
            c("zed.sam@example.test", "zed.sam@example.test", "person"),
            c("sam.sample@example.test", "sam.sample@example.test", "person"),
        ];
        let r = rank(&u, "sam", 10);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].selector, "sam.sample@example.test");
    }

    /// A word-boundary hit sorts above a mid-word one at equal kind.
    #[test]
    fn word_boundary_outranks_mid_word() {
        let u = vec![
            c("abcsample@example.test", "abcsample@example.test", "person"),
            c("sam.sample@example.test", "sam.sample@example.test", "person"),
        ];
        let r = rank(&u, "sample", 10);
        assert_eq!(r[0].selector, "sam.sample@example.test");
    }

    /// People before orgs before aliases before peers, at equal position.
    #[test]
    fn persons_sort_first() {
        let u = vec![
            c("peer-goat-x", "peer-goat-x", "peer"),
            c("goat-org", "goat-org", "org"),
            c("goat@example.test", "goat@example.test", "person"),
        ];
        let r = rank(&u, "goat", 10);
        assert_eq!(
            r.iter().map(|x| x.kind.as_str()).collect::<Vec<_>>(),
            vec!["person", "org", "peer"]
        );
    }

    /// A pasted UUID / mesh IP finds its row without those strings cluttering
    /// every suggestion.
    #[test]
    fn hidden_keys_match_but_rank_last() {
        let mut person = c("someone@example.test", "someone@example.test", "person");
        person.keys = vec!["7ba9fa8c-9a71-4d18-a28c-c2b6b2a90f5c".into()];
        let mut peer = c("peer-a", "peer-a", "peer");
        peer.keys = vec!["100.127.0.224".into()];
        let u = vec![person, peer];
        assert_eq!(rank(&u, "c2b6b2a9", 10).len(), 1);
        assert_eq!(rank(&u, "100.127.0.224", 10)[0].selector, "peer-a");
    }

    #[test]
    fn an_empty_query_suggests_nothing_rather_than_everything() {
        assert!(rank(&universe(), "", 10).is_empty());
        assert!(rank(&universe(), "   ", 10).is_empty());
    }

    #[test]
    fn duplicate_selectors_collapse_to_the_best_row() {
        let u = vec![
            c("goat-a", "goat-a", "peer"),
            c("goat-a", "goat-a", "person"),
        ];
        let r = rank(&u, "goat", 10);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].kind, "person");
    }

    #[test]
    fn the_limit_is_honoured() {
        let u: Vec<Candidate> = (0..50)
            .map(|i| c(&format!("goat{i}@example.test"), &format!("goat{i}"), "person"))
            .collect();
        assert_eq!(rank(&u, "goat", 7).len(), 7);
    }

    /// A no-match query and an unread roster are different answers, and the
    /// snapshot has to be able to say which. (The renderer's contract; this
    /// pins the field that carries it.)
    #[test]
    fn a_never_built_snapshot_says_the_roster_was_not_read() {
        let s = Snapshot::default();
        assert!(!s.roster_read);
        assert!(s.built_at.is_none());
    }
}
