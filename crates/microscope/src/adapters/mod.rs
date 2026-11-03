//! Source adapters — one per actor-attributable stream (design §Sources).
//! Stethoscope plugin idiom: self-contained, reads the OWNING store, degrades
//! to `skipped` when unconfigured. Adding a stream = one file + one line in
//! [`all`].

use async_trait::async_trait;
use std::time::Duration;

use panoscope_core::audit_tail::{AuditTail, AuditTailConfig};

use crate::config::Config;
use crate::event::{SourceState, SourceStatus, TimelineEvent};
use crate::spine::EntitySpine;

pub mod console_audit;
pub mod fabric_audit;
pub mod hydra_consents;
mod kratos_sessions;
pub mod netbird_events;
pub mod netbird_peers;
pub mod operator_audit;
pub mod surface_visits;
pub mod portal_audit;

/// Shared MinIO audit-tail constructor for the two audit adapters.
/// `Ok(None)` ⇒ not configured on this deployment (adapters report
/// `skipped`): none of the three required fields are set, which is the
/// normal steady state for a site that has not turned audit tail on.
/// `Err` ⇒ configured but broken — only some of the three fields are set,
/// or the endpoint does not parse — and the caller must surface it rather
/// than fold it into a quiet skip.
pub(crate) fn tail_from(cfg: &Config) -> Result<Option<AuditTail>, String> {
    if cfg.audit_endpoint.is_empty()
        && cfg.audit_access_key.is_empty()
        && cfg.audit_secret_key.is_empty()
    {
        return Ok(None);
    }
    AuditTail::new(AuditTailConfig {
        enabled: true,
        endpoint: cfg.audit_endpoint.clone(),
        region: cfg.audit_region.clone(),
        bucket: cfg.audit_bucket.clone(),
        key_prefix: cfg.audit_key_prefix.clone(),
        access_key: cfg.audit_access_key.clone(),
        secret_key: cfg.audit_secret_key.clone(),
        request_timeout: Duration::from_secs(30),
        max_objects: 5_000,
    })
    .map(Some)
    .map_err(|e| e.to_string())
}

pub struct AdapterOutput {
    pub events: Vec<TimelineEvent>,
    pub status: SourceStatus,
}

impl AdapterOutput {
    pub fn skipped(source: &'static str, why: &str) -> Self {
        Self {
            events: Vec::new(),
            status: SourceStatus {
                source,
                state: SourceState::Skipped,
                detail: why.to_string(),
                n: 0,
            },
        }
    }
    pub fn error(source: &'static str, err: impl std::fmt::Display) -> Self {
        Self {
            events: Vec::new(),
            status: SourceStatus {
                source,
                state: SourceState::Error,
                detail: err.to_string(),
                n: 0,
            },
        }
    }
    pub fn ok(source: &'static str, detail: &str, events: Vec<TimelineEvent>) -> Self {
        let n = events.len();
        Self {
            events,
            status: SourceStatus {
                source,
                state: SourceState::Ok,
                detail: detail.to_string(),
                n,
            },
        }
    }
}

#[async_trait]
pub trait SourceAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    async fn collect(&self, cfg: &Config, spine: &EntitySpine) -> AdapterOutput;
}

/// The adapter registry. Order is presentation order in per-source status.
pub fn all() -> Vec<Box<dyn SourceAdapter>> {
    vec![
        Box::new(console_audit::ConsoleAudit),
        Box::new(kratos_sessions::KratosSessions),
        Box::new(hydra_consents::HydraConsents),
        Box::new(operator_audit::OperatorAudit),
        Box::new(portal_audit::PortalAudit),
        Box::new(fabric_audit::FabricAudit),
        // Sits next to operator_audit and fabric_audit because it reads the
        // same MinIO tier, and after hydra_consents because it answers the
        // question that source raises and cannot close: the grant says a person
        // reached a surface, this says how often and when.
        Box::new(surface_visits::SurfaceVisits),
        Box::new(netbird_events::NetbirdEvents),
        Box::new(netbird_peers::NetbirdPeers),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing set at all (the normal, steady state for a site that has
    /// never turned audit tail on) must stay a quiet skip, not an error.
    /// AuditTail holds a reqwest::Client and does not derive Debug, so the
    /// branches are matched explicitly rather than via an `assert!` with a
    /// `{:?}`-formatted value.
    #[test]
    fn tail_from_unconfigured_is_ok_none() {
        let cfg = Config::default();
        match tail_from(&cfg) {
            Ok(None) => {}
            Ok(Some(_)) => panic!("expected Ok(None) for a fully unconfigured site, got Ok(Some(_))"),
            Err(e) => panic!("expected Ok(None) for a fully unconfigured site, got Err({e})"),
        }
    }

    /// A malformed endpoint (present but not a valid URL) must surface its
    /// real parse error through tail_from's Result, not get discarded by
    /// `.ok()` and reported as "unset" by the caller.
    #[test]
    fn tail_from_malformed_endpoint_surfaces_parse_error() {
        let cfg = Config {
            audit_endpoint: "not a url".to_string(),
            audit_access_key: "access-key".to_string(),
            audit_secret_key: "secret-key".to_string(),
            ..Default::default()
        };
        let err = match tail_from(&cfg) {
            Err(e) => e,
            Ok(None) => panic!("expected Err surfacing the parse failure, got Ok(None)"),
            Ok(Some(_)) => panic!("expected Err surfacing the parse failure, got Ok(Some(_))"),
        };
        assert!(
            err.contains("not a url"),
            "expected the real parse error to name the malformed endpoint, got: {err}"
        );
    }

    /// Partial configuration (var-name drift between the secret mint and
    /// the consumer leaves one field empty while the others are set) is a
    /// real misconfiguration, not an intentional skip, and must surface.
    #[test]
    fn tail_from_partial_config_is_an_error_not_a_skip() {
        let cfg = Config {
            audit_endpoint: "http://198.18.0.9:9000".to_string(),
            audit_access_key: "access-key".to_string(),
            audit_secret_key: String::new(),
            ..Default::default()
        };
        let err = match tail_from(&cfg) {
            Err(e) => e,
            Ok(None) => panic!("expected Err for a partially configured tail, got Ok(None)"),
            Ok(Some(_)) => panic!("expected Err for a partially configured tail, got Ok(Some(_))"),
        };
        assert!(
            err.contains("secret_key"),
            "expected the error to name the missing field, got: {err}"
        );
    }
}
