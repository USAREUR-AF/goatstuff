// SPDX-License-Identifier: Apache-2.0
//
// GET /api/directory — the navigate-anywhere catalog. Lets an operator
// who doesn't already know a key/vendor browse what exists and jump
// straight to a focused view, instead of having to hand-type a Zenoh
// key into the URL bar (docs/design/flashlight.md §"Discovery +
// navigation"). Backs both the empty-focus directory landing and the
// always-present header jump box.
//
// Source of truth is the portal topic registry, reached via the shared
// `panoscope_core::portal::PortalClient` (same substrate panoscope and
// stethoscope use). Vendors → topics. Role-based grouping
// (sources/sinks/derivers) is a follow-on that needs the live topology
// hydrated inside flashlight; it is intentionally NOT here yet.
//
// When the portal is unconfigured (`FLASHLIGHT_PORTAL_URL` /
// `FLASHLIGHT_PORTAL_API_KEY` empty) the endpoint returns
// `{ "enabled": false, "vendors": [] }` so the UI can say "portal not
// configured — type a key" rather than spin.

use axum::{extract::State, response::Json};
use futures::future::join_all;
use serde::Serialize;
use tracing::{info, warn};

use crate::AppState;

#[derive(Debug, Serialize)]
pub struct Directory {
    /// False when no portal is configured at this deployment.
    pub enabled: bool,
    pub vendors: Vec<VendorEntry>,
    /// Peers observed via schema-declarations, for role-based browse
    /// (Sources / Sinks / Derivers). Empty until the first decl lands
    /// (no live fabric / portal-only deployments). Independent of the
    /// portal `enabled` flag — topology comes from the Zenoh tap.
    pub peers: Vec<PeerEntry>,
}

#[derive(Debug, Serialize)]
pub struct PeerEntry {
    pub peer_id: String,
    pub display_name: String,
    pub vendor: String,
    pub site: String,
    /// Computed role — `source` / `sink` / `deriver` / `unknown`.
    pub role: String,
    /// Declared role_class (e.g. `sensor`, `c2`), when the schema-decl
    /// carried one.
    pub role_class: String,
    /// Key expressions this peer publishes (sources/derivers) or, for a
    /// pure sink, subscribes to — the keys the UI offers as one-click
    /// `/?key=…` tails.
    pub topics: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct VendorEntry {
    pub prefix: String,
    pub display_name: String,
    pub topic_count: usize,
    pub topics: Vec<TopicEntry>,
}

#[derive(Debug, Serialize)]
pub struct TopicEntry {
    /// Zenoh key expression — the value the UI drops into `/?key=…`.
    pub key_pattern: String,
    pub format: String,
    pub description: String,
    pub publish_count: i64,
}

/// Snapshot the hydrated topology into peer entries for role-based
/// browse. Pure read of the shared topology; sorted for stable display.
fn peers_from_topology(topology: &std::sync::RwLock<panoscope_core::topology::Topology>) -> Vec<PeerEntry> {
    use panoscope_core::types::Role;
    let topo = match topology.read() {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut peers: Vec<PeerEntry> = topo
        .peers
        .values()
        .map(|p| {
            let role = match p.role() {
                Role::Source => "source",
                Role::Sink => "sink",
                Role::Deriver => "deriver",
                Role::Unknown => "unknown",
            };
            // Pure sinks have no publishes — offer their subscribe keys
            // (the traffic they receive) as the tail target instead.
            let mut topics: Vec<String> = if p.publishes.is_empty() {
                p.subscribes.iter().cloned().collect()
            } else {
                p.publishes.iter().cloned().collect()
            };
            topics.sort();
            PeerEntry {
                peer_id: p.peer_id.clone(),
                display_name: p.display_name.clone().unwrap_or_default(),
                vendor: p.vendor.clone().unwrap_or_default(),
                site: p.site.clone().unwrap_or_default(),
                role: role.to_string(),
                role_class: p.role_class.clone().unwrap_or_default(),
                topics,
            }
        })
        .collect();
    peers.sort_by(|a, b| a.peer_id.cmp(&b.peer_id));
    peers
}

/// `GET /api/directory` — vendors + topics from the portal, plus peers
/// (role-grouped) from the schema-decl-hydrated topology.
pub async fn directory_handler(State(state): State<AppState>) -> Json<Directory> {
    let peers = peers_from_topology(&state.topology);

    if !state.portal.is_enabled() {
        return Json(Directory {
            enabled: false,
            vendors: Vec::new(),
            peers,
        });
    }

    // ONE request for every org and its topics, where the portal serves it.
    //
    // A list_vendors call followed by list_topics ONCE PER ORG, all concurrently
    // via join_all, sends hundreds of simultaneous requests, and the portal's
    // shared per-source rate limit refuses most of them. That leaves this
    // directory SPARSE rather than empty — the worst failure shape, because a
    // partial catalogue looks like a complete one. No individual call is
    // unreasonable; the shape of the call is.
    //
    // `catalog()` returns Ok(None) on 404, and then the per-org walk below
    // still runs: not every goatnet's portal serves /api/catalog yet, and
    // hard-failing against an older portal would be a worse trade than being
    // briefly chattier against it.
    let mut by_prefix: std::collections::HashMap<String, Vec<TopicEntry>> =
        std::collections::HashMap::new();
    let mut vendors: Vec<panoscope_core::portal::Vendor> = Vec::new();
    let mut have_catalog = false;

    match state.portal.catalog().await {
        Ok(Some(cat)) => {
            have_catalog = true;
            for o in cat.orgs {
                let mut entries: Vec<TopicEntry> = o
                    .topics
                    .into_iter()
                    .map(|t| TopicEntry {
                        key_pattern: t.key_pattern,
                        format: t.format,
                        description: t.description,
                        publish_count: t.publish_count,
                    })
                    .collect();
                entries.sort_by(|a, b| a.key_pattern.cmp(&b.key_pattern));
                by_prefix.insert(o.prefix.clone(), entries);
                vendors.push(panoscope_core::portal::Vendor {
                    prefix: o.prefix,
                    display_name: o.display_name,
                    description: String::new(),
                    synthetic: o.synthetic,
                    sponsor_authorized: false,
                    sponsor_max_tier: String::new(),
                    declared_topics: None,
                });
            }
        }
        Ok(None) => {
            info!("directory: portal has no /api/catalog; falling back to the per-org walk");
        }
        Err(e) => {
            warn!(error = %e, "directory: catalog failed; falling back to the per-org walk");
        }
    }

    if !have_catalog {
        vendors = match state.portal.list_vendors().await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "directory: list_vendors failed");
                return Json(Directory {
                    enabled: true,
                    vendors: Vec::new(),
                    peers,
                });
            }
        };

        // Fetch each vendor's topics concurrently — the portal client caps
        // each call at a 5 s timeout, so the slowest vendor bounds latency
        // rather than the sum.
        let topic_futures = vendors.iter().map(|v| {
            let portal = state.portal.clone();
            let prefix = v.prefix.clone();
            async move { (prefix.clone(), portal.list_topics(&prefix).await) }
        });
        let results = join_all(topic_futures).await;

        for (prefix, res) in results {
            match res {
                Ok(topics) => {
                    let mut entries: Vec<TopicEntry> = topics
                        .into_iter()
                        .map(|t| TopicEntry {
                            key_pattern: t.key_pattern,
                            format: t.format,
                            description: t.description,
                            publish_count: t.publish_count,
                        })
                        .collect();
                    // Stable display order: by key.
                    entries.sort_by(|a, b| a.key_pattern.cmp(&b.key_pattern));
                    by_prefix.insert(prefix, entries);
                }
                Err(e) => {
                    warn!(vendor = %prefix, error = %e, "directory: list_topics failed");
                    by_prefix.insert(prefix, Vec::new());
                }
            }
        }
    }

    let mut entries: Vec<VendorEntry> = vendors
        .into_iter()
        .map(|v| {
            let topics = by_prefix.remove(&v.prefix).unwrap_or_default();
            let display_name = if v.display_name.is_empty() {
                v.prefix.clone()
            } else {
                v.display_name
            };
            VendorEntry {
                topic_count: topics.len(),
                prefix: v.prefix,
                display_name,
                topics,
            }
        })
        .collect();
    entries.sort_by(|a, b| a.prefix.cmp(&b.prefix));

    Json(Directory {
        enabled: true,
        vendors: entries,
        peers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SampleStore;
    use std::sync::Arc;

    #[tokio::test]
    async fn disabled_portal_reports_not_enabled() {
        // Empty creds → disabled PortalClient → directory short-circuits
        // to `enabled: false` with no network call.
        let state = AppState {
            events: Arc::new(std::sync::RwLock::new(None)),
            zenoh_configured: false,
            store: Arc::new(SampleStore::new(8, 4096)),
            portal: panoscope_core::portal::PortalClient::new(String::new(), String::new()),
            topology: Arc::new(std::sync::RwLock::new(
                panoscope_core::topology::Topology::default(),
            )),
            descriptor_cache: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        };
        let Json(dir) = directory_handler(State(state)).await;
        assert!(!dir.enabled);
        assert!(dir.vendors.is_empty());
        assert!(dir.peers.is_empty());
    }

    #[test]
    fn peers_from_topology_groups_by_role() {
        use panoscope_core::topology::Topology;
        let topo = std::sync::RwLock::new(Topology::default());
        {
            let mut t = topo.write().unwrap();
            // Source: publishes only.
            t.record_publish_declaration("src".into(), "v/src/track/v1".into(), 1);
            t.peers.get_mut("src").unwrap().role_class = Some("sensor".into());
            // Sink: subscribes only.
            t.record_subscribe_declaration("snk".into(), "v/src/track/v1".into(), 1);
            // Deriver: publishes + subscribes a related key.
            t.record_subscribe_declaration("drv".into(), "v/src/track/v1".into(), 1);
            t.record_publish_declaration("drv".into(), "v/src/track/derived/v1".into(), 1);
        }
        let peers = peers_from_topology(&topo);
        let role = |id: &str| peers.iter().find(|p| p.peer_id == id).map(|p| p.role.as_str());
        assert_eq!(role("src"), Some("source"));
        assert_eq!(role("snk"), Some("sink"));
        assert_eq!(role("drv"), Some("deriver"));
        // Source carries its declared role_class + publish topic.
        let src = peers.iter().find(|p| p.peer_id == "src").unwrap();
        assert_eq!(src.role_class, "sensor");
        assert_eq!(src.topics, vec!["v/src/track/v1".to_string()]);
        // Pure sink offers its subscribe key as the tail target.
        let snk = peers.iter().find(|p| p.peer_id == "snk").unwrap();
        assert_eq!(snk.topics, vec!["v/src/track/v1".to_string()]);
    }
}
