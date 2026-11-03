//! Operator-action audit presence (design source S1, F-207): the
//! `ops/audit/operator/<actor>` stream, read from the same MinIO
//! audit-integrity tier the fabric adapter uses. The payload is
//! age-encrypted to the operator roster — this adapter deliberately does
//! NOT decrypt (the service holds no roster age identity; key custody
//! stays with operators). What the ciphertext record still proves, with
//! strong attribution from the key path: WHO took an admin action and
//! WHEN. The detail line points at `goat audit ops` for the decrypted
//! what. Folding decrypted detail in is the named F-207 follow-on.

use async_trait::async_trait;

use crate::adapters::{tail_from, AdapterOutput, SourceAdapter};
use crate::config::Config;
use crate::event::{Attribution, TimelineEvent};
use crate::spine::EntitySpine;

pub struct OperatorAudit;

const SOURCE: &str = "operator-audit";
const OP_PREFIX: &str = "ops/audit/operator/";

#[async_trait]
impl SourceAdapter for OperatorAudit {
    fn name(&self) -> &'static str {
        SOURCE
    }

    async fn collect(&self, cfg: &Config, spine: &EntitySpine) -> AdapterOutput {
        let tail = match tail_from(cfg) {
            Ok(Some(t)) => t,
            Ok(None) => return AdapterOutput::skipped(SOURCE, "MICROSCOPE_AUDIT_* unset"),
            Err(e) => return AdapterOutput::error(SOURCE, e),
        };
        // The stream's first key segment is `ops`, so one vendor-filtered
        // fetch narrows to the ops namespace before the prefix match.
        let (rows, stats) = match tail.recent_events(cfg.audit_window_hours, Some("ops")).await {
            Ok(v) => v,
            Err(e) => return AdapterOutput::error(SOURCE, e),
        };
        let partial = stats.objects_fetched >= 5_000;
        let keys: Vec<String> = spine.match_keys().iter().map(|k| k.to_lowercase()).collect();
        let mut events = Vec::new();
        for r in rows {
            let Some(actor) = r.key_expr.strip_prefix(OP_PREFIX) else {
                continue;
            };
            let actor = actor.split('/').next().unwrap_or(actor).to_string();
            if !keys.iter().any(|k| actor.eq_ignore_ascii_case(k)) {
                continue;
            }
            events.push(TimelineEvent {
                ts: crate::timefmt::canonical_str(&r.ts),
                source: SOURCE,
                action: "operator-action".into(),
                actor: actor.clone(),
                target: None,
                summary: format!(
                    "operator action recorded ({} B age-encrypted — decrypt: goat audit ops --actor {})",
                    r.payload_len, actor
                ),
            kind: crate::event::EventKind::Admin,
            outcome: crate::event::Outcome::Unrecorded,
            direction: crate::event::Direction::BySubject,
            detail: None,
            attribution: Attribution::Strong,
                raw: serde_json::to_value(&r).ok(),
            });
        }
        let coverage = if partial {
            format!("PARTIAL {}h (object ceiling — older hours uncovered)", cfg.audit_window_hours)
        } else {
            format!("last {}h", cfg.audit_window_hours)
        };
        AdapterOutput::ok(
            SOURCE,
            &format!(
                "ciphertext presence from audit-integrity, {coverage}; decrypted detail stays operator-side (goat audit ops)"
            ),
            events,
        )
    }
}
