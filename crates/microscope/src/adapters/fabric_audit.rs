//! Zenoh fabric audit (design source S4): every routed publish, read from
//! the MinIO `audit-integrity` tier (30d GOVERNANCE) via panoscope-core's
//! audit tail — the same read stethoscope's flow-rate check uses. The
//! record's `vendor` field IS the enforced write-anchor (ADR 1025
//! tri-binding), so attribution is strong. Window-bounded; the detail line
//! names the window so "n=0" is never read as "never published".
//!
//! Events are folded as the tail streams them and only the newest
//! [`crate::adapters::audit_max_rows`] rows are kept: a busy anchor publishes
//! millions of samples a day, and materialising the whole window first is
//! what exhausted the service's memory. The detail line says when rows were
//! capped and how many matched in all.

use async_trait::async_trait;
use panoscope_core::AuditEvent;

use crate::adapters::{
    audit_max_rows, rows_clause, tail_from, AdapterOutput, NewestRows, SourceAdapter,
    AUDIT_MAX_OBJECTS,
};
use crate::config::Config;
use crate::event::{Attribution, TimelineEvent};
use crate::spine::EntitySpine;

pub struct FabricAudit;

const SOURCE: &str = "fabric-audit";

/// Offer one publish record, attributed to `anchor`, to the bounded rows.
fn offer_publish(rows: &mut NewestRows, anchor: &str, r: AuditEvent) {
    rows.offer(crate::timefmt::canonical_str(&r.ts), move |ts| TimelineEvent {
        ts,
        source: SOURCE,
        action: "publish".into(),
        actor: anchor.to_string(),
        target: Some(r.key_expr.clone()),
        summary: format!("published {} ({} B)", r.key_expr, r.payload_len),
        kind: crate::event::EventKind::classify("fabric.publish"),
        outcome: crate::event::Outcome::Unrecorded,
        direction: crate::event::Direction::BySubject,
        detail: None,
        attribution: Attribution::Strong,
        raw: serde_json::to_value(&r).ok(),
    });
}

#[async_trait]
impl SourceAdapter for FabricAudit {
    fn name(&self) -> &'static str {
        SOURCE
    }

    async fn collect(&self, cfg: &Config, spine: &EntitySpine) -> AdapterOutput {
        let tail = match tail_from(cfg) {
            Ok(Some(t)) => t,
            Ok(None) => return AdapterOutput::skipped(SOURCE, "MICROSCOPE_AUDIT_* unset"),
            Err(e) => return AdapterOutput::error(SOURCE, e),
        };
        if spine.anchors.is_empty() {
            return AdapterOutput::ok(
                SOURCE,
                "no write anchors on the spine — nothing to attribute",
                vec![],
            );
        }
        let mut rows = NewestRows::new(audit_max_rows(cfg));
        let mut partial = false;
        for anchor in &spine.anchors {
            let walked = tail
                .stream_recent_events(cfg.audit_window_hours, Some(anchor), |r| {
                    offer_publish(&mut rows, anchor, r)
                })
                .await;
            match walked {
                Ok(stats) => partial |= stats.objects_fetched >= AUDIT_MAX_OBJECTS,
                Err(e) => return AdapterOutput::error(SOURCE, e),
            }
        }
        let (events, seen) = rows.finish();
        let mut detail = if partial {
            format!(
                "audit-integrity tier — PARTIAL: object ceiling hit inside the {}h window; older hours uncovered",
                cfg.audit_window_hours
            )
        } else {
            format!("audit-integrity tier, last {}h window", cfg.audit_window_hours)
        };
        if let Some(c) = rows_clause(events.len(), seen) {
            detail.push_str("; ");
            detail.push_str(&c);
        }
        AdapterOutput::ok(SOURCE, &detail, events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window far larger than the ceiling keeps at most the ceiling, keeps
    /// the newest rows, and still counts every publish.
    #[test]
    fn a_large_window_keeps_at_most_the_ceiling() {
        const N: usize = 200_000;
        const K: usize = 500;
        let mut rows = NewestRows::new(K);
        for i in 0..N {
            offer_publish(&mut rows, "vendor-a", crate::adapters::test_audit_event("vendor-a/t/v1", i));
        }
        let (events, seen) = rows.finish();
        assert_eq!(seen, N);
        assert_eq!(events.len(), K);
        let newest = crate::timefmt::canonical_str(&crate::adapters::test_audit_event("x", N - 1).ts);
        assert!(events.iter().any(|e| e.ts == newest));
        let oldest_kept = crate::timefmt::canonical_str(&crate::adapters::test_audit_event("x", N - K).ts);
        assert!(events.iter().all(|e| e.ts >= oldest_kept));
        assert!(events.iter().all(|e| e.actor == "vendor-a"));
    }

    /// Under the ceiling every row comes back, shaped as before.
    #[test]
    fn a_small_window_is_unchanged() {
        let mut rows = NewestRows::new(100);
        for i in 0..3 {
            offer_publish(&mut rows, "vendor-a", crate::adapters::test_audit_event("vendor-a/t/v1", i));
        }
        let (events, seen) = rows.finish();
        assert_eq!((events.len(), seen), (3, 3));
        assert_eq!(events[0].summary, "published vendor-a/t/v1 (8 B)");
        assert_eq!(events[0].target.as_deref(), Some("vendor-a/t/v1"));
        assert!(events[0].raw.is_some());
        assert_eq!(rows_clause(events.len(), seen), None);
    }
}
