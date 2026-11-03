// SPDX-License-Identifier: Apache-2.0
//
// Env-driven configuration. Every field has a documented STETHOSCOPE_*
// env var; defaults disable network-touching paths so a partial deploy
// can run + render skeletons + return `Skipped` cards rather than crash.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use panoscope_core::{AuditTailConfig, PortalClient};

/// Resolved runtime configuration. Built once at startup from env vars.
#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub portal_base_url: String,
    pub portal_api_key: String,
    pub audit: AuditTailConfig,
    /// User-facing site label rendered in the top bar (e.g. "nova-a").
    pub site_label: String,
    /// Block 94 cross-surface deep-link contract — sibling-surface
    /// base URLs (`docs/design/zenoh-surface-deep-links.md`). Each is
    /// `None` when no sibling exists at this deployment; the topbar
    /// hides the corresponding button rather than rendering a dead
    /// href. Portal URL is omitted here — stethoscope already carries
    /// it as `portal_base_url`.
    pub siblings: Siblings,
    /// Display-only participant-noun lexicon (nova-a "vendor" / ZAGROS "org").
    pub lexicon: Lexicon,
}

/// Sibling-surface origins (Block 94). Panoscope + flashlight only;
/// portal is already in `Config::portal_base_url`.
#[derive(Debug, Clone, Default)]
pub struct Siblings {
    pub panoscope: Option<String>,
    pub flashlight: Option<String>,
}

/// Display-only lexicon — the goatnet-appropriate participant noun so one
/// stethoscope image renders correct wording on every goatnet (nova-a
/// "Vendor"; ZAGROS "Org"). Render-time substitution only: the `vendor`
/// `TargetKind` / `data-kind` value, the `?vendor=` deep-link param, the
/// portal `list_vendors` call, and the cert-bound anchor are all
/// untouched. Mirror of the portal/panoscope lexicon
/// (`docs/design/portal-lexicon.md`). Defaults reproduce nova-a wording
/// (byte-stable); ZAGROS overrides via `STETHOSCOPE_LEX_ORG` from the same
/// `portal_lex_org` group_var the portal reads.
#[derive(Debug, Clone)]
pub struct Lexicon {
    /// lower singular ("vendor" / "org") — placeholders + hint text.
    pub org: String,
    /// upper singular ("VENDOR" / "ORG") — the hero kind eyebrow.
    pub org_upper: String,
    /// title-case singular ("Vendor" / "Org") — the kind-chip button.
    pub org_title: String,
}

impl Default for Lexicon {
    fn default() -> Self {
        Self {
            org: "vendor".to_string(),
            org_upper: "VENDOR".to_string(),
            org_title: "Vendor".to_string(),
        }
    }
}

impl Lexicon {
    /// Build from `STETHOSCOPE_LEX_ORG`; upper/title derive from it.
    /// Unset → nova-a default.
    pub fn from_env() -> Self {
        let mut l = Self::default();
        if let Ok(v) = std::env::var("STETHOSCOPE_LEX_ORG") {
            let t = v.trim();
            if !t.is_empty() {
                l.org = t.to_string();
                l.org_upper = t.to_uppercase();
                let mut c = t.chars();
                l.org_title = match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => t.to_string(),
                };
            }
        }
        l
    }
}

impl Config {
    /// Load from `STETHOSCOPE_*` env vars. Returns the resolved config
    /// or an error describing which required field is malformed.
    pub fn from_env() -> Result<Self> {
        let bind: SocketAddr = std::env::var("STETHOSCOPE_BIND")
            .unwrap_or_else(|_| "0.0.0.0:8080".into())
            .parse()
            .map_err(|e| anyhow!("STETHOSCOPE_BIND parse: {e}"))?;
        let portal_base_url =
            std::env::var("STETHOSCOPE_PORTAL_URL").unwrap_or_default();
        let portal_api_key =
            std::env::var("STETHOSCOPE_PORTAL_API_KEY").unwrap_or_default();
        let site_label =
            std::env::var("STETHOSCOPE_SITE_LABEL").unwrap_or_else(|_| "trial".into());

        let audit = AuditTailConfig {
            enabled: std::env::var("STETHOSCOPE_AUDIT_ENABLED")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
            endpoint: std::env::var("STETHOSCOPE_AUDIT_ENDPOINT").unwrap_or_default(),
            region: std::env::var("STETHOSCOPE_AUDIT_REGION")
                .unwrap_or_else(|_| "us-east-1".into()),
            bucket: std::env::var("STETHOSCOPE_AUDIT_BUCKET")
                .unwrap_or_else(|_| "audit-integrity".into()),
            key_prefix: std::env::var("STETHOSCOPE_AUDIT_KEY_PREFIX").unwrap_or_default(),
            access_key: std::env::var("STETHOSCOPE_AUDIT_ACCESS_KEY").unwrap_or_default(),
            secret_key: std::env::var("STETHOSCOPE_AUDIT_SECRET_KEY").unwrap_or_default(),
            request_timeout: Duration::from_secs(
                std::env::var("STETHOSCOPE_AUDIT_REQUEST_TIMEOUT_S")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(30),
            ),
            max_objects: std::env::var("STETHOSCOPE_AUDIT_MAX_OBJECTS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5_000),
        };
        let gaps = audit_env_gaps(&audit);
        if !gaps.is_empty() {
            bail!(
                "STETHOSCOPE_AUDIT_ENABLED is true but {} unset or empty; set them or disable the tail",
                gaps.join(", ")
            );
        }

        let siblings = Siblings {
            panoscope: std::env::var("STETHOSCOPE_SIBLING_PANOSCOPE_URL")
                .ok()
                .map(|s| s.trim().trim_end_matches('/').to_string())
                .filter(|s| !s.is_empty()),
            flashlight: std::env::var("STETHOSCOPE_SIBLING_FLASHLIGHT_URL")
                .ok()
                .map(|s| s.trim().trim_end_matches('/').to_string())
                .filter(|s| !s.is_empty()),
        };

        Ok(Self {
            bind,
            portal_base_url,
            portal_api_key,
            audit,
            site_label,
            siblings,
            lexicon: Lexicon::from_env(),
        })
    }

    /// Build the PortalClient — returns a disabled client when URL or
    /// key is empty.
    pub fn portal_client(&self) -> PortalClient {
        PortalClient::new(self.portal_base_url.clone(), self.portal_api_key.clone())
    }
}

/// The env vars an enabled audit tail cannot run without. Empty when the
/// tail is disabled or complete. Kept apart from `from_env` so the rule
/// is testable without touching process env.
pub fn audit_env_gaps(audit: &AuditTailConfig) -> Vec<&'static str> {
    if !audit.enabled {
        return Vec::new();
    }
    [
        ("STETHOSCOPE_AUDIT_ENDPOINT", audit.endpoint.is_empty()),
        ("STETHOSCOPE_AUDIT_ACCESS_KEY", audit.access_key.is_empty()),
        ("STETHOSCOPE_AUDIT_SECRET_KEY", audit.secret_key.is_empty()),
    ]
    .into_iter()
    .filter_map(|(name, empty)| empty.then_some(name))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_tail_has_no_gaps_whatever_is_empty() {
        assert!(audit_env_gaps(&AuditTailConfig::default()).is_empty());
    }

    #[test]
    fn an_enabled_tail_names_each_missing_env_var() {
        let audit = AuditTailConfig {
            enabled: true,
            endpoint: "http://127.0.0.1:9000".into(),
            access_key: "reader".into(),
            ..AuditTailConfig::default()
        };
        assert_eq!(audit_env_gaps(&audit), vec!["STETHOSCOPE_AUDIT_SECRET_KEY"]);
        let bare = AuditTailConfig {
            enabled: true,
            ..AuditTailConfig::default()
        };
        assert_eq!(audit_env_gaps(&bare).len(), 3);
    }

    #[test]
    fn defaults_when_env_empty() {
        // Clear our env vars for this test; rely on defaults.
        for k in [
            "STETHOSCOPE_BIND",
            "STETHOSCOPE_PORTAL_URL",
            "STETHOSCOPE_PORTAL_API_KEY",
            "STETHOSCOPE_AUDIT_ENABLED",
            "STETHOSCOPE_AUDIT_ENDPOINT",
            "STETHOSCOPE_AUDIT_BUCKET",
            "STETHOSCOPE_SITE_LABEL",
            "STETHOSCOPE_SIBLING_PANOSCOPE_URL",
            "STETHOSCOPE_SIBLING_FLASHLIGHT_URL",
        ] {
            std::env::remove_var(k);
        }
        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.bind.port(), 8080);
        assert!(cfg.portal_base_url.is_empty());
        assert!(cfg.portal_api_key.is_empty());
        assert!(!cfg.audit.enabled);
        assert_eq!(cfg.audit.bucket, "audit-integrity");
        assert_eq!(cfg.site_label, "trial");
        // Block 94: siblings default to None when env vars unset.
        assert!(cfg.siblings.panoscope.is_none());
        assert!(cfg.siblings.flashlight.is_none());
    }
}
