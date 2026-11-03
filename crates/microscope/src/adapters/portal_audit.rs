//! Portal `audit_events` (design source S3): vendor/topic/probation/label/
//! onboard actions from `GET /api/audit/events`. Attribution honesty (gap
//! G4): rows whose actor is the shared `"operator"` literal are tagged
//! `coarse` — they mean "someone holding the shared key", not this entity.

use async_trait::async_trait;

use crate::adapters::{AdapterOutput, SourceAdapter};
use crate::config::Config;
use crate::event::{Attribution, TimelineEvent};
use crate::spine::EntitySpine;

pub struct PortalAudit;

const SOURCE: &str = "portal-audit";

#[async_trait]
impl SourceAdapter for PortalAudit {
    fn name(&self) -> &'static str {
        SOURCE
    }

    async fn collect(&self, cfg: &Config, spine: &EntitySpine) -> AdapterOutput {
        if cfg.portal_url.is_empty() || cfg.portal_api_key.is_empty() {
            return AdapterOutput::skipped(SOURCE, "MICROSCOPE_PORTAL_URL/API_KEY unset");
        }
        let url = format!(
            "{}/api/audit/events",
            cfg.portal_url.trim_end_matches('/')
        );
        let resp = match reqwest::Client::new()
            .get(&url)
            .bearer_auth(&cfg.portal_api_key)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return AdapterOutput::error(SOURCE, e),
        };
        if !resp.status().is_success() {
            return AdapterOutput::error(SOURCE, format!("portal returned {}", resp.status()));
        }
        let body: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => return AdapterOutput::error(SOURCE, e),
        };
        let rows = body["events"]
            .as_array()
            .or_else(|| body.as_array())
            .cloned()
            .unwrap_or_default();
        let keys: Vec<String> = spine.match_keys().iter().map(|k| k.to_lowercase()).collect();
        let mut events = Vec::new();
        for rec in rows {
            let actor = rec["actor"].as_str().unwrap_or_default();
            let target = rec["target"].as_str().unwrap_or_default();
            let actor_l = actor.to_lowercase();
            let target_l = target.to_lowercase();
            let hit_actor = keys.contains(&actor_l);
            // Anchor-prefixed targets: match any spine anchor as a prefix.
            let hit_target = keys
                .iter()
                .any(|k| target_l == *k || target_l.starts_with(&format!("{k}/")));
            if !hit_actor && !hit_target {
                continue;
            }
            let action = rec["action"].as_str().unwrap_or("?").to_string();
            let attribution = if actor == "operator" {
                Attribution::Coarse
            } else {
                Attribution::Strong
            };
            events.push(TimelineEvent {
                ts: crate::timefmt::canonical(&rec["occurred_at"]),
                source: SOURCE,
                action: action.clone(),
                actor: actor.to_string(),
                target: (!target.is_empty()).then(|| target.to_string()),
                summary: format!(
                    "portal {action}{}",
                    if attribution == Attribution::Coarse {
                        " (shared-key actor — coarse, gap G4)"
                    } else {
                        ""
                    }
                ),
                kind: crate::event::EventKind::classify(&action),
                outcome: crate::event::Outcome::Unrecorded,
                direction: crate::event::Direction::from_hits(hit_actor, hit_target),
                detail: None,
                attribution,
                raw: Some(rec),
            });
        }
        AdapterOutput::ok(SOURCE, "GET /api/audit/events", events)
    }
}
