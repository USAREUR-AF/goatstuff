// SPDX-License-Identifier: Apache-2.0
//
// Schema resolver — maps topic key-expressions to a schema family name
// (e.g. "tak/cot/track/v1" → "cot"). The frontend
// colors edges by family.
//
// This is the compiled-in **family-map floor** per
// docs/design/panoscope.md §"Schema-color data source". Live, per-peer
// schema coloring comes from schema-declaration self-registration —
// `zenoh_client` subscribes `attest/schema/**` and writes each peer's
// declared family onto the topology — which *replaced* the original
// design's portal-driven schema-color path for the trial fabric. This
// floor only names the well-known prefix families when no schema-decl
// has arrived for a key.
//
// History: an earlier design fetched a bulk family list from the
// portal (`PortalSchemaResolver` → GET /api/schemas) and fell back to
// this map. The portal never served a bulk family endpoint — it serves
// per-key schemas at `/api/schemas/<key>`, Bearer-auth-gated — so that
// fetch could never succeed in production. It nonetheless ran every
// refresh interval, failed, and left the /api/legend resolver status
// pinned at a misleading "embedded-fallback" that reads as breakage. The
// dead fetch path (resolver + reqwest client + composite wrapper) was
// removed; the floor is now the resolver, honestly labelled `family-map`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Resolver status reported on /api/legend. The schema resolver is the
/// compiled-in family-map floor; live per-peer coloring is schema-decl
/// driven (see module docs), so there is a single honest state — never a
/// portal-reachability verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResolverStatus {
    /// Using the compiled-in family-map floor. NOT a failure mode —
    /// per-peer coloring rides schema-decl self-registration on top.
    FamilyMap,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaMapping {
    pub key_prefix: String,
    pub family: String,
}

/// Strip the trailing `/**` or `/*` from a key-prefix to get the
/// concrete prefix used for longest-match lookup.
fn normalize_prefix(p: &str) -> &str {
    p.trim_end_matches("/**").trim_end_matches("/*")
}

/// Find the schema family for a key by longest-prefix match against the
/// supplied mapping table. Returns `None` if no prefix matches.
pub fn longest_prefix_match(key: &str, mappings: &[SchemaMapping]) -> Option<String> {
    let mut best: Option<(&SchemaMapping, usize)> = None;
    for m in mappings {
        let prefix = normalize_prefix(&m.key_prefix);
        if key == prefix || key.starts_with(&format!("{prefix}/")) {
            if best.map(|(_, len)| prefix.len() > len).unwrap_or(true) {
                best = Some((m, prefix.len()));
            }
        }
    }
    best.map(|(m, _)| m.family.clone())
}

/// The compiled-in family-map floor: product namespaces only, the same on
/// every goatnet. A goatnet's own publishers' prefixes come from
/// PANOSCOPE_SCHEMA_FAMILIES. Matches docs/design/panoscope.md
/// §"Schema-color data source".
pub fn embedded_mappings() -> Vec<SchemaMapping> {
    vec![
        SchemaMapping {
            key_prefix: "tak/cot/**".into(),
            family: "cot".into(),
        },
        SchemaMapping {
            key_prefix: "community/**".into(),
            family: "community".into(),
        },
        SchemaMapping {
            key_prefix: "caracal/**".into(),
            family: "caracal".into(),
        },
        // Notional umbrella — post PR #225 all synth-vendor peers
        // publish under a single `notional/<peer>/<site>/<thing>/v1`
        // identity. The per-peer axis lives in the topic path, so this
        // mapping just gives the family a stable name in the floor.
        SchemaMapping {
            key_prefix: "notional/**".into(),
            family: "notional".into(),
        },
    ]
}

/// Trait abstraction so the data tail + tests can substitute resolvers.
pub trait SchemaResolver: Send + Sync {
    /// Returns the schema family name for `key`, or `None`.
    fn resolve(&self, key: &str) -> Option<String>;

    /// Current resolver status — surfaces on /api/legend.
    fn status(&self) -> ResolverStatus;

    /// Number of distinct schema families known to this resolver right
    /// now. Surfaces on /api/legend.
    fn known_family_count(&self) -> usize;
}

// ---------------------------------------------------------------------------
// FamilyMapResolver — the compiled-in family-map floor (the resolver).
// ---------------------------------------------------------------------------

pub struct FamilyMapResolver {
    mappings: Vec<SchemaMapping>,
}

/// Names the deployment's own family mappings: comma-separated
/// `<key-prefix>=<family>` entries, e.g. `org-a/**=lattice,org-b/**=ecn`.
pub const FAMILIES_ENV: &str = "PANOSCOPE_SCHEMA_FAMILIES";

/// Parse `PANOSCOPE_SCHEMA_FAMILIES`. Entries without both halves are skipped.
pub fn parse_families(v: &str) -> Vec<SchemaMapping> {
    v.split(',')
        .filter_map(|entry| {
            let (prefix, family) = entry.split_once('=')?;
            let (prefix, family) = (prefix.trim(), family.trim());
            (!prefix.is_empty() && !family.is_empty()).then(|| SchemaMapping {
                key_prefix: prefix.to_string(),
                family: family.to_string(),
            })
        })
        .collect()
}

impl FamilyMapResolver {
    /// The deployment's mappings (`PANOSCOPE_SCHEMA_FAMILIES`) ahead of the
    /// compiled-in floor; for the same prefix the deployment's family wins.
    pub fn new() -> Self {
        Self::with_families(&std::env::var(FAMILIES_ENV).unwrap_or_default())
    }

    /// As `new`, with the deployment's mappings given directly.
    pub fn with_families(families: &str) -> Self {
        let mut mappings = parse_families(families);
        mappings.extend(embedded_mappings());
        Self { mappings }
    }
}

impl Default for FamilyMapResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl SchemaResolver for FamilyMapResolver {
    fn resolve(&self, key: &str) -> Option<String> {
        longest_prefix_match(key, &self.mappings)
    }
    fn status(&self) -> ResolverStatus {
        ResolverStatus::FamilyMap
    }
    fn known_family_count(&self) -> usize {
        let mut families: HashMap<String, ()> = HashMap::new();
        for m in &self.mappings {
            families.insert(m.family.clone(), ());
        }
        families.len()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_map_resolver_known_prefixes() {
        let r = FamilyMapResolver::new();
        assert_eq!(r.resolve("notional/peer/site-3/thing/v1"), Some("notional".into()));
        assert_eq!(r.resolve("tak/cot/track/v1"), Some("cot".into()));
        assert_eq!(r.resolve("community/pat-test/foo"), Some("community".into()));
        assert_eq!(r.resolve("caracal/snitch/probe-report/v1"), Some("caracal".into()));
    }

    #[test]
    fn deployment_families_add_to_and_override_the_floor() {
        let r = FamilyMapResolver::with_families("org-a/**=lattice, tak/cot/**=mine,broken,=x");
        assert_eq!(r.resolve("org-a/sensor/track/v1"), Some("lattice".into()));
        assert_eq!(r.resolve("tak/cot/track/v1"), Some("mine".into()));
        assert_eq!(r.resolve("notional/peer/site-3/thing/v1"), Some("notional".into()));
        assert!(parse_families("broken,=x,y=").is_empty());
    }

    #[test]
    fn family_map_resolver_unknown_returns_none() {
        let r = FamilyMapResolver::new();
        assert_eq!(r.resolve("unknown-vendor/system/topic"), None);
        assert_eq!(r.resolve(""), None);
    }

    #[test]
    fn family_map_resolver_status_and_count() {
        let r = FamilyMapResolver::new();
        assert_eq!(r.status(), ResolverStatus::FamilyMap);
        assert_eq!(r.known_family_count(), 4);
    }

    #[test]
    fn longest_prefix_match_prefers_specific_over_general() {
        let mappings = vec![
            SchemaMapping {
                key_prefix: "vendor-b/**".into(),
                family: "family-generic".into(),
            },
            SchemaMapping {
                key_prefix: "vendor-b/sensorgrid/entity/**".into(),
                family: "family-entity".into(),
            },
        ];
        assert_eq!(
            longest_prefix_match("vendor-b/sensorgrid/entity/track/v1", &mappings),
            Some("family-entity".into())
        );
        assert_eq!(
            longest_prefix_match("vendor-b/other/topic", &mappings),
            Some("family-generic".into())
        );
    }

    #[test]
    fn longest_prefix_match_requires_path_boundary() {
        // "vendor-b" prefix should NOT match "vendor-bbar/foo"
        let mappings = vec![SchemaMapping {
            key_prefix: "vendor-b/**".into(),
            family: "family".into(),
        }];
        assert_eq!(longest_prefix_match("vendor-bbar/foo", &mappings), None);
        assert_eq!(
            longest_prefix_match("vendor-b/foo", &mappings),
            Some("family".into())
        );
    }

    #[test]
    fn normalize_prefix_strips_wildcards() {
        assert_eq!(normalize_prefix("vendor-b/**"), "vendor-b");
        assert_eq!(normalize_prefix("a/b/*"), "a/b");
        assert_eq!(normalize_prefix("plain/prefix"), "plain/prefix");
    }
}
