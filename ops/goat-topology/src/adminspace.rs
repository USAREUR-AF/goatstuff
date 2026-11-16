// SPDX-License-Identifier: Apache-2.0
//! Reads one router's OBSERVED state from its own adminspace.
//!
//! # Two facts that shape this module
//!
//! **The adminspace is on host loopback.** `8000/tcp -> 127.0.0.1:8000`.
//! The publisher needs no `nsenter`, no
//! `--privileged` and no client cert to read the router it sits next to. What
//! *is* invisible from the host namespace is the router's zenoh SESSIONS as
//! `ss` sees them — a debugging trap, not a constraint on this code.
//!
//! **`@/local/**`, never `@/**`.** An adminspace query is ROUTED: `@/**`
//! returns the adminspace of every reachable router, so a per-router census
//! silently becomes a fabric-wide one and inflates a route-table count. On a goat
//! goatnet the fan-out is masked by `access_control` default-deny, which is what
//! makes the mistake latent rather than obvious. `local` is the alias for this
//! router alone.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::wire::{LinkV1, NextHopV1, RegionV1, RouterTopologyV1, SessionV1, MAX_LINKS_PER_REGION};

/// One `{key, value}` pair as the REST plugin renders the adminspace.
#[derive(Debug, Deserialize)]
struct AdminEntry {
    key: String,
    value: serde_json::Value,
}

/// The keys one snapshot reads: this router's sessions, its link-state graphs,
/// its route table and its version. Each is scoped below `router`, so the
/// snapshot never pulls the subscriber, queryable, token or transport tables —
/// on a busy router those are hundreds of entries the snapshot does not use.
/// The declaration tables are read on their own, slower cycle
/// ([`crate::declarations`]).
///
/// The route table is read last, and only as [`RouteTable`] allows: see there
/// for why.
const ROUTER_KEY: &str = "@/local/router";
const LINKSTATE_KEY: &str = "@/local/router/linkstate/*";
const VERSION_KEY: &str = "@/local/router/status/plugins/rest/version";
const ROUTE_TABLE_KEY: &str = "@/local/router/route/successor/**";

/// When to read the route table (`route/successor/**`).
///
/// # Why this is a setting
///
/// In zenoh 1.9.0 (and 1.8.0 and 1.10.x), a `route/successor` query that
/// arrives after a router joins or leaves, but before the routing trees are
/// recomputed (100 ms later by default), indexes past the end of a routing
/// tree and panics the router; a release build then aborts the whole
/// `zenohd` process (eclipse-zenoh/zenoh#2831, open). A router fabric whose
/// membership is changing is exactly when a publisher on every router reads
/// it, so reading the table unconditionally can take a router down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum RouteTable {
    /// Read it every cycle, whatever the routing map is doing. Use only with a
    /// router build that has the fix for zenoh#2831.
    Always,
    /// Read the routing map twice, `--route-table-settle-ms` apart, and read
    /// the route table only if the two agree. A router that joined or left
    /// within that time skips the read for this cycle. This narrows the
    /// window in which the read can land; it does not close it.
    Settled,
    /// Never read it. The page shows route entries as not read, and the
    /// route-table finding (T-005) does not run.
    Off,
}

/// How a snapshot read treats the route table.
#[derive(Clone, Copy, Debug)]
pub struct RoutePolicy {
    pub mode: RouteTable,
    pub settle_ms: u64,
}

/// The largest adminspace body read. A body past this is refused, not
/// truncated: half a JSON array is not a smaller answer, it is no answer.
pub const MAX_BODY_BYTES: u64 = 32 * 1024 * 1024;

/// GET one adminspace key over plaintext loopback HTTP.
///
/// Hand-rolled rather than pulling an HTTP client crate: the request is one
/// fixed GET to 127.0.0.1 with no TLS, no redirects, no auth and no
/// content-negotiation. A client dependency would add supply-chain surface to
/// the image for thirty lines of work.
pub async fn fetch_key(addr: &str, key: &str, timeout_secs: u64) -> Result<Vec<u8>> {
    let fut = async {
        let mut stream = TcpStream::connect(addr)
            .await
            .with_context(|| format!("connect adminspace {addr}"))?;
        let req = format!(
            "GET /{key} HTTP/1.1\r\nHost: {addr}\r\nAccept: application/json\r\n\
             User-Agent: goat-topology\r\nConnection: close\r\n\r\n"
        );
        stream
            .write_all(req.as_bytes())
            .await
            .context("write request")?;
        let mut buf = Vec::new();
        (&mut stream)
            .take(MAX_BODY_BYTES + 1)
            .read_to_end(&mut buf)
            .await
            .context("read response")?;
        if buf.len() as u64 > MAX_BODY_BYTES {
            anyhow::bail!("{key}: response over {MAX_BODY_BYTES} bytes");
        }
        Ok::<_, anyhow::Error>(buf)
    };
    tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut)
        .await
        .map_err(|_| anyhow::anyhow!("adminspace read of {key} timed out after {timeout_secs}s"))?
}

/// GET one key and return its body, refusing anything but a 200.
pub async fn get_body(addr: &str, key: &str, timeout_secs: u64) -> Result<Vec<u8>> {
    let raw = fetch_key(addr, key, timeout_secs).await?;
    let (status, body) = split_response(&raw)?;
    if status != 200 {
        anyhow::bail!("adminspace returned HTTP {status} for {key}");
    }
    Ok(body.to_vec())
}

/// One snapshot body: every snapshot key the policy allows, read
/// within one shared time budget and concatenated into one JSON array. The
/// note is empty when the route table was read, otherwise says why not.
pub struct SnapshotBody {
    pub body: Vec<u8>,
    pub route_table_note: String,
}

async fn get_entries(addr: &str, key: &str, timeout_secs: u64) -> Result<Vec<serde_json::Value>> {
    let body = get_body(addr, key, timeout_secs).await?;
    serde_json::from_slice(&body)
        .with_context(|| format!("{key}: body is not a JSON array of entries"))
}

/// What a link-state read says, with node numbering taken out: per region, the
/// set of member ZIDs and the set of links between them. Two reads that agree
/// on this agree on the routing map, whatever order the DOT text lists it in.
type MapShape = BTreeMap<String, (BTreeSet<String>, BTreeSet<(String, String)>, u32)>;

fn map_shape(entries: &[serde_json::Value]) -> MapShape {
    let mut out = MapShape::new();
    for e in entries {
        let key = e.get("key").and_then(|k| k.as_str()).unwrap_or_default();
        let Some((_, region)) = key.split_once("/linkstate/") else {
            continue;
        };
        let g = parse_dot(e.get("value").and_then(|v| v.as_str()).unwrap_or_default());
        let links = g
            .links
            .into_iter()
            .map(|l| if l.a <= l.b { (l.a, l.b) } else { (l.b, l.a) })
            .collect();
        out.insert(
            region.to_string(),
            (g.nodes.into_iter().collect(), links, g.edges),
        );
    }
    out
}

pub async fn fetch_snapshot(
    addr: &str,
    timeout_secs: u64,
    policy: RoutePolicy,
) -> Result<SnapshotBody> {
    let all = async {
        let mut out: Vec<serde_json::Value> = Vec::new();
        out.extend(get_entries(addr, ROUTER_KEY, timeout_secs).await?);
        let linkstate = get_entries(addr, LINKSTATE_KEY, timeout_secs).await?;
        let first = map_shape(&linkstate);
        out.extend(linkstate);
        out.extend(get_entries(addr, VERSION_KEY, timeout_secs).await?);

        let note = match policy.mode {
            RouteTable::Off => "not read: the publisher runs with --route-table off".to_string(),
            RouteTable::Always => String::new(),
            RouteTable::Settled => {
                tokio::time::sleep(std::time::Duration::from_millis(policy.settle_ms)).await;
                let again = map_shape(&get_entries(addr, LINKSTATE_KEY, timeout_secs).await?);
                if again == first {
                    String::new()
                } else {
                    format!(
                        "not read this cycle: the routing map changed within {} ms (a router \
                         joined or left), and a route-table read at that moment can crash the \
                         router (zenoh#2831); it is read again next cycle",
                        policy.settle_ms
                    )
                }
            }
        };
        if note.is_empty() {
            out.extend(get_entries(addr, ROUTE_TABLE_KEY, timeout_secs).await?);
        }
        Ok::<_, anyhow::Error>(SnapshotBody {
            body: serde_json::to_vec(&out)?,
            route_table_note: note,
        })
    };
    let budget = timeout_secs.saturating_add(policy.settle_ms.div_ceil(1000));
    tokio::time::timeout(std::time::Duration::from_secs(budget), all)
        .await
        .map_err(|_| anyhow::anyhow!("adminspace read timed out after {budget}s"))?
}

/// Split an HTTP/1.1 response into (status, body).
///
/// The adminspace answers `Connection: close` with an unchunked body, so the
/// body is simply everything after the header terminator. A response that is
/// chunked or that we cannot find a header terminator in is an error, not
/// something to guess at.
pub fn split_response(raw: &[u8]) -> Result<(u16, &[u8])> {
    let sep = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .context("no HTTP header terminator in adminspace response")?;
    let head = std::str::from_utf8(&raw[..sep]).context("non-utf8 HTTP headers")?;
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .context("no status code in adminspace response")?;
    if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        anyhow::bail!("adminspace returned a chunked body; this reader expects Connection: close");
    }
    Ok((status, &raw[sep + 4..]))
}

/// What one router says about itself, before the declared config is joined on.
#[derive(Debug)]
pub struct Observed {
    pub zid: String,
    pub zenoh_version: String,
    pub regions: Vec<RegionV1>,
    pub sessions: Vec<SessionV1>,
    pub route_successor_count: u32,
    pub next_hops: Vec<NextHopV1>,
    /// What this reader expected and did not find. Empty on the shape it was
    /// built against (zenoh 1.9.0). See [`TESTED_VERSION`].
    pub shape_notes: Vec<String>,
}

/// The zenoh version whose adminspace layout this reader was built and tested
/// against. The layout is not a stable interface; on any other version the
/// publisher still publishes what it can read and lists what it could not.
pub const TESTED_VERSION: &str = "1.9.0";

/// Whether a version string (`v1.9.0`, `1.9.0`, `v1.9.0-modified`) is the
/// tested release.
fn is_tested_version(v: &str) -> bool {
    let v = v.trim().trim_start_matches('v');
    v == TESTED_VERSION || v.starts_with(&format!("{TESTED_VERSION}-"))
}

/// Parse an `@/local/**` body.
///
/// Refuses a body carrying more than one router. That is the `@/**` fan-out
/// described in this module's header: labelling it would mean guessing which
/// router each key belonged to, and a per-router census that quietly became
/// fabric-wide is worse than one that stops.
pub fn parse(body: &[u8]) -> Result<Observed> {
    let entries: Vec<AdminEntry> =
        serde_json::from_slice(body).context("adminspace body is not a JSON array of entries")?;

    let mut zids: Vec<&str> = entries
        .iter()
        .filter_map(|e| e.key.strip_prefix("@/"))
        .filter_map(|rest| rest.split('/').next())
        .filter(|z| *z != "local")
        .collect();
    zids.sort_unstable();
    zids.dedup();
    match zids.len() {
        1 => {}
        0 => anyhow::bail!("adminspace response names no router"),
        n => anyhow::bail!(
            "adminspace response holds {n} routers, not 1 — the query was not local-scoped; \
             use @/local/** (see this module's header)"
        ),
    }
    let zid = zids[0].to_string();

    let router = entries
        .iter()
        .find(|e| e.key.ends_with("/router"))
        .context("no @/<zid>/router key in adminspace response")?;

    let zenoh_version = entries
        .iter()
        .find(|e| e.key.ends_with("plugins/rest/version"))
        .and_then(|e| e.value.as_str())
        .unwrap_or_default()
        .to_string();

    // What this reader expected and did not find. Each note says what is
    // missing and what the page therefore cannot show, so an unknown layout
    // reads as "not read", never as an empty router.
    let mut shape_notes = Vec::new();
    if zenoh_version.is_empty() {
        shape_notes.push(format!(
            "the router reported no version (`@/local/router/status/plugins/rest/version`); \
             this reader is tested with zenoh {TESTED_VERSION} only"
        ));
    } else if !is_tested_version(&zenoh_version) {
        shape_notes.push(format!(
            "zenoh {zenoh_version} is not the version this reader is tested with \
             ({TESTED_VERSION}); what it could read is shown, and anything it could not is \
             listed here"
        ));
    }
    match router.value.get("sessions") {
        Some(v) if v.is_array() => {}
        Some(_) => shape_notes.push(
            "the router entry's `sessions` is not a list, so its sessions, clients and the \
             addresses they come from are not shown"
                .into(),
        ),
        None => shape_notes.push(
            "the router entry has no `sessions` list, so its sessions, clients and the \
             addresses they come from are not shown"
                .into(),
        ),
    }

    // --- sessions, and the per-region session tally --------------------------
    let mut sessions = Vec::new();
    let mut per_region: BTreeMap<String, u32> = BTreeMap::new();
    if let Some(list) = router.value.get("sessions").and_then(|v| v.as_array()) {
        for s in list {
            let region = s.get("region").and_then(|v| v.as_str()).unwrap_or_default();
            *per_region.entry(region.to_string()).or_default() += 1;
            sessions.push(SessionV1 {
                peer_zid: s
                    .get("peer")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .into(),
                whatami: s
                    .get("whatami")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .into(),
                region: region.into(),
                link_dst: s
                    .get("links")
                    .and_then(|v| v.as_array())
                    .map(|ls| {
                        ls.iter()
                            .filter_map(|l| l.get("dst").and_then(|d| d.as_str()))
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default(),
            });
        }
    }

    // --- per-region link-state graphs ---------------------------------------
    let mut regions = Vec::new();
    for e in &entries {
        let Some((_, region)) = e.key.split_once("/linkstate/") else {
            continue;
        };
        let dot = e.value.as_str().unwrap_or_default();
        let g = parse_dot(dot);
        let links_truncated = g.links.len() > MAX_LINKS_PER_REGION;
        regions.push(RegionV1 {
            name: region.to_string(),
            link_state_v: g.nodes.len() as u32,
            link_state_e: g.edges,
            member_zids: g.nodes,
            session_count: per_region.get(region).copied().unwrap_or(0),
            links: g.links.into_iter().take(MAX_LINKS_PER_REGION).collect(),
            links_truncated,
        });
    }
    regions.sort_by(|a, b| a.name.cmp(&b.name));
    if regions.is_empty() {
        shape_notes.push(
            "no link-state graph was returned (`@/local/router/linkstate/*`), so this \
             router's regions and the routers it links to are not drawn from its report"
                .into(),
        );
    }
    let unparsed: Vec<&str> = entries
        .iter()
        .filter_map(|e| {
            let (_, region) = e.key.split_once("/linkstate/")?;
            let dot = e.value.as_str().unwrap_or_default();
            (!dot.trim_start().starts_with("graph")).then_some(region)
        })
        .collect();
    if !unparsed.is_empty() {
        shape_notes.push(format!(
            "the link-state value for {} is not a DOT `graph {{ ... }}`, so it is counted as \
             empty",
            unparsed.join(", ")
        ));
    }
    let unnamed = sessions.iter().filter(|s| s.region.is_empty()).count();
    if unnamed > 0 {
        shape_notes.push(format!(
            "{unnamed} session(s) carry no `region`, so they are not assigned to a region"
        ));
    }

    let successors: Vec<&AdminEntry> = entries
        .iter()
        .filter(|e| e.key.contains("/route/successor/"))
        .collect();
    let route_successor_count = successors.len() as u32;
    // Route entries per next hop: the value of `src/<a>/dst/<b>` is the
    // neighbour this router forwards a-to-b traffic to.
    let mut hops: BTreeMap<String, u32> = BTreeMap::new();
    for e in &successors {
        if let Some(z) = e.value.as_str().filter(|z| !z.is_empty() && *z != zid) {
            *hops.entry(z.to_string()).or_default() += 1;
        }
    }
    let next_hops = hops
        .into_iter()
        .map(|(zid, routes)| NextHopV1 { zid, routes })
        .collect();

    Ok(Observed {
        zid,
        zenoh_version,
        regions,
        sessions,
        route_successor_count,
        next_hops,
        shape_notes,
    })
}

/// One region's link-state graph, as the adminspace's DOT renders it.
struct Dot {
    /// Node labels — each a router's ZID.
    nodes: Vec<String>,
    /// Every `--` line, resolvable or not. This is the `E` the route-table
    /// invariant (T-005) is checked against, so it must not shrink when a line
    /// names an index we could not resolve.
    edges: u32,
    /// The edges whose two ends resolve to labels, as ZID pairs.
    links: Vec<LinkV1>,
}

/// Pull node labels, the edge count and the edge list out of the DOT graph the
/// adminspace publishes for a region:
///
/// ```text
/// graph {
///     0 [ label = "8b9a…" ]
///     1 [ label = "4b54…" ]
///     1 -- 0 [ label = "100.5" ]
/// }
/// ```
///
/// Deliberately tolerant: an unparseable line is skipped rather than failing
/// the whole snapshot, because a partial graph is still useful and a lost
/// snapshot is not.
fn parse_dot(dot: &str) -> Dot {
    let mut index: BTreeMap<&str, String> = BTreeMap::new();
    let mut nodes = Vec::new();
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    let mut edges = 0u32;
    for line in dot.lines() {
        let t = line.trim();
        if let Some((l, r)) = t.split_once("--") {
            edges += 1;
            let a = l.trim();
            let b = r.split_whitespace().next().unwrap_or_default();
            pairs.push((a, b));
        } else if let Some((head, rest)) = t.split_once("label = \"") {
            if let Some(end) = rest.find('"') {
                let label = rest[..end].to_string();
                let idx = head.trim().trim_end_matches('[').trim();
                index.insert(idx, label.clone());
                nodes.push(label);
            }
        }
    }
    let links = pairs
        .into_iter()
        .filter_map(|(a, b)| {
            let (a, b) = (index.get(a)?, index.get(b)?);
            Some(LinkV1 {
                a: a.clone(),
                b: b.clone(),
            })
        })
        .collect();
    Dot {
        nodes,
        edges,
        links,
    }
}

/// Join the observed state onto a snapshot that already carries identity,
/// freshness and the declared config.
pub fn apply(snap: &mut RouterTopologyV1, obs: Observed) {
    snap.zid = obs.zid;
    snap.zenoh_version = obs.zenoh_version;
    snap.regions = obs.regions;
    snap.sessions = obs.sessions;
    snap.route_successor_count = obs.route_successor_count;
    snap.next_hops = obs.next_hops;
    snap.shape_notes = obs.shape_notes;
    snap.observe_error.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped from a real `@/local/**` response: three
    /// routers in `north` as a star, thirteen clients terminated south.
    fn zagros_prod_body() -> String {
        serde_json::json!([
            {"key": "@/8b9a/router", "value": {
                "locators": ["tls/172.19.0.13:7447"],
                "sessions": [
                    {"peer": "4b54", "whatami": "router", "region": "north",
                     "links": [{"dst": "tls/100.127.0.102:38374"}]},
                    {"peer": "fd12", "whatami": "router", "region": "north",
                     "links": [{"dst": "tls/100.127.0.202:48524"}]},
                    {"peer": "7513", "whatami": "client", "region": "south:0:client",
                     "links": [{"dst": "tls/100.127.0.163:47048"}]}
                ]}},
            {"key": "@/8b9a/router/status/plugins/rest/version", "value": "v1.9.0-modified"},
            {"key": "@/8b9a/router/linkstate/north", "value":
                "graph {\n    0 [ label = \"8b9a\" ]\n    1 [ label = \"4b54\" ]\n    2 [ label = \"fd12\" ]\n    1 -- 0 [ label = \"100.5\" ]\n    2 -- 0 [ label = \"100.5\" ]\n}\n"},
            {"key": "@/8b9a/router/linkstate/south:0:peer", "value":
                "graph {\n    0 [ label = \"8b9a\" ]\n}\n"},
            {"key": "@/8b9a/router/route/successor/src/4b54/dst/fd12", "value": "fd12"},
            {"key": "@/8b9a/router/route/successor/src/8b9a/dst/fd12", "value": "fd12"},
            {"key": "@/8b9a/router/route/successor/src/fd12/dst/fd12", "value": "fd12"},
            {"key": "@/8b9a/router/route/successor/src/4b54/dst/4b54", "value": "4b54"},
            {"key": "@/8b9a/router/route/successor/src/8b9a/dst/4b54", "value": "4b54"},
            {"key": "@/8b9a/router/route/successor/src/fd12/dst/4b54", "value": "4b54"}
        ])
        .to_string()
    }

    #[test]
    fn parses_the_zagros_prod_shape() {
        let obs = parse(zagros_prod_body().as_bytes()).expect("parse");
        assert_eq!(obs.zid, "8b9a");
        assert_eq!(obs.zenoh_version, "v1.9.0-modified");
        assert_eq!(obs.sessions.len(), 3);

        let north = obs.regions.iter().find(|r| r.name == "north").unwrap();
        assert_eq!(north.link_state_v, 3, "north V");
        assert_eq!(north.link_state_e, 2, "north E — a star, not a full mesh");
        assert_eq!(north.session_count, 2, "two router sessions in north");

        // The next-hop invariant, on this fixture's numbers.
        assert_eq!(obs.route_successor_count, 6);
        assert_eq!(north.successor_terms(), 6);
        // Route entries per next hop: three destinations' worth each, through
        // the two neighbours; none names this router itself.
        let hops: Vec<(&str, u32)> = obs
            .next_hops
            .iter()
            .map(|h| (h.zid.as_str(), h.routes))
            .collect();
        assert_eq!(hops, vec![("4b54", 3), ("fd12", 3)]);
    }

    #[test]
    fn the_tested_shape_raises_no_note() {
        let obs = parse(zagros_prod_body().as_bytes()).expect("parse");
        assert!(obs.shape_notes.is_empty(), "{:?}", obs.shape_notes);
    }

    #[test]
    fn an_unknown_shape_is_reported_not_zeroed() {
        // A router of another version whose router entry has no sessions list
        // and that returns no link-state graph: the parse still succeeds, and
        // the notes say what is missing and why the page cannot show it.
        let body = serde_json::json!([
            {"key": "@/aaaa/router", "value": {"locators": [], "peers": []}},
            {"key": "@/aaaa/router/status/plugins/rest/version", "value": "v2.0.0"}
        ])
        .to_string();
        let obs = parse(body.as_bytes()).expect("an unknown shape is not an error");
        let n = obs.shape_notes.join(" | ");
        assert!(n.contains("zenoh v2.0.0 is not the version"), "{n}");
        assert!(n.contains("no `sessions` list"), "{n}");
        assert!(n.contains("no link-state graph"), "{n}");
    }

    #[test]
    fn version_strings_of_the_tested_release_are_recognized() {
        for v in ["v1.9.0", "1.9.0", "v1.9.0-modified"] {
            assert!(is_tested_version(v), "{v}");
        }
        for v in ["v1.9.1", "v1.10.0", "v1.9.00", "", "v1.8.0"] {
            assert!(!is_tested_version(v), "{v}");
        }
    }

    #[test]
    fn refuses_a_fabric_wide_response() {
        // Two routers in one body = the `@/**` fan-out. Stop, do not guess.
        let body = serde_json::json!([
            {"key": "@/aaaa/router", "value": {"sessions": []}},
            {"key": "@/bbbb/router", "value": {"sessions": []}}
        ])
        .to_string();
        let err = parse(body.as_bytes()).unwrap_err().to_string();
        assert!(err.contains("holds 2 routers"), "got: {err}");
        assert!(
            err.contains("@/local/**"),
            "the error should say the fix: {err}"
        );
    }

    #[test]
    fn splits_a_plain_response() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n[]";
        let (status, body) = split_response(raw).expect("split");
        assert_eq!(status, 200);
        assert_eq!(body, b"[]");
    }

    #[test]
    fn refuses_a_chunked_response_rather_than_mangling_it() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n[]\r\n0\r\n\r\n";
        assert!(split_response(raw)
            .unwrap_err()
            .to_string()
            .contains("chunked"));
    }

    #[test]
    fn dot_with_no_edges_reports_zero() {
        // A south:0:peer graph with three nodes and no edges. The
        // parser must not invent an edge from the node count.
        let g = parse_dot(
            "graph {\n    0 [ label = \"aa\" ]\n    1 [ label = \"bb\" ]\n    2 [ label = \"cc\" ]\n}\n",
        );
        assert_eq!(g.nodes.len(), 3);
        assert_eq!(g.edges, 0);
        assert!(g.links.is_empty());
    }

    #[test]
    fn dot_edges_resolve_to_zid_pairs_including_routers_two_hops_away() {
        // A chain a -- b -- c seen from a: c is NOT a session of a, and is known
        // only from this edge list. That is the whole reason links are carried.
        let g = parse_dot(
            "graph {\n    0 [ label = \"aa\" ]\n    1 [ label = \"bb\" ]\n    2 [ label = \"cc\" ]\n    1 -- 0 [ label = \"100\" ]\n    2 -- 1 [ label = \"100\" ]\n}\n",
        );
        assert_eq!(g.edges, 2);
        let pairs: Vec<(&str, &str)> = g
            .links
            .iter()
            .map(|l| (l.a.as_str(), l.b.as_str()))
            .collect();
        assert_eq!(pairs, vec![("bb", "aa"), ("cc", "bb")]);
    }

    #[test]
    fn an_unresolvable_edge_still_counts_toward_e() {
        // E feeds the route-table invariant; dropping an edge we could not name
        // would manufacture a false T-005.
        let g = parse_dot("graph {\n    0 [ label = \"aa\" ]\n    0 -- 7 [ label = \"1\" ]\n}\n");
        assert_eq!(g.edges, 1);
        assert!(g.links.is_empty());
    }

    fn ls(region: &str, dot: &str) -> serde_json::Value {
        serde_json::json!({"key": format!("@/aa/router/linkstate/{region}"), "value": dot})
    }

    #[test]
    fn the_routing_map_compares_by_shape_not_by_numbering() {
        // The same three routers and two links, numbered and ordered
        // differently: the same map, so `settled` reads the route table.
        let a = ls("north", "graph {\n 0 [ label = \"aa\" ]\n 1 [ label = \"bb\" ]\n 2 [ label = \"cc\" ]\n 1 -- 0 [ label = \"1\" ]\n 2 -- 1 [ label = \"1\" ]\n}\n");
        let b = ls("north", "graph {\n 0 [ label = \"cc\" ]\n 1 [ label = \"aa\" ]\n 2 [ label = \"bb\" ]\n 0 -- 2 [ label = \"1\" ]\n 2 -- 1 [ label = \"1\" ]\n}\n");
        assert_eq!(map_shape(std::slice::from_ref(&a)), map_shape(&[b]));
        // A router joined: a different map, so the read is skipped.
        let c = ls("north", "graph {\n 0 [ label = \"aa\" ]\n 1 [ label = \"bb\" ]\n 2 [ label = \"cc\" ]\n 3 [ label = \"dd\" ]\n 1 -- 0 [ label = \"1\" ]\n 2 -- 1 [ label = \"1\" ]\n 3 -- 0 [ label = \"1\" ]\n}\n");
        assert_ne!(map_shape(std::slice::from_ref(&a)), map_shape(&[c]));
        // A new region appearing is a change too.
        let d = ls("south:0:router", "graph {\n 0 [ label = \"aa\" ]\n}\n");
        assert_ne!(map_shape(std::slice::from_ref(&a)), map_shape(&[a, d]));
    }

    #[test]
    fn the_zagros_fixture_carries_its_links() {
        let obs = parse(zagros_prod_body().as_bytes()).expect("parse");
        let north = obs.regions.iter().find(|r| r.name == "north").unwrap();
        assert_eq!(north.links.len(), 2);
        assert!(!north.links_truncated);
        assert!(north.links.iter().all(|l| l.b == "8b9a"), "a star on 8b9a");
    }
}
