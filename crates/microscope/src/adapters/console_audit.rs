//! id-admin-console `audit.jsonl` (design source S2) — the richest actor
//! context: identity CRUD, group patches, invites, onboarding decisions,
//! `result: ok|refused|error`. Read as a bind-mounted file until the
//! console's audit fan-out lands (gap G1); this adapter is the interim leg
//! that keeps the class of miss impossible meanwhile.

use async_trait::async_trait;

use crate::adapters::{AdapterOutput, SourceAdapter};
use crate::config::Config;
use crate::event::{Attribution, TimelineEvent};
use crate::spine::EntitySpine;

pub struct ConsoleAudit;

const SOURCE: &str = "console-audit";

#[async_trait]
impl SourceAdapter for ConsoleAudit {
    fn name(&self) -> &'static str {
        SOURCE
    }

    async fn collect(&self, cfg: &Config, spine: &EntitySpine) -> AdapterOutput {
        if cfg.console_audit_path.is_empty() {
            return AdapterOutput::skipped(SOURCE, "MICROSCOPE_CONSOLE_AUDIT_PATH unset");
        }
        let path = cfg.console_audit_path.clone();
        let keys: Vec<String> = spine.match_keys().iter().map(|k| k.to_lowercase()).collect();
        let read = tokio::task::spawn_blocking(move || std::fs::read_to_string(&path)).await;
        let text = match read {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => return AdapterOutput::error(SOURCE, e),
            Err(e) => return AdapterOutput::error(SOURCE, e),
        };
        let mut events = Vec::new();
        for line in text.lines() {
            let Ok(rec) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let actor_email = rec["actor"]["email"].as_str().unwrap_or_default();
            let actor_id = rec["actor"]["identity_id"].as_str().unwrap_or_default();
            let target_email = rec["target"]["email"].as_str().unwrap_or_default();
            let hit_as_actor = keys.iter().any(|k| {
                actor_email.eq_ignore_ascii_case(k) || (!actor_id.is_empty() && actor_id == k)
            });
            let hit_as_target = keys.iter().any(|k| target_email.eq_ignore_ascii_case(k));
            if !hit_as_actor && !hit_as_target {
                continue;
            }
            let action = rec["action"].as_str().unwrap_or("?").to_string();
            let result = rec["result"].as_str().unwrap_or("?");
            let role = if hit_as_actor { "as actor" } else { "as target" };
            events.push(TimelineEvent {
                ts: crate::timefmt::canonical(&rec["ts"]),
                source: SOURCE,
                action: action.clone(),
                actor: actor_email.to_string(),
                target: (!target_email.is_empty()).then(|| target_email.to_string()),
                summary: format!("console {action} ({result}, {role})"),
                kind: crate::event::EventKind::classify(&action),
                direction: crate::event::Direction::from_hits(hit_as_actor, hit_as_target),
                outcome: crate::event::Outcome::classify(result),
                // The console records WHY separately from WHETHER: `result` is
                // the verdict word, `error_message` is the sentence a real
                // person hit. Putting `result` here rendered the detail column
                // as the literal string "error" next to a row already badged
                // `failed` — restating the badge and discarding the only part
                // that is actionable. Messages are specific enough to fix things from: an
                // onboarding invite failing SES verification, a proposal falling out of the
                // lint gate.
                detail: error_detail(&rec),
                attribution: Attribution::Strong,
                raw: Some(rec),
            });
        }
        // "ok, 0" is TWO different facts and they must not render alike: the
        // file held nothing at all, or it held N rows and none named this
        // entity. A 0-byte file (a crash-looping console fence, say) must not report
        // the same "ok 0" as a healthy file with no match, or an outage is
        // indistinguishable from an uneventful person. The denominator is the
        // cheapest possible fix and it is the whole difference.
        let scanned = text.lines().filter(|l| !l.trim().is_empty()).count();
        let detail = if scanned == 0 {
            format!(
                "{} — the file is EMPTY (0 rows). This is a statement about the SOURCE, not about the entity.",
                cfg.console_audit_path
            )
        } else {
            format!(
                "{} — {} of {scanned} row(s) matched this entity",
                cfg.console_audit_path,
                events.len()
            )
        };
        AdapterOutput::ok(SOURCE, &detail, events)
    }
}

/// The console's own words for WHY, or nothing.
///
/// Split out so the mapping is testable without a fixture file. The bug this
/// guards: `detail` was previously fed `result`, so a failed row's detail read
/// "error" — a restatement of the badge beside it, with the one actionable
/// part (the message a real person hit) dropped on the floor.
///
/// Returns `None` rather than a placeholder when the console recorded no
/// message. An empty detail column says "the source did not say"; a fabricated
/// one would say something the source never claimed.
fn error_detail(rec: &serde_json::Value) -> Option<String> {
    rec["error_message"]
        .as_str()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::error_detail;
    use serde_json::json;

    #[test]
    fn the_console_message_survives_to_the_detail_column() {
        let rec = json!({
            "action": "onboard_email_proposed",
            "result": "error",
            "error_message": "smtp send: 554 Message rejected: Email address is not verified.",
        });
        // The verdict word is NOT the detail — the sentence is.
        assert_eq!(
            error_detail(&rec).as_deref(),
            Some("smtp send: 554 Message rejected: Email address is not verified."),
        );
    }

    #[test]
    fn no_message_yields_no_detail_never_a_placeholder() {
        assert_eq!(error_detail(&json!({"result": "refused"})), None);
        assert_eq!(error_detail(&json!({"result": "error", "error_message": ""})), None);
        assert_eq!(error_detail(&json!({"error_message": "   "})), None);
    }
}
