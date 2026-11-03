// SPDX-License-Identifier: Apache-2.0
//
// Schema-declaration tap → local topology hydration.
//
// flashlight's broad `**` subscriber already sees every synth-vendor
// schema declaration (published on `attest/schema/<vendor>/<system>`
// keys, JSON payload, ~30s cadence). This module parses those decls
// into a `panoscope_core::topology::Topology` so flashlight can resolve
// a sample's key → its publisher's declared `role_class` / `site` /
// `format` — which is what the `?role=` / `?site=` / `?format=` URL
// post-filters need (sse.rs applies them).
//
// The field semantics here MIRROR panoscope's `apply_schema_declaration`
// (ops/panoscope/src/zenoh_client.rs) deliberately: same key
// prefix, same staleness gate, same JSON field names, so the two
// surfaces classify a peer identically. The schema-decl JSON shape is
// the stable published contract; if it ever moves, both sides update
// together. (A future cleanup may factor this into panoscope-core so
// there is one copy — tracked with the rest of Phase F.)

use panoscope_core::topology::Topology;

/// Schema-decl staleness ceiling. Synth-vendors re-publish every ~30s;
/// anything older than 3× that without a refresh is almost certainly
/// rocksdb-replayed residue from a defunct namespace. Matches
/// panoscope's `SCHEMA_DECL_STALE_MS`.
pub const SCHEMA_DECL_STALE_MS: u64 = 90_000;

/// Key prefix synth-vendor schema declarations are published under.
pub const SCHEMA_DECL_PREFIX: &str = "attest/schema/";

/// True when `key` is a schema-declaration key.
pub fn is_schema_decl_key(key: &str) -> bool {
    key.starts_with(SCHEMA_DECL_PREFIX)
}

/// If `key` is a schema-decl and `bytes` parse as the expected JSON,
/// apply it into `topology`. Non-decl keys and unparseable payloads are
/// ignored (returns `false`). Returns `true` when a decl was applied.
pub fn ingest_if_schema_decl(
    topology: &mut Topology,
    key: &str,
    bytes: &[u8],
    now_ms: u64,
) -> bool {
    if !is_schema_decl_key(key) {
        return false;
    }
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return false;
    };
    apply_schema_declaration(topology, key, &json, now_ms)
}

/// Apply a parsed schema-declaration JSON into the topology. Returns
/// `false` (no-op) when the decl is stale or carries no resolvable
/// peer id. Mirrors panoscope's field handling.
pub fn apply_schema_declaration(
    topology: &mut Topology,
    key: &str,
    json: &serde_json::Value,
    now_ms: u64,
) -> bool {
    // Staleness gate — drop storage-replayed residue. Decls without
    // `published_at_ms` pass (older synth-vendor versions / partners).
    if let Some(pub_at) = json.get("published_at_ms").and_then(|v| v.as_u64()) {
        if now_ms > pub_at && now_ms - pub_at > SCHEMA_DECL_STALE_MS {
            return false;
        }
    }

    // peer_id: prefer json.peer_id, else derive from the key path
    // (attest/schema/<vendor>/<system>/... → "<vendor>/<system>").
    let peer_id = json
        .get("peer_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            let parts: Vec<&str> = key.strip_prefix(SCHEMA_DECL_PREFIX)?.split('/').collect();
            if parts.len() >= 2 {
                Some(format!("{}/{}", parts[0], parts[1]))
            } else {
                None
            }
        });
    let Some(peer_id) = peer_id else { return false };

    {
        let p = topology.upsert_peer(peer_id.clone(), now_ms);
        if let Some(v) = json.get("vendor").and_then(|v| v.as_str()) {
            if !v.is_empty() {
                p.vendor = Some(v.to_string());
            }
        }
        if let Some(dn) = json.get("display_name").and_then(|v| v.as_str()) {
            if !dn.is_empty() {
                p.display_name = Some(dn.to_string());
            }
        }
        if let Some(s) = json.get("site").and_then(|v| v.as_str()) {
            if !s.is_empty() {
                p.site = Some(s.to_string());
            }
        }
        if let Some(rc) = json.get("role_class").and_then(|v| v.as_str()) {
            if !rc.is_empty() {
                p.role_class = Some(rc.to_string());
            }
        }
        if let Some(fmt) = json.get("format").and_then(|v| v.as_str()) {
            if !fmt.is_empty() {
                p.format_hint = Some(fmt.to_string());
            }
        }
        if let Some(syn) = json.get("synthetic").and_then(|v| v.as_bool()) {
            p.synthetic = Some(syn);
        }
        p.attest_source = Some("schema_decl".to_string());
    }

    if let Some(arr) = json.get("publishes").and_then(|v| v.as_array()) {
        for v in arr {
            if let Some(ke) = v.as_str() {
                topology.record_publish_declaration(peer_id.clone(), ke.to_string(), now_ms);
            }
        }
    }
    if let Some(arr) = json.get("subscribes").and_then(|v| v.as_array()) {
        for v in arr {
            if let Some(ke) = v.as_str() {
                topology.record_subscribe_declaration(peer_id.clone(), ke.to_string(), now_ms);
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(extra: serde_json::Value) -> serde_json::Value {
        extra
    }

    #[test]
    fn detects_schema_decl_keys() {
        assert!(is_schema_decl_key("attest/schema/vendor-b/sensorgrid/v1"));
        assert!(!is_schema_decl_key("vendor-b/sensorgrid/track/v1"));
    }

    #[test]
    fn applies_role_site_format_and_publishes() {
        let mut t = Topology::default();
        let json = decl(serde_json::json!({
            "peer_id": "vendor-b/sensorgrid",
            "site": "alpha-fob",
            "role_class": "sensor",
            "format": "protobuf:Track",
            "publishes": ["vendor-b/sensorgrid/track/v1", "vendor-b/sensorgrid/raw/v1"],
            "subscribes": ["c2/orders/v1"],
        }));
        assert!(apply_schema_declaration(
            &mut t,
            "attest/schema/vendor-b/sensorgrid/v1",
            &json,
            1_000
        ));
        let p = t.peers.get("vendor-b/sensorgrid").expect("peer hydrated");
        assert_eq!(p.site.as_deref(), Some("alpha-fob"));
        assert_eq!(p.role_class.as_deref(), Some("sensor"));
        assert_eq!(p.format_hint.as_deref(), Some("protobuf:Track"));
        assert_eq!(p.publishes.len(), 2);
        assert_eq!(p.subscribes.len(), 1);
    }

    #[test]
    fn derives_peer_id_from_key_when_absent() {
        let mut t = Topology::default();
        let json = decl(serde_json::json!({ "role_class": "effector" }));
        assert!(apply_schema_declaration(
            &mut t,
            "attest/schema/mongoose/eff/v1",
            &json,
            1_000
        ));
        assert!(t.peers.contains_key("mongoose/eff"));
    }

    #[test]
    fn stale_decl_is_dropped() {
        let mut t = Topology::default();
        let json = decl(serde_json::json!({
            "peer_id": "old/peer",
            "role_class": "sensor",
            "published_at_ms": 1_000u64,
        }));
        // now = 1000 + 90001 → just past the ceiling.
        assert!(!apply_schema_declaration(
            &mut t,
            "attest/schema/old/peer/v1",
            &json,
            91_002
        ));
        assert!(t.peers.is_empty());
    }

    #[test]
    fn fresh_decl_with_published_at_passes() {
        let mut t = Topology::default();
        let json = decl(serde_json::json!({
            "peer_id": "live/peer",
            "role_class": "c2",
            "published_at_ms": 100_000u64,
        }));
        assert!(apply_schema_declaration(
            &mut t,
            "attest/schema/live/peer/v1",
            &json,
            130_000
        ));
        assert!(t.peers.contains_key("live/peer"));
    }

    #[test]
    fn ingest_ignores_non_decl_and_bad_json() {
        let mut t = Topology::default();
        assert!(!ingest_if_schema_decl(
            &mut t,
            "vendor-b/sensorgrid/track/v1",
            b"{}",
            1_000
        ));
        assert!(!ingest_if_schema_decl(
            &mut t,
            "attest/schema/x/y/v1",
            b"not json",
            1_000
        ));
        assert!(t.peers.is_empty());
    }

    #[test]
    fn ingest_applies_valid_decl_bytes() {
        let mut t = Topology::default();
        let bytes = br#"{"peer_id":"v/s","role_class":"sensor","publishes":["v/s/x/v1"]}"#;
        assert!(ingest_if_schema_decl(
            &mut t,
            "attest/schema/v/s/v1",
            bytes,
            1_000
        ));
        assert_eq!(
            t.peers.get("v/s").unwrap().role_class.as_deref(),
            Some("sensor")
        );
    }
}
