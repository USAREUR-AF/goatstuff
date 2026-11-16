// SPDX-License-Identifier: Apache-2.0
//! What each router carries: a census of the subscriber and publisher
//! declarations this router's own adminspace holds, read on a cycle slower than
//! the snapshot and published as COUNTS.
//!
//! # What zenoh 1.9.0 attributes, and to whom
//!
//! Each entry of `@/<zid>/router/subscriber/**` is one key expression with the
//! sources that declared it, `{clients:[], peers:[], routers:[]}`
//! (zenoh-1.9.0 `net/runtime/adminspace.rs` `resources_data`):
//!
//! - `clients` / `peers`: the ZIDs of this router's OWN sessions that declared
//!   it (broker and peer hats, `hat/broker/pubsub.rs:195`). This router's own
//!   process appears here under its own ZID.
//! - `routers`: the routers of the north routing group that hold it
//!   (`hat/router/pubsub.rs:278`, `router_subs` minus this router), plus this
//!   router's own ZID for anything declared south of it
//!   (`dispatcher/tables.rs:339` `add_north_source`). A subscription declared
//!   by an application below another gateway is attributed to that gateway.
//!
//! Publisher entries (`router/publisher/**`) exist only for this router's own
//! sessions: the router hat reports none (`hat/router/pubsub.rs:301`), so how
//! many publishers ANOTHER router holds is not knowable from here.
//!
//! Neither table counts messages or bytes. Those need a router built with
//! zenoh's `stats` feature; the fleet build's `router/metrics` holds only
//! `zenoh_build_info`.
//!
//! # Load
//!
//! These two tables are the large part of a busy router's adminspace (hundreds
//! of entries). They are read on their own cycle
//! (`--declarations-interval-secs`, 300 s by default), with their own timeout
//! and an entry cap. A failed read publishes `read_error` and no counts, never
//! zeros.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::wire::{DeclarationsV1, RouterDeclarationsV1};

/// The longest key expression carried in a sample. A key past this is cut and
/// marked, so one pathological name cannot inflate a snapshot.
pub const MAX_KEY_CHARS: usize = 160;

#[derive(Debug, Deserialize)]
struct Entry {
    key: String,
    value: Sources,
}

#[derive(Debug, Default, Deserialize)]
struct Sources {
    #[serde(default)]
    clients: Vec<String>,
    #[serde(default)]
    peers: Vec<String>,
    #[serde(default)]
    routers: Vec<String>,
}

/// Limits on one census.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Entries counted per table; the rest are dropped and `capped` is set.
    pub max_entries: usize,
    /// Key expressions sampled per router (0 = none).
    pub top_keys: usize,
}

/// Parse one table's body into (key expression, sources), stripping the
/// `@/<zid>/router/<kind>/` prefix.
fn table(body: &[u8], kind: &str) -> Result<Vec<(String, Sources)>> {
    let entries: Vec<Entry> = serde_json::from_slice(body)
        .with_context(|| format!("{kind} table is not a JSON array of entries"))?;
    let marker = format!("/router/{kind}/");
    Ok(entries
        .into_iter()
        .filter_map(|e| {
            let (_, ke) = e.key.split_once(&marker)?;
            Some((ke.to_string(), e.value))
        })
        .collect())
}

fn clip(k: &str) -> String {
    if k.chars().count() <= MAX_KEY_CHARS {
        k.to_string()
    } else {
        let mut s: String = k.chars().take(MAX_KEY_CHARS).collect();
        s.push('…');
        s
    }
}

/// The first `n` keys by number of declaring sources, then by name. A sample
/// for an operator to recognize, not a ranking by traffic: no traffic is known.
fn sample(mut keys: Vec<(&str, usize)>, n: usize) -> (Vec<String>, bool) {
    keys.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let truncated = keys.len() > n;
    (
        keys.into_iter().take(n).map(|(k, _)| clip(k)).collect(),
        truncated,
    )
}

/// The router ZID a table's keys are under (`@/<zid>/router/...`).
fn owner_zid(body: &[u8]) -> Option<String> {
    let v: Vec<serde_json::Value> = serde_json::from_slice(body).ok()?;
    let k = v.first()?.get("key")?.as_str()?;
    Some(k.strip_prefix("@/")?.split('/').next()?.to_string())
}

/// Build the census from the two table bodies. This router's own ZID is read
/// from the keys, so a census never needs the snapshot to have succeeded.
pub fn census(subs: &[u8], pubs: &[u8], lim: Limits) -> Result<DeclarationsV1> {
    let me = owner_zid(subs)
        .or_else(|| owner_zid(pubs))
        .unwrap_or_default();
    let me = me.as_str();
    let mut subs = table(subs, "subscriber")?;
    let mut pubs = table(pubs, "publisher")?;
    let capped = subs.len() > lim.max_entries || pubs.len() > lim.max_entries;
    subs.truncate(lim.max_entries);
    pubs.truncate(lim.max_entries);

    let mut d = DeclarationsV1 {
        capped,
        ..Default::default()
    };
    let mut topics: BTreeSet<&str> = BTreeSet::new();
    let mut own_keys: Vec<(&str, usize)> = Vec::new();
    let mut per_router: BTreeMap<&str, Vec<(&str, usize)>> = BTreeMap::new();

    for (ke, s) in &subs {
        let sessions: Vec<&String> = s
            .clients
            .iter()
            .chain(&s.peers)
            .filter(|z| z.as_str() != me)
            .collect();
        let only_me = sessions.is_empty()
            && s.routers.iter().all(|z| z == me)
            && (s.clients.iter().any(|z| z == me) || s.peers.iter().any(|z| z == me));
        if only_me {
            d.own_process_subscribed_keys += 1;
            continue;
        }
        if !sessions.is_empty() {
            d.session_subscriptions += sessions.len() as u32;
            d.session_subscribed_keys += 1;
            own_keys.push((ke, sessions.len()));
            topics.insert(ke);
        }
        for r in s.routers.iter().filter(|z| z.as_str() != me) {
            per_router.entry(r).or_default().push((ke, 1));
            topics.insert(ke);
        }
    }
    for (ke, s) in &pubs {
        let sessions = s
            .clients
            .iter()
            .chain(&s.peers)
            .filter(|z| z.as_str() != me)
            .count();
        if sessions == 0 {
            continue;
        }
        d.session_publishers += sessions as u32;
        d.session_published_keys += 1;
        own_keys.push((ke, sessions));
        topics.insert(ke);
    }
    d.topics = topics.len() as u32;
    let (keys, t) = sample(own_keys, lim.top_keys);
    d.sample_keys = keys;
    d.sample_truncated = t;
    d.routers = per_router
        .into_iter()
        .map(|(zid, keys)| {
            let n = keys.len() as u32;
            let (sample_keys, sample_truncated) = sample(keys, lim.top_keys);
            RouterDeclarationsV1 {
                zid: zid.to_string(),
                subscribed_keys: n,
                sample_keys,
                sample_truncated,
            }
        })
        .collect();
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIM: Limits = Limits {
        max_entries: 10_000,
        top_keys: 2,
    };

    /// Shaped from a live router's tables, with synthetic ZIDs: this router's
    /// own process (`aa00`), two of its clients, a neighbor router `bb11`
    /// whose application subscribes, and one router two hops away (`cc22`).
    fn subs() -> String {
        serde_json::json!([
            {"key": "@/aa00/router/subscriber/5f1e/**", "value": {"clients": ["aa00"], "peers": [], "routers": ["aa00"]}},
            {"key": "@/aa00/router/subscriber/9e0d/**", "value": {"clients": ["aa00"], "peers": [], "routers": ["aa00"]}},
            {"key": "@/aa00/router/subscriber/geo/track/v1/*", "value": {"clients": ["c001", "c002"], "peers": [], "routers": ["aa00"]}},
            {"key": "@/aa00/router/subscriber/ops/fabric/topology/**", "value": {"clients": ["c001"], "peers": [], "routers": ["aa00"]}},
            {"key": "@/aa00/router/subscriber/rig/b/sub/0", "value": {"clients": [], "peers": [], "routers": ["bb11"]}},
            {"key": "@/aa00/router/subscriber/rig/b/sub/1", "value": {"clients": [], "peers": [], "routers": ["bb11"]}},
            {"key": "@/aa00/router/subscriber/rig/b/sub/2", "value": {"clients": [], "peers": [], "routers": ["bb11", "cc22"]}},
            {"key": "@/aa00/router/subscriber/rig/c/sub/0", "value": {"clients": [], "peers": [], "routers": ["cc22"]}}
        ])
        .to_string()
    }
    fn pubs() -> String {
        serde_json::json!([
            {"key": "@/aa00/router/publisher/geo/track/v1/901", "value": {"clients": ["c002"], "peers": [], "routers": []}},
            {"key": "@/aa00/router/publisher/geo/track/v1/902", "value": {"clients": ["c002"], "peers": [], "routers": []}}
        ])
        .to_string()
    }

    #[test]
    fn counts_what_each_source_declared() {
        let d = census(subs().as_bytes(), pubs().as_bytes(), LIM).unwrap();
        assert_eq!(d.own_process_subscribed_keys, 2, "the router's own process");
        assert_eq!(d.session_subscribed_keys, 2);
        assert_eq!(
            d.session_subscriptions, 3,
            "two clients on one key, one on another"
        );
        assert_eq!(d.session_published_keys, 2);
        assert_eq!(d.session_publishers, 2);
        // 2 subscribed + 2 published by sessions, 4 held by other routers.
        assert_eq!(d.topics, 8);
        let b = d.routers.iter().find(|r| r.zid == "bb11").unwrap();
        assert_eq!(b.subscribed_keys, 3);
        let c = d.routers.iter().find(|r| r.zid == "cc22").unwrap();
        assert_eq!(
            c.subscribed_keys, 2,
            "a router two hops out is attributed too"
        );
        assert!(
            d.routers.iter().all(|r| r.zid != "aa00"),
            "self is not a neighbor"
        );
        assert!(!d.capped);
    }

    #[test]
    fn samples_are_capped_and_ordered_by_sources_then_name() {
        let d = census(subs().as_bytes(), pubs().as_bytes(), LIM).unwrap();
        assert_eq!(d.sample_keys, vec!["geo/track/v1/*", "geo/track/v1/901"]);
        assert!(d.sample_truncated);
        let b = d.routers.iter().find(|r| r.zid == "bb11").unwrap();
        assert_eq!(b.sample_keys.len(), 2);
        assert!(b.sample_truncated);
    }

    #[test]
    fn the_entry_cap_sets_capped() {
        let lim = Limits {
            max_entries: 3,
            top_keys: 0,
        };
        let d = census(subs().as_bytes(), pubs().as_bytes(), lim).unwrap();
        assert!(d.capped);
        assert!(d.sample_keys.is_empty());
    }

    #[test]
    fn a_long_key_is_clipped() {
        let long = "k/".repeat(200);
        let body = serde_json::json!([
            {"key": format!("@/aa00/router/subscriber/{long}"), "value": {"clients": ["c001"], "peers": [], "routers": ["aa00"]}}
        ])
        .to_string();
        let d = census(body.as_bytes(), b"[]", LIM).unwrap();
        assert_eq!(d.sample_keys[0].chars().count(), MAX_KEY_CHARS + 1);
        assert!(d.sample_keys[0].ends_with('…'));
    }

    #[test]
    fn a_body_that_is_not_a_table_is_an_error_not_zeros() {
        assert!(census(b"<html>", b"[]", LIM).is_err());
    }
}
