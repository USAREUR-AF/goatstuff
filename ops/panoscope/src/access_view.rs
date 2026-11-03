// SPDX-License-Identifier: Apache-2.0
//
// Access COP + data catalog view models (/api/access/*) — the ADR 1026
// "See it" web surface + the ADR 1075 catalog browse half, hosted as the
// panoscope `/access` page. Design: docs/design/access-cop-web.md.
//
// Gate tiers (enforce mode; off = pass-through like every other page):
//   catalog metadata (orgs, topic rows, formats)  → any authenticated
//     viewer — ADR 1028: topic visibility is advisory, not access;
//     discovery is the catalog's purpose.
//   live-rate overlay on catalog rows             → per-viewer
//     ViewerScope::allows_key — live rates are fabric data, not catalog
//     metadata. Out of scope ⇒ an honest `null` (rendered "—"), never 0.
//   cross-identity queries (roster/who/why/topic) → who_groups (handled
//     in main.rs with the same require_who gate as /who).
//
// Labels + grant edges (ADR 1075/1076) do not exist server-side yet; the
// page renders an explicit not-yet-declared state. Nothing here invents
// an edge or a lane.

use std::collections::HashMap;

use serde::Serialize;

use crate::model::FlowState;
use crate::viewer::ViewerScope;
use crate::zenoh_client::EMA_TAU_SECS;
use panoscope_core::alias::AliasRegistry;
use panoscope_core::portal::{Topic, Vendor};

/// Live projected msg/s per topic key, summed over publishers. The
/// handler builds this once per request from the flow state; rates are
/// then attributed to catalog rows by whole-segment org prefix (org
/// rollup) or keyexpr match (per-topic).
pub fn live_rates(flow: &FlowState, now_ms: u64) -> HashMap<String, f64> {
    let mut per_topic: HashMap<String, f64> = HashMap::new();
    for (key, edge) in &flow.edges {
        *per_topic.entry(key.topic.clone()).or_insert(0.0) +=
            edge.projected_eps(now_ms, EMA_TAU_SECS);
    }
    per_topic
}

#[derive(Serialize)]
pub struct CatalogOrg {
    pub prefix: String,
    /// Portal display name, falling back to the alias, then the prefix
    /// (truncated client-side) — never invented.
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    pub synthetic: bool,
    /// Live msg/s summed over this org's namespace — `null` when every
    /// key under the org is out of the viewer's scope (rendered "—");
    /// `0.0` means in-scope and genuinely quiet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_eps: Option<f64>,
    /// Live declared topic rows, verbatim from the portal. `None` = the
    /// portal did not say (predates the field); the page shows such rows.
    /// `Some(0)` = nothing to discover here; the page folds those by default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_topics: Option<i64>,
}

#[derive(Serialize)]
pub struct CatalogView {
    /// "ok" | "disabled" (portal unconfigured — catalog needs the portal).
    pub portal_state: &'static str,
    pub orgs: Vec<CatalogOrg>,
    /// True when ADR 1075 label rows exist server-side (never true yet;
    /// the frontend renders the explicit "no labels declared" state).
    pub labels_available: bool,
    pub generated_at_ms: u64,
}

#[derive(Serialize)]
pub struct CatalogTopic {
    pub id: i64,
    pub key_pattern: String,
    pub format: String,
    pub schema_ref: String,
    pub description: String,
    pub publish_count: i64,
    /// Live msg/s for keys matching this row's pattern — `null` when the
    /// pattern is out of the viewer's scope.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_eps: Option<f64>,
}

#[derive(Serialize)]
pub struct CatalogTopicsView {
    pub org: String,
    pub topics: Vec<CatalogTopic>,
    pub generated_at_ms: u64,
}

/// Assemble the org catalog. Pure over its inputs.
pub fn build_catalog(
    scope: &ViewerScope,
    vendors: &[Vendor],
    aliases: &AliasRegistry,
    rates: &HashMap<String, f64>,
    now_ms: u64,
) -> CatalogView {
    let orgs = vendors
        .iter()
        .map(|v| {
            let alias = aliases.alias_of(&v.prefix);
            let display_name = if !v.display_name.is_empty() {
                v.display_name.clone()
            } else if let Some(a) = &alias {
                a.clone()
            } else {
                v.prefix.clone()
            };
            // Org rollup: sum live rates over keys under the org prefix
            // (whole-segment), counting only keys within the viewer's
            // scope. If the org's namespace is entirely out of scope the
            // overlay is None — an honest dash, never a fake quiet zero.
            let probe_key = format!("{}/", v.prefix);
            let in_scope = scope.allows_key(&probe_key);
            let live_eps = in_scope.then(|| {
                rates
                    .iter()
                    .filter(|(k, _)| seg_prefixed(k, &v.prefix) && scope.allows_key(k))
                    .map(|(_, eps)| *eps)
                    .sum()
            });
            CatalogOrg {
                prefix: v.prefix.clone(),
                display_name,
                alias,
                synthetic: v.synthetic,
                live_eps,
                declared_topics: v.declared_topics,
            }
        })
        .collect();
    CatalogView {
        portal_state: "ok",
        orgs,
        labels_available: false,
        generated_at_ms: now_ms,
    }
}

/// Assemble one org's topic rows. Pure over its inputs.
pub fn build_catalog_topics(
    scope: &ViewerScope,
    org: &str,
    topics: &[Topic],
    rates: &HashMap<String, f64>,
    now_ms: u64,
) -> CatalogTopicsView {
    let topics = topics
        .iter()
        .map(|t| {
            let in_scope = scope.allows_key(&t.key_pattern);
            let live_eps = in_scope.then(|| {
                rates
                    .iter()
                    .filter(|(k, _)| {
                        crate::snapshot_view::keyexpr_matches(&t.key_pattern, k)
                            && scope.allows_key(k)
                    })
                    .map(|(_, eps)| *eps)
                    .sum()
            });
            CatalogTopic {
                id: t.id,
                key_pattern: t.key_pattern.clone(),
                format: t.format.clone(),
                schema_ref: t.schema_ref.clone(),
                description: t.description.clone(),
                publish_count: t.publish_count,
                live_eps,
            }
        })
        .collect();
    CatalogTopicsView { org: org.to_string(), topics, generated_at_ms: now_ms }
}

/// Whole-segment prefix (mirror of viewer.rs's private helper).
fn seg_prefixed(key: &str, anchor: &str) -> bool {
    key.starts_with(anchor) && key[anchor.len()..].starts_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn scoped(anchors: &[&str], include_notional: bool) -> ViewerScope {
        ViewerScope::Scoped {
            anchors: anchors.iter().map(|s| s.to_string()).collect::<HashSet<_>>(),
            include_notional,
        }
    }

    fn vendor(prefix: &str, name: &str, synthetic: bool) -> Vendor {
        Vendor {
            prefix: prefix.into(),
            display_name: name.into(),
            description: String::new(),
            synthetic,
            sponsor_authorized: false,
            sponsor_max_tier: String::new(),
            declared_topics: None,
        }
    }

    fn topic(pattern: &str) -> Topic {
        Topic {
            id: 1,
            vendor_prefix: pattern.split('/').next().unwrap_or("").to_string(),
            key_pattern: pattern.into(),
            format: "json".into(),
            schema_ref: String::new(),
            description: String::new(),
            publish_count: 3,
            // Added to `Topic` by the `goat flow` work (25189ced8); this test
            // helper was not updated, which broke `cargo test` for the whole
            // panoscope binary. `None` = "the audit tailer has not stamped a
            // last-publish for this topic", which is the field's normal state
            // today and the right default for a fixture about access scoping.
            last_seen_at: None,
            created_at: 0,
        }
    }

    #[test]
    fn catalog_metadata_visible_to_floor_but_rates_scoped() {
        // ADR 1028: a floor viewer browses the catalog (org names, rows)
        // — but the live overlay is null for out-of-scope namespaces.
        let vendors = vec![vendor("abc123", "Acme", false)];
        let mut rates = HashMap::new();
        rates.insert("abc123/telemetry/v1".to_string(), 5.0);
        let v = build_catalog(&scoped(&[], false), &vendors, &AliasRegistry::default(), &rates, 1);
        assert_eq!(v.orgs.len(), 1, "catalog metadata is advisory — visible");
        assert!(v.orgs[0].live_eps.is_none(), "out-of-scope rate is null, never 0");
        assert!(!v.labels_available, "no fabricated label lanes (ADR 1075 unbuilt)");
    }

    /// The declared-topic count rides through verbatim, including the
    /// "portal did not say" case: an old portal must not make every org look
    /// empty, so None stays None rather than collapsing to 0.
    #[test]
    fn declared_topic_count_passes_through_unknown_as_unknown() {
        let mut said = vendor("abc123", "Acme", false);
        said.declared_topics = Some(0);
        let unsaid = vendor("def456", "Beta", false);
        let v = build_catalog(
            &ViewerScope::Full,
            &[said, unsaid],
            &AliasRegistry::default(),
            &HashMap::new(),
            1,
        );
        assert_eq!(v.orgs[0].declared_topics, Some(0));
        assert_eq!(v.orgs[1].declared_topics, None);
    }

    #[test]
    fn granted_viewer_sees_live_rollup_and_full_sees_all() {
        let vendors = vec![vendor("abc123", "Acme", false)];
        let mut rates = HashMap::new();
        rates.insert("abc123/telemetry/v1".to_string(), 5.0);
        rates.insert("abc123/status/v1".to_string(), 1.0);
        rates.insert("other/t/v1".to_string(), 9.0);

        let v = build_catalog(&scoped(&["abc123"], false), &vendors, &AliasRegistry::default(), &rates, 1);
        assert_eq!(v.orgs[0].live_eps, Some(6.0), "own-org rollup sums only own keys");

        let v = build_catalog(&ViewerScope::Full, &vendors, &AliasRegistry::default(), &rates, 1);
        assert_eq!(v.orgs[0].live_eps, Some(6.0));
    }

    #[test]
    fn topic_rows_join_rates_by_keyexpr_and_stay_scoped() {
        let topics = vec![topic("abc123/telemetry/*")];
        let mut rates = HashMap::new();
        rates.insert("abc123/telemetry/v1".to_string(), 4.0);
        rates.insert("abc123/other/v1".to_string(), 2.0);

        let v = build_catalog_topics(&ViewerScope::Full, "abc123", &topics, &rates, 1);
        assert_eq!(v.topics[0].live_eps, Some(4.0), "wildcard pattern matches its keys only");

        let v = build_catalog_topics(&scoped(&[], false), "abc123", &topics, &rates, 1);
        assert_eq!(v.topics.len(), 1, "row metadata visible (advisory)");
        assert!(v.topics[0].live_eps.is_none(), "rate withheld at floor");
    }

    #[test]
    fn display_name_falls_back_alias_then_prefix() {
        let vendors = vec![vendor("2cf6aaaa", "", false)];
        let v = build_catalog(
            &ViewerScope::Full,
            &vendors,
            &AliasRegistry::default(),
            &HashMap::new(),
            1,
        );
        assert_eq!(v.orgs[0].display_name, "2cf6aaaa", "no invented names");
    }
}
