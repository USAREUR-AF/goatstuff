//! Ory Kratos sessions (design source S6): login history proxy with per-
//! session device IP / UA / AAL. Reads the admin API (loopback on the mgmt
//! host). NOTE (gap G3): this is a session TABLE, not an auth-event stream —
//! history depth is an accident of non-pruning and the adapter says so.

use async_trait::async_trait;

use crate::adapters::{AdapterOutput, SourceAdapter};
use crate::config::Config;
use crate::event::{Attribution, TimelineEvent};
use crate::spine::EntitySpine;

pub struct KratosSessions;

const SOURCE: &str = "kratos-sessions";

#[async_trait]
impl SourceAdapter for KratosSessions {
    fn name(&self) -> &'static str {
        SOURCE
    }

    async fn collect(&self, cfg: &Config, spine: &EntitySpine) -> AdapterOutput {
        if cfg.kratos_admin_url.is_empty() {
            return AdapterOutput::skipped(SOURCE, "MICROSCOPE_KRATOS_ADMIN_URL unset");
        }
        let Some(ory_id) = spine.ory_id.clone() else {
            return AdapterOutput::skipped(SOURCE, "no Ory identity resolved for this entity");
        };
        let url = format!(
            "{}/admin/identities/{}/sessions",
            cfg.kratos_admin_url.trim_end_matches('/'),
            ory_id
        );
        let resp = match reqwest::Client::new().get(&url).send().await {
            Ok(r) => r,
            Err(e) => return AdapterOutput::error(SOURCE, e),
        };
        let sessions: Vec<serde_json::Value> = match resp.json().await {
            Ok(v) => v,
            Err(e) => return AdapterOutput::error(SOURCE, e),
        };
        let email = spine.email.clone().unwrap_or_else(|| ory_id.clone());
        let mut events = Vec::new();
        for s in sessions {
            let active = s["active"].as_bool().unwrap_or(false);
            let aal = s["authenticator_assurance_level"].as_str().unwrap_or("?");
            let device = s["devices"]
                .as_array()
                .and_then(|d| d.first())
                .map(|d| {
                    format!(
                        "{} ({})",
                        d["ip_address"].as_str().unwrap_or("?"),
                        d["user_agent"].as_str().unwrap_or("?")
                    )
                })
                .unwrap_or_else(|| "unknown device".into());
            events.push(TimelineEvent {
                ts: crate::timefmt::canonical(&s["authenticated_at"]),
                source: SOURCE,
                action: "login".into(),
                actor: email.clone(),
                target: None,
                // Name the LAYER, not just the vendor. "Ory login" told a
                // reader nothing and — sitting beside hydra-consents rows that
                // DO name a service — read as a gap the tool had failed to
                // fill. It is not a gap: Kratos authenticates the PERSON and a
                // session carries no destination, by design. The row now says
                // that in place, and points at the rows that do carry one, so
                // the absence is legible instead of looking like a defect.
                summary: format!(
                    "identity proven to Kratos (authentication · aal={aal}, {}) from {device} — a session has NO destination; see the hydra-consents rows for which service was reached",
                    if active { "active" } else { "expired" }
                ),
            kind: crate::event::EventKind::classify("ory.login"),
            outcome: crate::event::Outcome::Unrecorded,
            direction: crate::event::Direction::Reflexive,
            detail: None,
            attribution: Attribution::Strong,
                raw: Some(s),
            });
        }
        AdapterOutput::ok(
            SOURCE,
            "session table (history depth = non-pruning accident, gap G3)",
            events,
        )
    }
}
