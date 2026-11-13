// SPDX-License-Identifier: Apache-2.0
//
// Runtime config for panoscope. Env-var driven; defaults are
// hackathon-scope sensible.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// Re-export TlsConfig from panoscope-core so existing consumers using
// `crate::config::TlsConfig` are unchanged. Factored out in Phase B
// so flashlight reuses the same Zenoh-mTLS substrate.
pub use panoscope_core::TlsConfig;

/// Default WS coalescing tick (ms). Per docs/design/panoscope.md
/// §"Sampling + back-pressure" — keeps browser-side D3 repaint cost
/// bounded while feeling live.
pub const DEFAULT_WS_COALESCE_MS: u64 = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// HTTP bind address (e.g. "0.0.0.0:8080").
    pub bind: SocketAddr,

    /// Zenoh router endpoint (e.g. "tcp/127.0.0.1:7447" or
    /// "tls/goat-zenoh-router:7447").
    pub zenoh_endpoint: String,

    /// Optional client-side mTLS material for `tls/` endpoints. When
    /// `enabled` is true, panoscope injects `transport/link/tls/*` into
    /// the Zenoh config before opening the session. Required when the
    /// router has `enable_mtls: true` on its TLS listener.
    pub zenoh_tls: TlsConfig,

    /// Vendor portal /api/schemas URL. Resolver tries this first;
    /// falls back to the embedded map on 404 / connection-refused.
    pub portal_schemas_url: String,

    /// Refresh interval for the portal schemas (seconds). 0 disables
    /// refresh (only fetched at startup).
    pub portal_refresh_secs: u64,

    /// Bearer token for portal calls that gate behind AuthMiddleware
    /// (currently `/api/schemas/<key>` for protobuf descriptor fetch by
    /// the decoded-stream view). Empty disables the decoder — frontend
    /// renders NO SCHEMA for every topic with a tooltip pointing at the
    /// missing env var. See decoder.rs / portal_client.rs.
    pub portal_api_key: String,

    /// Per-topic data-tail sampling ceiling (events/sec).
    pub sampling_per_topic_eps: u32,

    /// WebSocket coalescing tick (milliseconds).
    pub ws_coalesce_ms: u64,

    /// Topic-prefix blocklist — panoscope drops any sample whose
    /// key_expr starts with one of these strings before it touches the
    /// topology / flow / sample ring. Applies to both the live
    /// `data_tail` subscriber and the `minio_replay` audit walker.
    /// Vendor-c-class noisy publishers can be hidden from the COP
    /// without revoking their Zenoh write ACL; they keep publishing
    /// to the router, the audit-subscriber still records them, and
    /// other subscribers still receive their samples. Panoscope just
    /// silences them in its own view.
    #[serde(default)]
    pub topic_blocklist: Vec<String>,

    /// Optional MinIO replay-on-boot — re-hydrates the in-RAM peer
    /// registry from the audit-subscriber's NDJSON history. Disabled
    /// by default; see [`crate::minio_replay::MinioReplayConfig`].
    #[serde(skip)]
    pub minio_replay: crate::minio_replay::MinioReplayConfig,

    /// Sibling-surface base URLs for the cross-surface deep-link
    /// contract (Block 94, see `docs/design/zenoh-surface-deep-links.md`).
    /// Each surface is `None` when no sibling exists at this deployment;
    /// the frontend hides the corresponding outbound button rather than
    /// emitting a dead href. The `portal` field overrides the
    /// `portal_base_url()` derivation when set — useful when the portal
    /// origin differs from the schemas origin (rare but possible).
    #[serde(default)]
    pub siblings: SiblingUrls,

    /// Display-only lexicon — the goatnet-appropriate participant noun +
    /// fabric label so one panoscope image renders correct wording on
    /// every goatnet. Mirror of the portal's `Lexicon`
    /// (`docs/design/portal-lexicon.md`): a trial fabric renders "vendor" /
    /// "<site> fabric"; a production goatnet renders "org" / "<site> Fabric".
    #[serde(default)]
    pub lexicon: Lexicon,

    /// Per-viewer RBAC scoping (ADR 1073). Off by default — behavior is
    /// byte-identical to pre-RBAC panoscope until an operator flips
    /// PANOSCOPE_RBAC_MODE=enforce (with the oauth2-proxy sidecar in
    /// front supplying X-Auth-Request-Groups). See src/viewer.rs.
    #[serde(skip)]
    pub rbac: crate::viewer::RbacConfig,
}

/// Display-only vocabulary of the panoscope UI. Exists so one image
/// serves every goatnet with appropriate wording — a trial fabric's
/// participants are synthetic vendors, others' are organizations. **Render-time
/// substitution only**: this never changes a key, ACL subject, cert
/// field, or any machine path (mirror of the slot-alias-display-layer
/// invariant — display projects over the machine anchor; the anchor
/// never moves). The participant cluster on the wire is always keyed by
/// the cert-bound `vendor` field / UUID anchor regardless of the word
/// shown. See `docs/design/portal-lexicon.md` §"Panoscope display layer".
///
/// Defaults reproduce the trial-fabric wording so an unconfigured panoscope is
/// byte-stable with what shipped before the lexicon existed. A goatnet
/// overrides `org` (+ optionally `fabric_label`) via `PANOSCOPE_LEX_*`
/// env wired by the ansible role from the same `portal_lex_org`
/// group_var the portal reads — single source of truth, no drift.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lexicon {
    /// Participant noun, lower singular ("vendor" / "org").
    pub org: String,
    /// Participant noun, lower plural ("vendors" / "orgs").
    pub orgs: String,
    /// Participant noun, upper singular for caps badges ("VENDOR" / "ORG").
    pub org_upper: String,
    /// Participant noun, upper plural for caps legends ("VENDORS" / "ORGS").
    pub orgs_upper: String,
    /// Fabric label used in the page title + subtitle
    /// (for example "<site> fabric").
    pub fabric_label: String,
}

impl Default for Lexicon {
    fn default() -> Self {
        // Trial-fabric wording — byte-stable with pre-lexicon panoscope.
        Self {
            org: "vendor".to_string(),
            orgs: "vendors".to_string(),
            org_upper: "VENDOR".to_string(),
            orgs_upper: "VENDORS".to_string(),
            fabric_label: "Zenoh fabric".to_string(),
        }
    }
}

impl Lexicon {
    /// Fill derived forms when only the singular was overridden: plural
    /// from `org`, then upper-case variants from the (possibly
    /// overridden) lower forms. Idempotent; called once after env parse.
    fn normalize(&mut self) {
        if self.org.is_empty() {
            self.org = "vendor".to_string();
        }
        if self.orgs.is_empty() {
            self.orgs = format!("{}s", self.org);
        }
        if self.org_upper.is_empty() {
            self.org_upper = self.org.to_uppercase();
        }
        if self.orgs_upper.is_empty() {
            self.orgs_upper = self.orgs.to_uppercase();
        }
        if self.fabric_label.is_empty() {
            self.fabric_label = "Zenoh fabric".to_string();
        }
    }

    /// HTML serve-time substitution tokens → resolved values. Applied in
    /// a single non-overlapping pass over `index.html` (see `embed.rs`).
    pub fn tokens(&self) -> [(&'static str, &str); 5] {
        [
            ("__LEX_ORG__", &self.org),
            ("__LEX_ORGS__", &self.orgs),
            ("__LEX_ORG_UPPER__", &self.org_upper),
            ("__LEX_ORGS_UPPER__", &self.orgs_upper),
            ("__LEX_FABRIC__", &self.fabric_label),
        ]
    }
}

/// Sibling-surface base URLs (e.g. `https://stethoscope.nova.netbird.nova.example`).
/// All four fields are optional — a panoscope deployed without a
/// stethoscope sibling carries `stethoscope: None` and the frontend
/// hides the diagnose buttons.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SiblingUrls {
    /// Stethoscope origin (e.g. `https://stethoscope.nova.netbird.nova.example`).
    /// Renders `diagnose ↗` outbound links in the panoscope side panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stethoscope: Option<String>,
    /// Flashlight origin. Renders `tail ↗` outbound links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flashlight: Option<String>,
    /// Microscope origin (e.g. `https://microscope.my-goatnet.example`).
    /// Renders a `microscope ↗` button on the /who roster detail — the
    /// COP-to-lens handoff (ADR 1090). Admin surface; the link only appears
    /// on /who (itself group-gated), never on the public data panels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microscope: Option<String>,
    /// Portal origin override. When set, takes precedence over
    /// [`Config::portal_base_url`]. When `None`, the frontend uses
    /// `portal_base_url` for backwards compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portal: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:8080".parse().expect("default bind parses"),
            zenoh_endpoint: "tcp/127.0.0.1:7447".to_string(),
            zenoh_tls: TlsConfig::default(),
            portal_schemas_url: "https://portal.goat.example/api/schemas".to_string(),
            portal_refresh_secs: 60,
            portal_api_key: String::new(),
            sampling_per_topic_eps: 50,
            ws_coalesce_ms: DEFAULT_WS_COALESCE_MS,
            topic_blocklist: Vec::new(),
            minio_replay: crate::minio_replay::MinioReplayConfig::default(),
            siblings: SiblingUrls::default(),
            lexicon: Lexicon::default(),
            rbac: crate::viewer::RbacConfig::default(),
        }
    }
}

impl Config {
    /// Derive the portal web origin (e.g. `https://portal.nova.netbird.nova.example`)
    /// from `portal_schemas_url` by stripping `/api/schemas`. The frontend
    /// uses this to link out to the portal's `/schema.html` viewer.
    pub fn portal_base_url(&self) -> String {
        let trimmed = self.portal_schemas_url.trim_end_matches('/');
        trimmed
            .strip_suffix("/api/schemas")
            .unwrap_or(trimmed)
            .trim_end_matches('/')
            .to_string()
    }

    /// Build a Config from environment variables; fall back to defaults
    /// for any unset field.
    ///
    /// Env vars:
    /// - `PANOSCOPE_BIND`              — bind addr
    /// - `PANOSCOPE_ZENOH_ENDPOINT`    — Zenoh router endpoint
    /// - `PANOSCOPE_ZENOH_MTLS_ENABLED`         — "true" enables TLS injection
    /// - `PANOSCOPE_ZENOH_TLS_ROOT_CA`          — PEM path (trust root)
    /// - `PANOSCOPE_ZENOH_TLS_CLIENT_CERT`      — PEM path (client cert)
    /// - `PANOSCOPE_ZENOH_TLS_CLIENT_KEY`       — PEM path (client key)
    /// - `PANOSCOPE_PORTAL_SCHEMAS_URL` — portal /api/schemas URL
    /// - `PANOSCOPE_PORTAL_REFRESH_SECS` — refresh interval seconds
    /// - `PANOSCOPE_SAMPLING_PER_TOPIC_EPS` — per-topic sampling ceiling
    /// - `PANOSCOPE_WS_COALESCE_MS`    — WS coalescing tick ms
    pub fn from_env_or_default() -> Result<Self> {
        let mut c = Self::default();

        if let Ok(v) = std::env::var("PANOSCOPE_BIND") {
            c.bind = v.parse().context("PANOSCOPE_BIND parse")?;
        }
        if let Ok(v) = std::env::var("PANOSCOPE_ZENOH_ENDPOINT") {
            c.zenoh_endpoint = v;
        }
        if let Ok(v) = std::env::var("PANOSCOPE_ZENOH_MTLS_ENABLED") {
            c.zenoh_tls.enabled = matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes");
        }
        if let Ok(v) = std::env::var("PANOSCOPE_ZENOH_TLS_ROOT_CA") {
            c.zenoh_tls.root_ca_certificate = v;
        }
        if let Ok(v) = std::env::var("PANOSCOPE_ZENOH_TLS_CLIENT_CERT") {
            c.zenoh_tls.connect_certificate = v;
        }
        if let Ok(v) = std::env::var("PANOSCOPE_ZENOH_TLS_CLIENT_KEY") {
            c.zenoh_tls.connect_private_key = v;
        }
        if c.zenoh_tls.enabled {
            anyhow::ensure!(
                !c.zenoh_tls.root_ca_certificate.is_empty()
                    && !c.zenoh_tls.connect_certificate.is_empty()
                    && !c.zenoh_tls.connect_private_key.is_empty(),
                "PANOSCOPE_ZENOH_MTLS_ENABLED=true requires \
                 PANOSCOPE_ZENOH_TLS_ROOT_CA + PANOSCOPE_ZENOH_TLS_CLIENT_CERT + \
                 PANOSCOPE_ZENOH_TLS_CLIENT_KEY"
            );
        }
        if let Ok(v) = std::env::var("PANOSCOPE_PORTAL_SCHEMAS_URL") {
            c.portal_schemas_url = v;
        }
        if let Ok(v) = std::env::var("PANOSCOPE_PORTAL_REFRESH_SECS") {
            c.portal_refresh_secs = v.parse().context("PANOSCOPE_PORTAL_REFRESH_SECS parse")?;
        }
        if let Ok(v) = std::env::var("PANOSCOPE_PORTAL_API_KEY") {
            c.portal_api_key = v;
        }
        if let Ok(v) = std::env::var("PANOSCOPE_SAMPLING_PER_TOPIC_EPS") {
            c.sampling_per_topic_eps =
                v.parse().context("PANOSCOPE_SAMPLING_PER_TOPIC_EPS parse")?;
        }
        if let Ok(v) = std::env::var("PANOSCOPE_WS_COALESCE_MS") {
            c.ws_coalesce_ms = v.parse().context("PANOSCOPE_WS_COALESCE_MS parse")?;
        }
        if let Ok(v) = std::env::var("PANOSCOPE_TOPIC_BLOCKLIST") {
            // Comma-separated topic-prefix list (e.g. "vendor-c,foo/bar").
            // Whitespace + empty entries discarded so the Ansible-rendered
            // list tolerates trailing commas or stray spaces.
            let parsed: Vec<String> = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            c.topic_blocklist = parsed.clone();
            c.minio_replay.topic_blocklist = parsed;
        }

        // MinIO replay-on-boot — see docs/design/panoscope.md §"MinIO
        // replay on boot". All knobs are optional; disabled by default.
        if let Ok(v) = std::env::var("PANOSCOPE_MINIO_REPLAY_ENABLED") {
            c.minio_replay.enabled =
                matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes");
        }
        if let Ok(v) = std::env::var("PANOSCOPE_MINIO_ENDPOINT") {
            c.minio_replay.endpoint = v;
        }
        if let Ok(v) = std::env::var("PANOSCOPE_MINIO_REGION") {
            c.minio_replay.region = v;
        }
        if let Ok(v) = std::env::var("PANOSCOPE_MINIO_BUCKET") {
            c.minio_replay.bucket = v;
        }
        if let Ok(v) = std::env::var("PANOSCOPE_MINIO_KEY_PREFIX") {
            c.minio_replay.key_prefix = v;
        }
        if let Ok(v) = std::env::var("PANOSCOPE_MINIO_ACCESS_KEY") {
            c.minio_replay.access_key = v;
        }
        if let Ok(v) = std::env::var("PANOSCOPE_MINIO_SECRET_KEY") {
            c.minio_replay.secret_key = v;
        }
        if let Ok(v) = std::env::var("PANOSCOPE_MINIO_WINDOW_HOURS") {
            c.minio_replay.window_hours =
                v.parse().context("PANOSCOPE_MINIO_WINDOW_HOURS parse")?;
        }
        if let Ok(v) = std::env::var("PANOSCOPE_MINIO_MAX_OBJECTS") {
            c.minio_replay.max_objects =
                v.parse().context("PANOSCOPE_MINIO_MAX_OBJECTS parse")?;
        }
        if let Ok(v) = std::env::var("PANOSCOPE_MINIO_FETCH_CONCURRENCY") {
            c.minio_replay.fetch_concurrency =
                v.parse().context("PANOSCOPE_MINIO_FETCH_CONCURRENCY parse")?;
        }
        if let Ok(v) = std::env::var("PANOSCOPE_MINIO_VENDOR_BLOCKLIST") {
            // Comma-separated list of vendor prefixes to skip during
            // replay (e.g. "caracal,community"). Whitespace + empty
            // entries are ignored so the Ansible-rendered list can carry
            // trailing commas or stray spaces without surprises.
            c.minio_replay.vendor_blocklist = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        if c.minio_replay.enabled {
            anyhow::ensure!(
                !c.minio_replay.endpoint.is_empty(),
                "PANOSCOPE_MINIO_REPLAY_ENABLED=true requires PANOSCOPE_MINIO_ENDPOINT"
            );
            // Credentials may be empty here when the operator hasn't
            // wired the secret yet — replay_into_topology() detects
            // that case and skips with a warning instead of failing
            // boot.
        }

        // Sibling-surface deep-link contract (Block 94). Each URL is
        // optional — unset env var = no sibling = no outbound button.
        if let Ok(v) = std::env::var("PANOSCOPE_SIBLING_STETHOSCOPE_URL") {
            let trimmed = v.trim().trim_end_matches('/').to_string();
            if !trimmed.is_empty() {
                c.siblings.stethoscope = Some(trimmed);
            }
        }
        if let Ok(v) = std::env::var("PANOSCOPE_SIBLING_MICROSCOPE_URL") {
            let trimmed = v.trim().trim_end_matches('/').to_string();
            if !trimmed.is_empty() {
                c.siblings.microscope = Some(trimmed);
            }
        }
        if let Ok(v) = std::env::var("PANOSCOPE_SIBLING_FLASHLIGHT_URL") {
            let trimmed = v.trim().trim_end_matches('/').to_string();
            if !trimmed.is_empty() {
                c.siblings.flashlight = Some(trimmed);
            }
        }
        if let Ok(v) = std::env::var("PANOSCOPE_SIBLING_PORTAL_URL") {
            let trimmed = v.trim().trim_end_matches('/').to_string();
            if !trimmed.is_empty() {
                c.siblings.portal = Some(trimmed);
            }
        }

        // Display-only lexicon (docs/design/portal-lexicon.md §"Panoscope
        // display layer"). Each var is optional; unset → default wording.
        // `PANOSCOPE_LEX_ORG` alone is enough — plural + caps forms
        // derive in `normalize()`. The ansible role wires these from the
        // same `portal_lex_org` group_var the portal reads.
        if let Ok(v) = std::env::var("PANOSCOPE_LEX_ORG") {
            let t = v.trim();
            if !t.is_empty() {
                c.lexicon.org = t.to_string();
                // Re-derive plural/caps from the new singular unless the
                // caller also overrode them explicitly below.
                c.lexicon.orgs = String::new();
                c.lexicon.org_upper = String::new();
                c.lexicon.orgs_upper = String::new();
            }
        }
        if let Ok(v) = std::env::var("PANOSCOPE_LEX_ORGS") {
            let t = v.trim();
            if !t.is_empty() {
                c.lexicon.orgs = t.to_string();
            }
        }
        if let Ok(v) = std::env::var("PANOSCOPE_LEX_FABRIC_LABEL") {
            let t = v.trim();
            if !t.is_empty() {
                c.lexicon.fabric_label = t.to_string();
            }
        }
        c.lexicon.normalize();

        c.rbac = crate::viewer::RbacConfig::from_env()?;

        Ok(c)
    }

    /// Resolved sibling URLs for the frontend. Composes
    /// [`SiblingUrls`] with the legacy `portal_base_url()` derivation:
    /// when `siblings.portal` is unset, falls back to the value derived
    /// from `portal_schemas_url`. Stethoscope and flashlight only
    /// appear when explicitly configured.
    pub fn resolved_siblings(&self) -> SiblingUrls {
        let portal = self
            .siblings
            .portal
            .clone()
            .or_else(|| {
                let derived = self.portal_base_url();
                if derived.is_empty() {
                    None
                } else {
                    Some(derived)
                }
            });
        SiblingUrls {
            stethoscope: self.siblings.stethoscope.clone(),
            flashlight: self.siblings.flashlight.clone(),
            microscope: self.siblings.microscope.clone(),
            portal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_sensible_values() {
        let c = Config::default();
        assert_eq!(c.bind.port(), 8080);
        assert_eq!(c.portal_refresh_secs, 60);
        assert_eq!(c.sampling_per_topic_eps, 50);
        assert_eq!(c.ws_coalesce_ms, 100);
    }

    #[test]
    fn config_round_trips_through_toml() {
        let c = Config::default();
        let s = toml::to_string(&c).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.bind, c.bind);
        assert_eq!(back.portal_schemas_url, c.portal_schemas_url);
        assert!(!back.zenoh_tls.enabled);
    }

    #[test]
    fn tls_config_defaults_to_disabled() {
        let c = Config::default();
        assert!(!c.zenoh_tls.enabled);
        assert!(c.zenoh_tls.root_ca_certificate.is_empty());
        assert!(c.zenoh_tls.connect_certificate.is_empty());
        assert!(c.zenoh_tls.connect_private_key.is_empty());
    }

    #[test]
    fn sibling_urls_default_to_all_none() {
        let c = Config::default();
        assert!(c.siblings.stethoscope.is_none());
        assert!(c.siblings.flashlight.is_none());
        assert!(c.siblings.portal.is_none());
    }

    #[test]
    fn resolved_siblings_falls_back_to_portal_base_url_when_unset() {
        let c = Config::default();
        // siblings.portal is None; resolved_siblings derives from
        // portal_schemas_url so the existing single-portal-link path
        // stays unchanged for deployments that don't set the new env vars.
        let resolved = c.resolved_siblings();
        assert_eq!(
            resolved.portal.as_deref(),
            Some("https://portal.goat.example"),
        );
        assert!(resolved.stethoscope.is_none());
        assert!(resolved.flashlight.is_none());
    }

    #[test]
    fn resolved_siblings_portal_override_wins() {
        let mut c = Config::default();
        c.siblings.portal = Some("https://other-portal.example".to_string());
        let resolved = c.resolved_siblings();
        assert_eq!(
            resolved.portal.as_deref(),
            Some("https://other-portal.example"),
        );
    }

    #[test]
    fn resolved_siblings_carries_explicit_stethoscope_and_flashlight() {
        let mut c = Config::default();
        c.siblings.stethoscope = Some("https://stethoscope.example".to_string());
        c.siblings.flashlight = Some("https://flashlight.example".to_string());
        let resolved = c.resolved_siblings();
        assert_eq!(
            resolved.stethoscope.as_deref(),
            Some("https://stethoscope.example"),
        );
        assert_eq!(
            resolved.flashlight.as_deref(),
            Some("https://flashlight.example"),
        );
    }
}
