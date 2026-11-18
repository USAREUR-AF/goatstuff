//! Operator namespace observations through authenticated metadata projection.
//! The key names an asserted actor; it does not authenticate an administrative
//! action. No body is fetched or decrypted. Attribution remains coarse.

use async_trait::async_trait;
use panoscope_core::AuditEvent;

use crate::adapters::{
    audit_max_rows, rows_clause, tail_from, AdapterOutput, NewestRows, SourceAdapter,
    AUDIT_MAX_OBJECTS,
};
use crate::config::Config;
use crate::event::{Attribution, TimelineEvent};
use crate::spine::EntitySpine;

pub struct OperatorAudit;

const SOURCE: &str = "operator-audit";
const OP_PREFIX: &str = "ops/audit/operator/";

/// Offer one `ops` record to the bounded rows if it is an operator action by
/// one of `keys` (lower-cased spine match keys). Everything else in the `ops`
/// namespace is dropped as it streams past.
fn offer_operator(rows: &mut NewestRows, keys: &[String], r: AuditEvent) {
    let Some(actor) = r.key_expr.strip_prefix(OP_PREFIX) else {
        return;
    };
    let actor = actor.split('/').next().unwrap_or(actor).to_string();
    if !keys.iter().any(|k| actor.eq_ignore_ascii_case(k)) {
        return;
    }
    rows.offer(crate::timefmt::canonical_str(&r.ts), move |ts| TimelineEvent {
        ts,
        source: SOURCE,
        action: "operator-namespace-observed".into(),
        actor: actor.clone(),
        target: None,
        summary: format!(
            "observed {} in operator namespace for {} ({} B; actor unverified)",
            if r.sample_kind.is_empty() { "unknown kind" } else { &r.sample_kind },
            actor,
            r.payload_len
        ),
        kind: crate::event::EventKind::Admin,
        outcome: crate::event::Outcome::Unrecorded,
        direction: crate::event::Direction::BySubject,
        detail: None,
        attribution: Attribution::Coarse,
        raw: serde_json::to_value(&r).ok(),
    });
}

#[async_trait]
impl SourceAdapter for OperatorAudit {
    fn name(&self) -> &'static str {
        SOURCE
    }

    async fn collect(&self, cfg: &Config, spine: &EntitySpine) -> AdapterOutput {
        let tail = match tail_from(cfg) {
            Ok(Some(tail)) => tail,
            Ok(None) => return AdapterOutput::skipped(SOURCE, "portal history unconfigured"),
            Err(error) => return AdapterOutput::error(SOURCE, error),
        };
        // The stream's first key segment is `ops`, so one vendor-filtered
        // walk narrows to the ops namespace before the prefix match. Rows are
        // folded as they stream; only the newest matching ones are kept.
        let keys: Vec<String> = spine.match_keys().iter().map(|k| k.to_lowercase()).collect();
        let mut rows = NewestRows::new(audit_max_rows(cfg));
        let stats = match tail
            .stream_recent_events(cfg.audit_window_hours, Some("ops"), |r| {
                offer_operator(&mut rows, &keys, r)
            })
            .await
        {
            Ok(v) => v,
            Err(e) => return AdapterOutput::error(SOURCE, e),
        };
        let partial = stats.objects_fetched >= AUDIT_MAX_OBJECTS;
        let (events, seen) = rows.finish();
        let mut coverage = if partial {
            format!("PARTIAL {}h (object ceiling — older hours uncovered)", cfg.audit_window_hours)
        } else {
            format!("last {}h", cfg.audit_window_hours)
        };
        if let Some(c) = rows_clause(events.len(), seen) {
            coverage.push_str(", ");
            coverage.push_str(&c);
        }
        AdapterOutput::ok(
            SOURCE,
            &format!(
                "operator namespace observations from audit-integrity, {coverage}; actor and operation are not verified by this metadata"
            ),
            events,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::test_audit_event;

    /// The `ops` walk carries every ops stream; only this actor's operator
    /// actions are kept, and at most the ceiling of them.
    #[test]
    fn a_large_ops_window_keeps_at_most_the_ceiling() {
        const N: usize = 200_000;
        const K: usize = 250;
        let keys = vec!["maintainer".to_string()];
        let mut rows = NewestRows::new(K);
        for i in 0..N {
            let key = match i % 4 {
                0 => "ops/audit/operator/maintainer",
                1 => "ops/audit/operator/someone-else",
                2 => "ops/health/mesh/site",
                _ => "ops/audit/operator/MAINTAINER/sub",
            };
            offer_operator(&mut rows, &keys, test_audit_event(key, i));
        }
        let (events, seen) = rows.finish();
        assert_eq!(seen, N / 2, "every matching action is counted");
        assert_eq!(events.len(), K);
        assert!(events.iter().all(|e| e.actor.eq_ignore_ascii_case("maintainer")));
        let oldest_kept = crate::timefmt::canonical_str(&test_audit_event("x", N - 2 * K).ts);
        assert!(events.iter().all(|e| e.ts >= oldest_kept));
    }

    #[test]
    fn a_small_window_is_unchanged() {
        let keys = vec!["maintainer".to_string()];
        let mut rows = NewestRows::new(100);
        offer_operator(&mut rows, &keys, test_audit_event("ops/audit/operator/maintainer", 1));
        offer_operator(&mut rows, &keys, test_audit_event("ops/health/x", 2));
        let (events, seen) = rows.finish();
        assert_eq!((events.len(), seen), (1, 1));
        assert_eq!(events[0].actor, "maintainer");
        assert_eq!(events[0].target, None);
        assert!(events[0].summary.contains("observed put in operator namespace for maintainer"));
        assert!(events[0].summary.contains("actor unverified"));
        assert_eq!(events[0].attribution, Attribution::Coarse);
    }
}
