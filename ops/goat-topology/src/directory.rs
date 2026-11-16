// SPDX-License-Identifier: Apache-2.0
//! Who a router belongs to: the mesh peer it connects from, and the
//! organization, nation and individual the identity registry records for it.
//!
//! # Two files, both already on the management host, no credential
//!
//! - **Peer directory** (`--peer-directory`): a mesh address → peer name,
//!   hostname and groups. Two shapes are read, because two existing writers
//!   produce one: the relay-health peer-names file (`{"names": {<id>: {name,
//!   ip, host, aff, …}}}`, `aff` being the peer's mesh groups as a comma list)
//!   and the mesh-health peer registry (the mesh controller's peer list, an
//!   array of `{ip, name, hostname, groups:[{name}]}`). This process reads the
//!   file; it never calls the mesh controller.
//! - **Roster** (`--roster`): the identity registry's public-safe projection,
//!   `{"orgs": [{id, nation, display_name?, members:[{handle, type, custody?}]}],
//!   "members": [{handle, type, org, nationality}]}`.
//!
//! # How a peer reaches the roster, and what that join is worth
//!
//! A peer name is evidence about ownership, not a record of it. The name is
//! used only to find a roster handle, by the onboarding grammar the peerview
//! surface already uses: `peer-<handle>[-<goatnet>]-<yyyy>-<mm>-<dd>-<host>` for
//! a person's device, and `machine-<handle>[-<host>]` for a machine, matched
//! against the roster's handles, longest first. The handle is never decomposed
//! further. Organization, organization nation and person nationality are then
//! READ from the roster, never derived from the name, a group or an address,
//! and every owner carries `attribution: "peer-name"` so the page can say so.
//!
//! A person's nationality and an organization's nation are two facts and are
//! kept apart: an organization's nation is never shown as a person's
//! nationality, and a machine has no nationality. Absent means not recorded.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// One mesh peer, as the peer directory lists it.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct Peer {
    pub name: String,
    pub hostname: String,
    pub groups: Vec<String>,
    /// What the identity record says about this peer's owner, as the host
    /// writer carried it from the portal resolver. `None` when the file
    /// carries no owner block (a writer older than owner-from-record).
    pub owner: Option<RecordOwner>,
    /// An owner exception the operator granted for this peer (goatcore ADR
    /// 1249 decision 7), as the host writer carried it. Shown, never hidden.
    pub exception: Option<OwnerException>,
}

/// One owner exception: a decision the operator made about one peer.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct OwnerException {
    pub reason: String,
    pub granted_by: String,
    pub granted_on: String,
    /// Set only by the operator; empty when not set.
    pub review_by: String,
    /// `active`, `stale` (the record now names an owner: remove it), or
    /// `malformed` (the card's exception lacks a reason, grantor or date, or
    /// carries a state this surface does not know): shown, never applied.
    pub state: String,
    /// Why a malformed exception is malformed; empty otherwise.
    pub problem: String,
}

impl OwnerException {
    fn from_json(v: &serde_json::Value) -> Option<Self> {
        let g = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string()
        };
        v.as_object()?;
        let mut e = OwnerException {
            reason: g("reason"),
            granted_by: g("granted_by"),
            granted_on: g("granted_on"),
            review_by: g("review_by"),
            state: g("state"),
            problem: String::new(),
        };
        let mut why: Vec<String> = ["reason", "granted_by", "granted_on"]
            .iter()
            .zip([&e.reason, &e.granted_by, &e.granted_on])
            .filter(|(_, v)| v.is_empty())
            .map(|(k, _)| format!("no {k}"))
            .collect();
        if !matches!(e.state.as_str(), "active" | "stale") {
            why.push(if e.state.is_empty() {
                "no state".to_string()
            } else {
                format!("unknown state {:?}", e.state)
            });
        }
        if !why.is_empty() {
            e.state = "malformed".into();
            e.problem = why.join(", ");
        }
        Some(e)
    }

    /// Whether this exception applies: a well-formed active one.
    pub fn applies(&self) -> bool {
        self.state == "active"
    }
}

/// The portal resolver's owner block for one peer, as `peer-names.json`
/// carries it (goatcore `docs/design/peer-identity-resolution.md`, "The owner
/// comes from the record, not the roster"). Only identifiers and states: the
/// writer drops e-mail and display name.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct RecordOwner {
    /// `record`, `declared`, `roster`, `none` or `unread`.
    pub source: String,
    /// `machine-custody`, `slot-identity`, … (for `record`).
    pub via: String,
    /// What the first hop matched on (`peer-name` today).
    pub matched_by: String,
    pub reason: String,
    /// The responsible person's handle. Empty when the surface hides the
    /// individual. The person's identity id is never carried to a browser.
    pub person_handle: String,
    /// The record says the responsible person's identity is disabled.
    pub person_disabled: bool,
    pub org_id: String,
    pub org_short_tag: String,
    /// The ORGANIZATION's nation.
    pub org_nation: String,
    /// `recorded`, or why not (`missing`); with the record's reason.
    pub org_state: String,
    pub org_reason: String,
    /// The PERSON's own nationality, and its state (`recorded`, `not_stamped`).
    pub nationality: String,
    pub nationality_state: String,
    /// A machine's recorded unit, beside its custodian's organization.
    pub machine_unit: String,
}

impl RecordOwner {
    fn from_json(v: &serde_json::Value) -> Option<Self> {
        let o = v.as_object()?;
        let s = |m: Option<&serde_json::Value>, k: &str| -> String {
            m.and_then(|m| m.get(k))
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let top = Some(v);
        let person = o.get("person");
        let org = o.get("org");
        let nat = o.get("nationality");
        let source = s(top, "source");
        if source.is_empty() {
            return None;
        }
        Some(RecordOwner {
            source,
            via: s(top, "via"),
            matched_by: s(top, "matched_by"),
            reason: {
                let r = s(top, "reason");
                if r.is_empty() {
                    s(top, "note")
                } else {
                    r
                }
            },
            person_handle: s(person, "handle"),
            person_disabled: person
                .and_then(|p| p.get("disabled"))
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
            org_id: s(org, "id"),
            org_short_tag: s(org, "short_tag"),
            org_nation: s(org, "nation"),
            org_state: s(org, "state"),
            org_reason: s(org, "reason"),
            nationality: s(nat, "value"),
            nationality_state: s(nat, "state"),
            machine_unit: s(top, "machine_unit"),
        })
    }
}

/// Whether the host writer could ask the portal at all, from the file's
/// `owner_read`. `None` when the file predates it.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct OwnerRead {
    pub state: String,
    pub reason: String,
    pub at: u64,
}

/// What the roster records for the peer, when its name leads to a handle.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct Owner {
    /// `person` (the name follows the person-device grammar) or `machine`.
    pub kind: &'static str,
    /// The roster handle the name led to (the machine's own handle for a
    /// machine).
    pub handle: String,
    pub org: Option<String>,
    pub org_display_name: Option<String>,
    /// The ORGANIZATION's nation (roster `orgs[].nation`).
    pub org_nation: Option<String>,
    /// The individual: the person for a person's device, the custodian who
    /// enrolled it for a machine. `None` when the roster records none, or when
    /// the surface is run with the individual hidden.
    pub individual: Option<String>,
    /// That individual's OWN nationality (roster `members[].nationality`).
    /// Never the organization's nation.
    pub individual_nationality: Option<String>,
    /// How the owner was found. Always `peer-name` today: the name led to the
    /// handle. The recorded linkage (the portal's slot registry) needs the
    /// portal operator key and is not read.
    pub attribution: &'static str,
}

#[derive(Debug, Deserialize)]
struct RelayFile {
    names: BTreeMap<String, RelayCard>,
    #[serde(default)]
    owner_read: Option<serde_json::Value>,
    /// Unix seconds when the writer generated the file.
    #[serde(default)]
    generated_at: Option<u64>,
}
#[derive(Debug, Deserialize)]
struct RelayCard {
    #[serde(default)]
    name: String,
    #[serde(default)]
    ip: String,
    #[serde(default)]
    host: String,
    #[serde(default)]
    aff: String,
    #[serde(default)]
    owner: Option<serde_json::Value>,
    #[serde(default)]
    owner_exception: Option<serde_json::Value>,
}
#[derive(Debug, Deserialize)]
struct RegistryPeer {
    #[serde(default)]
    ip: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    groups: Vec<RegistryGroup>,
}
#[derive(Debug, Deserialize)]
struct RegistryGroup {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct Roster {
    #[serde(default)]
    orgs: Vec<RosterOrg>,
    #[serde(default)]
    members: Vec<RosterMember>,
}
#[derive(Debug, Deserialize)]
struct RosterOrg {
    id: String,
    #[serde(default)]
    nation: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    members: Vec<RosterOrgMember>,
}
#[derive(Debug, Deserialize)]
struct RosterOrgMember {
    #[serde(default)]
    handle: String,
    #[serde(default)]
    custody: Option<String>,
}
#[derive(Debug, Deserialize)]
struct RosterMember {
    handle: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    org: Option<String>,
    #[serde(default)]
    nationality: Option<String>,
}

/// Normalize an address as the directory and a session link spell it:
/// `100.127.0.7`, `100.127.0.7/16`, `tls/100.127.0.7:41544`, `[fd00::1]:7447`.
pub fn address_of(s: &str) -> Option<IpAddr> {
    let s = s.trim().trim_matches('"');
    let s = s.split_once('/').map_or(s, |(proto, rest)| {
        // `tls/addr:port` → `addr:port`; `addr/16` → `addr`.
        if proto.parse::<IpAddr>().is_ok() {
            proto
        } else {
            rest
        }
    });
    let s = s.split('#').next().unwrap_or(s);
    if let Ok(a) = s.parse::<IpAddr>() {
        return Some(a);
    }
    if let Ok(sa) = s.parse::<std::net::SocketAddr>() {
        return Some(sa.ip());
    }
    None
}

/// The two inputs, parsed, and the joins over them.
#[derive(Debug, Default)]
pub struct Directory {
    peers: BTreeMap<IpAddr, Peer>,
    roster: Roster,
    goatnet: String,
    show_individual: bool,
    /// Why a configured input could not be used; shown once on the page.
    pub errors: Vec<String>,
    /// Which inputs are configured, so "not recorded" can say why.
    pub peers_configured: bool,
    pub roster_configured: bool,
    /// When each file was last written (Unix ms; 0 when unread), so the page
    /// can say a file is old. Absolute times, not ages, so the payload does
    /// not change while nothing happens.
    pub peers_modified_ms: u64,
    pub roster_modified_ms: u64,
    /// Whether the host writer could ask the portal for owners (the peer
    /// directory's `owner_read`). `None` when the file carries none.
    pub owner_read: Option<OwnerRead>,
    /// When the writer generated the peer directory (Unix ms; 0 when the file
    /// does not say).
    pub peers_generated_ms: u64,
    /// Past this age (ms) the owners in the peer directory are not trusted:
    /// the writer runs every 5 minutes, so an older file means it stopped.
    /// 0 turns the check off.
    pub peers_max_age_ms: u64,
    /// Whether the individual (person, their nationality, an exception's
    /// grantor) is hidden on this surface.
    pub individual_hidden: bool,
}

impl Directory {
    /// When the peer directory was last written: the writer's own time when
    /// the file states one, else the file's modification time. 0 when unknown.
    pub fn peers_written_ms(&self) -> u64 {
        let at = self.owner_read.as_ref().map_or(0, |r| r.at * 1000);
        match self.peers_generated_ms.max(at) {
            0 => self.peers_modified_ms,
            stated => stated,
        }
    }

    /// The peer directory's age when it is past its limit, else `None`.
    pub fn peers_too_old(&self, now_ms: u64) -> Option<u64> {
        let w = self.peers_written_ms();
        if self.peers_max_age_ms == 0 || w == 0 {
            return None;
        }
        let age = now_ms.saturating_sub(w);
        (age > self.peers_max_age_ms).then_some(age)
    }

    pub fn peer_at(&self, addr: &str) -> Option<&Peer> {
        self.peers.get(&address_of(addr)?)
    }

    fn member(&self, handle: &str) -> Option<&RosterMember> {
        self.roster.members.iter().find(|m| m.handle == handle)
    }

    fn org(&self, id: &str) -> Option<&RosterOrg> {
        self.roster.orgs.iter().find(|o| o.id == id)
    }

    fn custodian(&self, handle: &str) -> Option<&str> {
        self.roster
            .orgs
            .iter()
            .flat_map(|o| &o.members)
            .find(|m| m.handle == handle)
            .and_then(|m| m.custody.as_deref())
            .filter(|c| !c.is_empty())
    }

    /// The person handle in `peer-<handle>[-<goatnet>]-<yyyy>-<mm>-<dd>-<host>`.
    /// Greedy up to the date, as peerview's `handle_of`, so an env token cannot
    /// swallow part of a handle.
    fn person_handle(&self, name: &str) -> Option<String> {
        let rest = name.strip_prefix("peer-")?;
        // Find the last `-dddd-dd-dd-` and take everything before it.
        let b = rest.as_bytes();
        let mut cut = None;
        for i in 0..b.len() {
            let w = &b[i..];
            if w.len() >= 12
                && w[0] == b'-'
                && w[1..5].iter().all(u8::is_ascii_digit)
                && w[5] == b'-'
                && w[6..8].iter().all(u8::is_ascii_digit)
                && w[8] == b'-'
                && w[9..11].iter().all(u8::is_ascii_digit)
                && w[11] == b'-'
            {
                cut = Some(i);
            }
        }
        let mut h = &rest[..cut?];
        if !self.goatnet.is_empty() {
            let suffix = format!("-{}", self.goatnet);
            if h.to_ascii_lowercase()
                .ends_with(&suffix.to_ascii_lowercase())
            {
                h = &h[..h.len() - suffix.len()];
            }
        }
        (!h.is_empty()).then(|| h.to_string())
    }

    /// The machine handle in `machine-<handle>[-<host>]`, longest roster
    /// handle first, as the portal's `MatchMachineHandle`.
    fn machine_handle(&self, name: &str) -> Option<String> {
        let rest = name.strip_prefix("machine-")?;
        self.roster
            .members
            .iter()
            .map(|m| m.handle.as_str())
            .filter(|h| !h.is_empty() && (rest == *h || rest.starts_with(&format!("{h}-"))))
            .max_by_key(|h| h.len())
            .map(String::from)
    }

    /// The owner the roster records for a peer name, or `None` when the name
    /// leads to no roster handle.
    pub fn owner_of(&self, peer_name: &str) -> Option<Owner> {
        let (kind, handle) = if let Some(h) = self.machine_handle(peer_name) {
            ("machine", h)
        } else {
            let h = self.person_handle(peer_name)?;
            self.member(&h)?;
            ("person", h)
        };
        let m = self.member(&handle)?;
        let org = m.org.clone().filter(|o| !o.is_empty());
        let o = org.as_deref().and_then(|id| self.org(id));
        let individual = match kind {
            "machine" => self.custodian(&handle).map(String::from),
            _ => Some(handle.clone()),
        };
        let individual_nationality = individual
            .as_deref()
            .and_then(|h| self.member(h))
            .filter(|p| p.kind != "system")
            .and_then(|p| p.nationality.clone())
            .filter(|n| !n.is_empty());
        let (individual, individual_nationality) = if self.show_individual {
            (individual, individual_nationality)
        } else {
            (None, None)
        };
        Some(Owner {
            kind,
            handle,
            org_display_name: o.and_then(|o| o.display_name.clone()),
            org_nation: o.and_then(|o| o.nation.clone()).filter(|n| !n.is_empty()),
            org,
            individual,
            individual_nationality,
            attribution: "peer-name",
        })
    }
}

type Parsed = (BTreeMap<IpAddr, Peer>, Option<OwnerRead>, u64);

/// Drop what names the individual, when the surface hides it.
fn hide_individual(peers: &mut BTreeMap<IpAddr, Peer>) {
    for p in peers.values_mut() {
        if let Some(o) = p.owner.as_mut() {
            o.person_handle.clear();
            o.nationality.clear();
            o.nationality_state.clear();
        }
        if let Some(e) = p.exception.as_mut() {
            e.granted_by.clear();
        }
    }
}

fn parse_peers(body: &[u8]) -> Result<Parsed> {
    let v: serde_json::Value =
        serde_json::from_slice(body).context("peer directory is not JSON")?;
    let mut out = BTreeMap::new();
    let mut read = None;
    let mut generated_ms = 0;
    if v.get("names").is_some() {
        let f: RelayFile = serde_json::from_value(v).context("peer-names shape")?;
        generated_ms = f.generated_at.unwrap_or(0) * 1000;
        read = f.owner_read.as_ref().and_then(|r| {
            Some(OwnerRead {
                state: r.get("state")?.as_str()?.to_string(),
                reason: r
                    .get("reason")
                    .and_then(|x| x.as_str())
                    .unwrap_or_default()
                    .to_string(),
                at: r.get("at").and_then(|x| x.as_u64()).unwrap_or(0),
            })
        });
        for c in f.names.into_values() {
            if let Some(a) = address_of(&c.ip) {
                let groups = c
                    .aff
                    .split(',')
                    .map(str::trim)
                    .filter(|g| !g.is_empty())
                    .map(String::from)
                    .collect();
                out.insert(
                    a,
                    Peer {
                        name: c.name,
                        hostname: c.host,
                        groups,
                        owner: c.owner.as_ref().and_then(RecordOwner::from_json),
                        exception: c
                            .owner_exception
                            .as_ref()
                            .and_then(OwnerException::from_json),
                    },
                );
            }
        }
    } else if v.is_array() {
        let ps: Vec<RegistryPeer> = serde_json::from_value(v).context("peer registry shape")?;
        for p in ps {
            if let Some(a) = address_of(&p.ip) {
                let groups = p
                    .groups
                    .into_iter()
                    .map(|g| g.name)
                    .filter(|g| !g.is_empty() && g != "All")
                    .collect();
                out.insert(
                    a,
                    Peer {
                        name: p.name,
                        hostname: p.hostname,
                        groups,
                        owner: None,
                        exception: None,
                    },
                );
            }
        }
    } else {
        anyhow::bail!("peer directory is neither a peer-names file nor a peer list");
    }
    Ok((out, read, generated_ms))
}

/// Where the inputs live, and their last-seen modification times, so a reload
/// happens only when a file changed.
#[derive(Debug, Default)]
pub struct Sources {
    pub peers: Option<PathBuf>,
    pub roster: Option<PathBuf>,
    pub goatnet: String,
    pub show_individual: bool,
    /// See [`Directory::peers_max_age_ms`].
    pub peers_max_age_ms: u64,
    seen: (Option<SystemTime>, Option<SystemTime>),
    loaded: bool,
}

impl Sources {
    pub fn new(
        peers: Option<PathBuf>,
        roster: Option<PathBuf>,
        goatnet: String,
        show_individual: bool,
    ) -> Self {
        Self {
            peers,
            roster,
            goatnet,
            show_individual,
            peers_max_age_ms: 20 * 60 * 1000,
            seen: (None, None),
            loaded: false,
        }
    }

    fn mtime_ms(p: &PathBuf) -> u64 {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as u64)
    }

    fn mtime(p: &Option<PathBuf>) -> Option<SystemTime> {
        p.as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok())
    }

    /// `Some(directory)` when either file changed since the last call (the
    /// first call always loads); `None` when nothing changed.
    pub fn reload_if_changed(&mut self) -> Option<Directory> {
        let now = (Self::mtime(&self.peers), Self::mtime(&self.roster));
        if self.loaded && self.seen == now {
            return None;
        }
        self.seen = now;
        self.loaded = true;
        Some(self.load())
    }

    pub fn load(&self) -> Directory {
        let mut d = Directory {
            goatnet: self.goatnet.clone(),
            show_individual: self.show_individual,
            individual_hidden: !self.show_individual,
            peers_max_age_ms: self.peers_max_age_ms,
            peers_configured: self.peers.is_some(),
            roster_configured: self.roster.is_some(),
            ..Default::default()
        };
        if let Some(p) = &self.peers {
            match std::fs::read(p)
                .with_context(|| format!("read {}", p.display()))
                .and_then(|b| parse_peers(&b))
            {
                Ok((mut m, read, generated_ms)) => {
                    if !self.show_individual {
                        hide_individual(&mut m);
                    }
                    d.peers = m;
                    d.owner_read = read;
                    d.peers_generated_ms = generated_ms;
                    d.peers_modified_ms = Self::mtime_ms(p);
                }
                Err(e) => d.errors.push(format!("peer directory not read: {e:#}")),
            }
        }
        if let Some(p) = &self.roster {
            match std::fs::read(p)
                .with_context(|| format!("read {}", p.display()))
                .and_then(|b| serde_json::from_slice::<Roster>(&b).context("roster shape"))
            {
                Ok(r) => {
                    d.roster = r;
                    d.roster_modified_ms = Self::mtime_ms(p);
                }
                Err(e) => d.errors.push(format!("roster not read: {e:#}")),
            }
        }
        d
    }
}

#[cfg(test)]
pub mod testing {
    use super::*;

    /// A directory built from in-memory JSON, for the view tests.
    pub fn directory(peers: &str, roster: &str, show_individual: bool) -> Directory {
        let (mut peers, owner_read, peers_generated_ms) = parse_peers(peers.as_bytes()).unwrap();
        if !show_individual {
            hide_individual(&mut peers);
        }
        Directory {
            peers,
            owner_read,
            peers_generated_ms,
            peers_max_age_ms: 20 * 60 * 1000,
            individual_hidden: !show_individual,
            roster: serde_json::from_str(roster).unwrap(),
            goatnet: "zagros".into(),
            show_individual,
            peers_configured: true,
            roster_configured: true,
            peers_modified_ms: 0,
            roster_modified_ms: 0,
            errors: vec![],
        }
    }

    pub const ROSTER: &str = r#"{
      "orgs": [
        {"id": "org-anvil", "nation": "AAA", "display_name": "Anvil Works", "members": [
          {"handle": "rivera-j", "type": "person"},
          {"handle": "anvil-pod-7", "type": "system", "custody": "rivera-j"}]},
        {"id": "org-birch", "nation": "BBB", "members": [
          {"handle": "okafor-n", "type": "person"}]},
        {"id": "org-cedar", "members": [
          {"handle": "cedar-relay", "type": "system"}]}
      ],
      "members": [
        {"handle": "rivera-j", "type": "person", "org": "org-anvil", "nationality": "CCC"},
        {"handle": "anvil-pod-7", "type": "system", "org": "org-anvil", "nationality": null},
        {"handle": "okafor-n", "type": "person", "org": "org-birch", "nationality": null},
        {"handle": "cedar-relay", "type": "system", "org": "org-cedar"}
      ]
    }"#;
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    const RELAY: &str = r#"{"names": {
        "sha-01": {"name": "machine-anvil-pod-7-rack2", "ip": "100.127.0.21", "host": "anvil-pod-7", "aff": "ZAGROS-PARTNER,ZAGROS-OBSERVER"},
        "sha-02": {"name": "peer-okafor-n-zagros-2026-10-02-laptop", "ip": "\"100.127.0.22\"", "host": "nb-laptop", "aff": ""},
        "sha-03": {"name": "peer-rivera-j-2026-09-30-desk", "ip": "100.127.0.23", "host": "desk", "aff": "ZAGROS-DEVELOPER"},
        "sha-04": {"name": "edge-gw-unlisted", "ip": "100.127.0.24", "host": "edge", "aff": ""}
    }, "generated_at": 0, "count": 4}"#;

    #[test]
    fn addresses_normalize_from_every_spelling() {
        let a: IpAddr = "100.127.0.7".parse().unwrap();
        for s in [
            "100.127.0.7",
            "100.127.0.7/16",
            "tls/100.127.0.7:41544",
            "tcp/100.127.0.7:7447#bind=0",
            "\"100.127.0.7\"",
        ] {
            assert_eq!(address_of(s), Some(a), "{s}");
        }
        assert_eq!(
            address_of("tls/[fd00::1]:7447"),
            Some("fd00::1".parse().unwrap())
        );
        assert_eq!(address_of("tls/example.org:7447"), None);
    }

    #[test]
    fn reads_both_directory_shapes() {
        let d = directory(RELAY, ROSTER, true);
        let p = d.peer_at("tls/100.127.0.21:50000").unwrap();
        assert_eq!(p.name, "machine-anvil-pod-7-rack2");
        assert_eq!(p.groups, vec!["ZAGROS-PARTNER", "ZAGROS-OBSERVER"]);
        assert_eq!(d.peer_at("100.127.0.22").unwrap().hostname, "nb-laptop");

        let reg = r#"[{"ip": "100.127.0.30", "name": "peer-x", "hostname": "h",
                       "groups": [{"name": "All"}, {"name": "ZAGROS-OPS"}]}]"#;
        let d = directory(reg, ROSTER, true);
        assert_eq!(
            d.peer_at("100.127.0.30").unwrap().groups,
            vec!["ZAGROS-OPS"]
        );
    }

    #[test]
    fn a_machine_answers_through_its_custodian_and_has_no_nationality() {
        let d = directory(RELAY, ROSTER, true);
        let o = d.owner_of("machine-anvil-pod-7-rack2").unwrap();
        assert_eq!(o.kind, "machine");
        assert_eq!(o.handle, "anvil-pod-7");
        assert_eq!(o.org.as_deref(), Some("org-anvil"));
        assert_eq!(o.org_nation.as_deref(), Some("AAA"));
        assert_eq!(o.individual.as_deref(), Some("rivera-j"), "the custodian");
        assert_eq!(
            o.individual_nationality.as_deref(),
            Some("CCC"),
            "the custodian's own nationality, not the org's nation"
        );
    }

    #[test]
    fn a_person_nationality_is_never_filled_from_the_org_nation() {
        let d = directory(RELAY, ROSTER, true);
        let o = d
            .owner_of("peer-okafor-n-zagros-2026-10-02-laptop")
            .unwrap();
        assert_eq!(o.kind, "person");
        assert_eq!(o.handle, "okafor-n", "goatnet token stripped");
        assert_eq!(o.org_nation.as_deref(), Some("BBB"));
        assert_eq!(
            o.individual_nationality, None,
            "not recorded stays not recorded"
        );
    }

    #[test]
    fn an_org_with_no_nation_and_a_machine_with_no_custodian() {
        let d = directory(RELAY, ROSTER, true);
        let o = d.owner_of("machine-cedar-relay").unwrap();
        assert_eq!(o.org.as_deref(), Some("org-cedar"));
        assert_eq!(o.org_nation, None);
        assert_eq!(o.individual, None);
    }

    #[test]
    fn a_name_outside_the_grammar_or_the_roster_has_no_owner() {
        let d = directory(RELAY, ROSTER, true);
        assert!(d.owner_of("edge-gw-unlisted").is_none());
        assert!(d.owner_of("peer-nobody-2026-01-01-box").is_none());
        // A machine handle must match a whole roster handle, not a prefix of one.
        assert!(d.owner_of("machine-anvil").is_none());
    }

    #[test]
    fn the_individual_can_be_hidden() {
        let d = directory(RELAY, ROSTER, false);
        let o = d.owner_of("peer-rivera-j-2026-09-30-desk").unwrap();
        assert_eq!(o.org.as_deref(), Some("org-anvil"));
        assert_eq!(o.individual, None);
        assert_eq!(o.individual_nationality, None);
    }

    #[test]
    fn a_bad_file_is_an_error_on_the_page_not_an_empty_directory() {
        let dir = std::env::temp_dir().join(format!("gt-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("peers.json");
        std::fs::write(&p, "{not json").unwrap();
        let r = dir.join("roster.json");
        std::fs::write(&r, testing::ROSTER).unwrap();
        let d = Sources::new(Some(p), Some(r), String::new(), true).load();
        assert_eq!(d.errors.len(), 1, "{:?}", d.errors);
        assert!(d.errors[0].starts_with("peer directory not read"));
        assert_eq!(d.peers_modified_ms, 0, "an unread file has no time");
        assert!(
            d.roster_modified_ms > 0,
            "the roster's write time is carried"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
