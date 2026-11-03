//! Ory Hydra OAuth2 consent grants (design source S6, the half that was never
//! built): WHICH SERVICE each authentication was for.
//!
//! The Kratos adapter answers "this person authenticated, from this IP, at this
//! assurance level". It cannot answer "to what" — and not because of how it
//! renders. A Kratos session carries only `active`, `authenticated_at`,
//! `authentication_methods`, `authenticator_assurance_level`, `devices`,
//! `expires_at`, `id`, `identity`, `issued_at`. There is no destination in it,
//! because Kratos authenticates the PERSON; Hydra is what grants access to a
//! SERVICE. Every microscope timeline therefore read as a list of identical
//! "Ory login" lines — true, and useless for the question the surface exists to
//! answer, which is what someone reached.
//!
//! Hydra holds the other half: one consent grant per (subject, client), where
//! the client_id IS the service — `microscope`, `isitup`, `netbird-dashboard`,
//! `oauth2-proxy-id-console`, and so on. A single operator can hold dozens of
//! grants across several services.
//!
//! HONESTY LIMITS, stated because this source is easy to over-read:
//!
//!   * A consent grant is not a login EVENT. It is the standing record that
//!     this person granted this client access, with `handled_at` as the last
//!     time that grant was exercised. Several logins to the same service
//!     collapse into one row with a moving timestamp. The summary says
//!     "access granted", never "logged in N times".
//!   * `skip_consent` clients (every internal surface here) never show a human
//!     consent screen — the grant records that Hydra issued the token, which is
//!     the fact worth having, not that anyone clicked "allow".
//!   * Revoked grants are absent from this API, so absence is NOT evidence a
//!     person never reached a service. Same class as F-177: this adapter can
//!     show what IS granted, never prove what never happened.

use async_trait::async_trait;

use crate::adapters::{AdapterOutput, SourceAdapter};
use crate::config::Config;
use crate::event::{Attribution, TimelineEvent};
use crate::spine::EntitySpine;

pub struct HydraConsents;

const SOURCE: &str = "hydra-consents";

#[async_trait]
impl SourceAdapter for HydraConsents {
    fn name(&self) -> &'static str {
        SOURCE
    }

    async fn collect(&self, cfg: &Config, spine: &EntitySpine) -> AdapterOutput {
        if cfg.hydra_admin_url.is_empty() {
            return AdapterOutput::skipped(SOURCE, "MICROSCOPE_HYDRA_ADMIN_URL unset");
        }
        // Keyed on the Ory subject, which is the identity UUID. No name join:
        // if the spine has no ory_id there is nothing to ask Hydra about, and
        // saying so is better than a query that silently matches nothing.
        let Some(ory_id) = spine.ory_id.clone() else {
            return AdapterOutput::skipped(SOURCE, "no Ory identity resolved for this entity");
        };
        let url = format!(
            "{}/admin/oauth2/auth/sessions/consent?subject={}",
            cfg.hydra_admin_url.trim_end_matches('/'),
            urlencoding(&ory_id)
        );
        let resp = match reqwest::Client::new().get(&url).send().await {
            Ok(r) => r,
            Err(e) => return AdapterOutput::error(SOURCE, e),
        };
        let grants: Vec<serde_json::Value> = match resp.json().await {
            Ok(v) => v,
            Err(e) => return AdapterOutput::error(SOURCE, e),
        };

        let actor = spine
            .email
            .clone()
            .unwrap_or_else(|| ory_id.clone());
        let mut events = Vec::new();
        for g in &grants {
            let req = &g["consent_request"];
            let client = req["client"]["client_id"].as_str().unwrap_or("");
            if client.is_empty() {
                continue;
            }
            // handled_at is when the grant was last exercised; fall back to the
            // request time. Never re-stamped with read time (microscope.md).
            let ts = crate::timefmt::canonical(if !g["handled_at"].is_null() {
                &g["handled_at"]
            } else {
                &req["requested_at"]
            });
            let scopes: Vec<String> = g["grant_scope"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let scope_note = if scopes.is_empty() {
                String::new()
            } else {
                format!(" [{}]", scopes.join(" "))
            };
            let remember = g["remember"].as_bool().unwrap_or(false);
            events.push(TimelineEvent {
                ts,
                source: SOURCE,
                action: "oauth2.access_granted".into(),
                actor: actor.clone(),
                target: Some(client.to_string()),
                // The complement of the Kratos row: this is the AUTHORIZATION
                // half, and it is the one that knows the destination.
                summary: format!(
                    "authorized to reach {client} (authorization · service named){scope_note}{}",
                    if remember { " (session remembered)" } else { "" }
                ),
                // Strong: keyed on the Ory subject UUID, not on any name.
            kind: crate::event::EventKind::classify("oauth2.access_granted"),
            outcome: crate::event::Outcome::Ok,
            direction: crate::event::Direction::Reflexive,
            detail: None,
            attribution: Attribution::Strong,
                raw: Some(g.clone()),
            });
        }

        let n = events.len();
        let services: std::collections::BTreeSet<String> = events
            .iter()
            .filter_map(|e| e.target.clone())
            .collect();
        AdapterOutput::ok(
            SOURCE,
            &format!(
                "GET /admin/oauth2/auth/sessions/consent — {n} standing grant(s) across {} service(s): {}. \
                 A grant is the standing record that access was issued, NOT a per-login event; \
                 revoked grants are absent, so absence is not evidence a service was never reached.",
                services.len(),
                if services.is_empty() {
                    "none".to_string()
                } else {
                    services.iter().cloned().collect::<Vec<_>>().join(", ")
                }
            ),
            events,
        )
    }
}

/// Minimal percent-encoding for the subject query value. Identity UUIDs are
/// already URL-safe; this exists so a non-UUID subject (an email on a
/// differently-configured goatnet) cannot break the query.
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
