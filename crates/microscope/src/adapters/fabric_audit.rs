//! Zenoh fabric audit (design source S4): every routed publish, read from
//! the MinIO `audit-integrity` tier (30d GOVERNANCE) via panoscope-core's
//! audit tail — the same read stethoscope's flow-rate check uses. The
//! record's `vendor` field IS the enforced write-anchor (ADR 1025
//! tri-binding), so attribution is strong. Window-bounded; the detail line
//! names the window so "n=0" is never read as "never published".

use async_trait::async_trait;

use crate::adapters::{tail_from, AdapterOutput, SourceAdapter};
use crate::config::Config;
use crate::event::{Attribution, TimelineEvent};
use crate::spine::EntitySpine;

pub struct FabricAudit;

const SOURCE: &str = "fabric-audit";

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
        let mut events = Vec::new();
        let mut partial = false;
        for anchor in &spine.anchors {
            match tail.recent_events(cfg.audit_window_hours, Some(anchor)).await {
                Ok((rows, stats)) => {
                    partial |= stats.objects_fetched >= 5_000;
                    for r in rows {
                        events.push(TimelineEvent {
                            ts: crate::timefmt::canonical_str(&r.ts),
                            source: SOURCE,
                            action: "publish".into(),
                            actor: anchor.clone(),
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
                }
                Err(e) => return AdapterOutput::error(SOURCE, e),
            }
        }
        let detail = if partial {
            format!(
                "audit-integrity tier — PARTIAL: object ceiling hit inside the {}h window; older hours uncovered",
                cfg.audit_window_hours
            )
        } else {
            format!("audit-integrity tier, last {}h window", cfg.audit_window_hours)
        };
        AdapterOutput::ok(SOURCE, &detail, events)
    }
}
