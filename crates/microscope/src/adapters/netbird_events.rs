//! NetBird activity events (design source S5): peer enrollments, dashboard
//! logins, setup-key use, policy/group CRUD, with initiator/target ids.
//! Reads `GET /api/events` on the management API (admin PAT).

use async_trait::async_trait;

use crate::adapters::{AdapterOutput, SourceAdapter};
use crate::config::Config;
use crate::event::{Attribution, TimelineEvent};
use crate::spine::EntitySpine;

pub struct NetbirdEvents;

const SOURCE: &str = "netbird-events";

#[async_trait]
impl SourceAdapter for NetbirdEvents {
    fn name(&self) -> &'static str {
        SOURCE
    }

    async fn collect(&self, cfg: &Config, spine: &EntitySpine) -> AdapterOutput {
        if cfg.netbird_url.is_empty() || cfg.netbird_token.is_empty() {
            return AdapterOutput::skipped(SOURCE, "MICROSCOPE_NETBIRD_URL/TOKEN unset");
        }
        let url = format!("{}/api/events", cfg.netbird_url.trim_end_matches('/'));
        let resp = match reqwest::Client::new()
            .get(&url)
            .header("Authorization", format!("Token {}", cfg.netbird_token))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return AdapterOutput::error(SOURCE, e),
        };
        if !resp.status().is_success() {
            return AdapterOutput::error(SOURCE, format!("netbird returned {}", resp.status()));
        }
        let rows: Vec<serde_json::Value> = match resp.json().await {
            Ok(v) => v,
            Err(e) => return AdapterOutput::error(SOURCE, e),
        };
        let keys: Vec<String> = spine.match_keys().iter().map(|k| k.to_lowercase()).collect();
        let peer_names: Vec<String> = spine.netbird_peers.iter().map(|p| p.to_lowercase()).collect();
        let mut events = Vec::new();
        for rec in rows {
            let initiator_email = rec["initiator_email"].as_str().unwrap_or_default();
            let initiator_name = rec["initiator_name"].as_str().unwrap_or_default();
            // initiator_id is the Ory identity UUID, and it can be the ONLY
            // populated identifier: NetBird can return initiator_email = "" and
            // initiator_name = "" on every event while carrying a real
            // initiator_id. Matching only the two empty fields would report n=0
            // for a person whose actions sit right there in the response — a false
            // "nothing happened" on the surface whose entire job is showing what
            // someone did.
            //
            // ory_id is already in spine.match_keys(), so no new join is
            // introduced here: this reads the field that actually carries it.
            let initiator_id = rec["initiator_id"].as_str().unwrap_or_default();
            let meta = rec["meta"].to_string().to_lowercase();
            let hit_initiator = keys.iter().any(|k| {
                initiator_email.eq_ignore_ascii_case(k)
                    || initiator_name.eq_ignore_ascii_case(k)
                    || initiator_id.eq_ignore_ascii_case(k)
            });
            // Meta carries peer names/fqdns for peer events — convention join.
            let hit_peer = peer_names.iter().any(|p| !p.is_empty() && meta.contains(p));
            if !hit_initiator && !hit_peer {
                continue;
            }
            let activity = rec["activity_code"]
                .as_str()
                .or_else(|| rec["activity"].as_str())
                .unwrap_or("?")
                .to_string();
            events.push(TimelineEvent {
                ts: crate::timefmt::canonical(&rec["timestamp"]),
                source: SOURCE,
                action: activity.clone(),
                actor: if !initiator_email.is_empty() {
                    initiator_email.to_string()
                } else if !initiator_name.is_empty() {
                    initiator_name.to_string()
                } else {
                    initiator_id.to_string()
                },
                target: None,
                summary: format!("netbird {activity}"),
            kind: crate::event::EventKind::classify(&activity),
            outcome: crate::event::Outcome::Unrecorded,
            direction: crate::event::Direction::from_hits(hit_initiator, hit_peer),
            detail: None,
            attribution: if hit_initiator {
                    Attribution::Strong
                } else {
                    Attribution::Convention
                },
                raw: Some(rec),
            });
        }
        AdapterOutput::ok(SOURCE, "GET /api/events", events)
    }
}
