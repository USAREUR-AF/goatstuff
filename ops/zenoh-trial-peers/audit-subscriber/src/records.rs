//! Audit metadata and replay bodies have separate serialization types.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize)]
pub struct AuditEvent {
    pub schema_version: u8,
    pub sample_kind: String,
    pub receiving_site: String,
    pub outcome: String,
    #[serde(flatten)]
    pub attribution: crate::attribution::Attribution,
    pub event_id: String,
    pub anchor: String,
    pub vendor: String,
    pub identity_model: String,
    pub key_expr: String,
    pub ts: String,
    /// Publisher-supplied label, never an authenticated actor.
    pub src_zid: String,
    pub source_provenance: &'static str,
    pub payload_hash: String,
    pub payload_len: usize,
    pub payload_capture: &'static str,
    /// Derived metadata preserves the existing onboarding fingerprint.
    pub onboarding_classification: &'static str,
    pub audit_subscriber_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReplayEvent {
    #[serde(flatten)]
    pub audit: AuditEvent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_bytes_b64: Option<String>,
}

pub struct Capture<'a> {
    pub sample_kind: &'a str,
    pub receiving_site: &'a str,
    pub policy: &'a crate::attribution::Policy,
    pub subscriber: &'a str,
    pub identity_model: &'a str,
    pub cap: usize,
    pub replay: bool,
}

pub fn record(
    key: &str,
    bytes: &[u8],
    source: Option<String>,
    ts: String,
    capture: Capture<'_>,
) -> ReplayEvent {
    let Capture {
        sample_kind,
        receiving_site,
        policy,
        subscriber,
        identity_model,
        cap,
        replay,
    } = capture;
    let anchor = key.split('/').next().unwrap_or_default().to_string();
    let captured = replay && bytes.len() <= cap;
    let src_zid = source.unwrap_or_default();
    let source_provenance = if src_zid.is_empty() {
        "unavailable"
    } else {
        "publisher_asserted"
    };
    ReplayEvent {
        audit: AuditEvent {
            schema_version: 2,
            sample_kind: sample_kind.into(),
            receiving_site: receiving_site.into(),
            outcome: "observed".into(),
            attribution: policy.attribute(key),
            event_id: String::new(), // assigned once when the batch is allocated
            anchor: anchor.clone(),
            vendor: anchor,
            identity_model: identity_model.to_string(),
            key_expr: key.to_string(),
            ts,
            src_zid,
            source_provenance,
            payload_hash: format!("sha256:{}", hex::encode(Sha256::digest(bytes))),
            payload_len: bytes.len(),
            payload_capture: if captured {
                "captured"
            } else if replay {
                "oversize"
            } else {
                "disabled"
            },
            // The legacy classifier could only inspect inlined bodies.
            onboarding_classification: if sample_kind == "put"
                && bytes.len() <= cap
                && is_hello(key, bytes)
            {
                "onboarding_hello"
            } else {
                "other"
            },
            audit_subscriber_id: subscriber.to_string(),
        },
        payload_bytes_b64: captured.then(|| STANDARD.encode(bytes)),
    }
}

/// Matches the legacy Go key/prefix/ASCII-regex fingerprint exactly.
pub fn is_hello(key: &str, bytes: &[u8]) -> bool {
    let Some((prefix, _)) = key.split_once('/') else {
        return false;
    };
    if prefix.is_empty()
        || key != format!("{prefix}/hello/v1")
        || !prefix
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return false;
    }
    let Ok(body) = std::str::from_utf8(bytes) else {
        return false;
    };
    let start = format!("hello from {prefix} at ");
    let Some(stamp) = body.trim().strip_prefix(&start) else {
        return false;
    };
    let Some((whole, fraction)) = stamp.split_once('.') else {
        return false;
    };
    !whole.is_empty()
        && !fraction.is_empty()
        && whole.bytes().all(|c| c.is_ascii_digit())
        && fraction.bytes().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_never_contains_body_and_capture_boundary_is_explicit() {
        for (size, capture) in [(0, "captured"), (16384, "captured"), (16385, "oversize")] {
            let body = vec![b'x'; size];
            let ev = record(
                "vendor/topic",
                &body,
                Some("claimed".into()),
                "now".into(),
                Capture {
                    sample_kind: "put",
                    receiving_site: "fixture",
                    policy: &crate::attribution::Policy::default(),
                    subscriber: "site",
                    identity_model: "per-vendor",
                    cap: 16384,
                    replay: true,
                },
            );
            let meta = serde_json::to_value(&ev.audit).unwrap();
            assert_eq!(ev.audit.payload_len, size);
            if size == 0 {
                assert_eq!(ev.audit.payload_hash, "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
            }
            assert!(meta.get("payload_bytes_b64").is_none());
            assert_eq!(meta["payload_capture"], capture);
            assert_eq!(ev.payload_bytes_b64.is_some(), size <= 16384);
            assert_eq!(ev.audit.source_provenance, "publisher_asserted");
        }
        let ev = record(
            "from/foreign/topic",
            b"private",
            None,
            "now".into(),
            Capture {
                sample_kind: "put",
                receiving_site: "fixture",
                policy: &crate::attribution::Policy::default(),
                subscriber: "site",
                identity_model: "per-vendor",
                cap: 16384,
                replay: false,
            },
        );
        assert_eq!(ev.audit.payload_capture, "disabled");
        assert_eq!(ev.audit.source_provenance, "unavailable");
        assert!(ev.payload_bytes_b64.is_none());
    }

    #[test]
    fn deletes_have_explicit_observation_semantics_and_do_not_complete_onboarding() {
        let event = record(
            "slot/hello/v1",
            b"hello from slot at 1.0",
            Some("forged".into()),
            "now".into(),
            Capture {
                subscriber: "receiver-A",
                identity_model: "per-uuid-slot-pool",
                cap: 16384,
                replay: false,
                sample_kind: "delete",
                receiving_site: "receiver",
                policy: &crate::attribution::Policy::default(),
            },
        );
        assert_eq!(event.audit.sample_kind, "delete");
        assert_eq!(event.audit.outcome, "observed");
        assert_eq!(event.audit.receiving_site, "receiver");
        assert_eq!(event.audit.onboarding_classification, "other");
        assert_eq!(event.audit.attribution.namespace_attribution, "unknown");
        assert_eq!(event.audit.source_provenance, "publisher_asserted");
    }

    #[test]
    fn shared_onboarding_fixtures() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/hello.json")).unwrap();
        for c in cases.as_array().unwrap() {
            assert_eq!(
                is_hello(
                    c["key"].as_str().unwrap(),
                    c["body"].as_str().unwrap().as_bytes()
                ),
                c["hello"].as_bool().unwrap(),
                "{c}"
            );
        }
    }
}
