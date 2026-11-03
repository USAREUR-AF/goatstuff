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

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::wire::{RegionV1, RouterTopologyV1, SessionV1};

/// One `{key, value}` pair as the REST plugin renders the adminspace.
#[derive(Debug, Deserialize)]
struct AdminEntry {
    key: String,
    value: serde_json::Value,
}

/// Fetch `@/local/**` over plaintext loopback HTTP.
///
/// Hand-rolled rather than pulling an HTTP client crate: the request is one
/// fixed GET to 127.0.0.1 with no TLS, no redirects, no auth and no
/// content-negotiation. A client dependency would add supply-chain surface to
/// the image for thirty lines of work.
pub async fn fetch(addr: &str, timeout_secs: u64) -> Result<Vec<u8>> {
    let fut = async {
        let mut stream = TcpStream::connect(addr)
            .await
            .with_context(|| format!("connect adminspace {addr}"))?;
        let req = format!(
            "GET /@/local/** HTTP/1.1\r\nHost: {addr}\r\nAccept: application/json\r\n\
             User-Agent: goat-topology\r\nConnection: close\r\n\r\n"
        );
        stream
            .write_all(req.as_bytes())
            .await
            .context("write request")?;
        let mut buf = Vec::new();
        stream
            .read_to_end(&mut buf)
            .await
            .context("read response")?;
        Ok::<_, anyhow::Error>(buf)
    };
    tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut)
        .await
        .map_err(|_| anyhow::anyhow!("adminspace read timed out after {timeout_secs}s"))?
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
        let (members, edges) = parse_dot(dot);
        regions.push(RegionV1 {
            name: region.to_string(),
            link_state_v: members.len() as u32,
            link_state_e: edges,
            member_zids: members,
            session_count: per_region.get(region).copied().unwrap_or(0),
        });
    }
    regions.sort_by(|a, b| a.name.cmp(&b.name));

    let route_successor_count = entries
        .iter()
        .filter(|e| e.key.contains("/route/successor/"))
        .count() as u32;

    Ok(Observed {
        zid,
        zenoh_version,
        regions,
        sessions,
        route_successor_count,
    })
}

/// Pull node labels and edge count out of the DOT graph the adminspace
/// publishes for a region. Deliberately tolerant: an unparseable line is
/// skipped rather than failing the whole snapshot, because a partial graph is
/// still useful and a lost snapshot is not.
fn parse_dot(dot: &str) -> (Vec<String>, u32) {
    let mut nodes = Vec::new();
    let mut edges = 0u32;
    for line in dot.lines() {
        let t = line.trim();
        if t.contains("--") {
            edges += 1;
        } else if let Some(rest) = t.split_once("label = \"") {
            if let Some(end) = rest.1.find('"') {
                nodes.push(rest.1[..end].to_string());
            }
        }
    }
    (nodes, edges)
}

/// Join the observed state onto a snapshot that already carries identity,
/// freshness and the declared config.
pub fn apply(snap: &mut RouterTopologyV1, obs: Observed) {
    snap.zid = obs.zid;
    snap.zenoh_version = obs.zenoh_version;
    snap.regions = obs.regions;
    snap.sessions = obs.sessions;
    snap.route_successor_count = obs.route_successor_count;
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
                     "links": [{"dst": "tls/100.127.0.99:38374"}]},
                    {"peer": "fd12", "whatami": "router", "region": "north",
                     "links": [{"dst": "tls/100.127.0.199:48524"}]},
                    {"peer": "7513", "whatami": "client", "region": "south:0:client",
                     "links": [{"dst": "tls/100.127.0.160:47048"}]}
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
        let (nodes, edges) = parse_dot(
            "graph {\n    0 [ label = \"aa\" ]\n    1 [ label = \"bb\" ]\n    2 [ label = \"cc\" ]\n}\n",
        );
        assert_eq!(nodes.len(), 3);
        assert_eq!(edges, 0);
    }
}
