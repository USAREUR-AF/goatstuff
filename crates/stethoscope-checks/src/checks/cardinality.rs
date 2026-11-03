// SPDX-License-Identifier: Apache-2.0
//
// `cardinality` check — vendor / system composite that reads the
// audit-tail's last 24 h of published keys under the target, computes
// (distinct_topics, total_events, median_events_per_topic), and warns
// when the ratio matches the fan-out anti-pattern:
//
//     many distinct topics  +  ~1 event per topic
//   = each topic exists to carry a single message
//
// Per `docs/design/pubsub-best-practices.md` §60 ("One topic per
// logical entity, stable key per entity"), bounded per-entity fan-out
// is fine — a long-lived entity publishing many updates on its stable
// key is correct. Per-event fan-out (each message creates a new
// topic) is the failure mode this check surfaces: thousands of
// `<vendor>/entities/v3/<uuid>` topics that each carry very few samples
// before the UUID rotates.
//
// Verdict shape:
//   audit-tail disabled          → Skipped (config gap)
//   0 events observed             → Pass (no data to judge; silent
//                                   vendor surfaces as flow-rate FAIL
//                                   which owns that signal)
//   ≤ FAN_OUT_TOPIC_FLOOR distinct → Pass with cardinality note
//   median events_per_topic > 1   → Pass with cardinality note
//   median events_per_topic ≤ 1   → Warn (fan-out signal)
//
// Remediation always points the operator at the canon design doc + a
// short script of what to consider:
//   - if entities are short-lived (per-event keys), consolidate the
//     discriminator into the payload and publish on one stable key.
//   - if entities are long-lived but new (legitimate per-entity fan-
//     out), confirm the registration covers the key shape and the
//     lookback just hasn't caught update samples yet — re-run after
//     the publisher has been live a while.

use std::collections::HashMap;

use async_trait::async_trait;
use tracing::debug;

use crate::check::{Check, CheckKind, Cost};
use crate::context::{CheckCtx, Target, TargetKind};
use crate::outcome::{CheckOutcome, Remediation};

pub struct CardinalityCheck;

const ID: &str = "cardinality";
const NAME: &str = "Cardinality";
const WINDOW_HOURS: u32 = 24;
/// Below this number of distinct topics, fan-out isn't worth flagging
/// (a vendor with 3 topics each carrying a single sample is just a
/// quiet vendor, not a fan-out problem).
const FAN_OUT_TOPIC_FLOOR: usize = 20;

#[async_trait]
impl Check for CardinalityCheck {
    fn id(&self) -> &'static str {
        ID
    }
    fn name(&self) -> &'static str {
        NAME
    }
    fn kind(&self) -> CheckKind {
        CheckKind::Passive
    }
    fn cost(&self) -> Cost {
        // Walks 24h of audit-tail rows for the vendor — one List +
        // GetObject per hour-prefix, capped by AuditTail's max_objects.
        Cost::NetworkRead
    }
    fn applies_to(&self) -> &'static [TargetKind] {
        &[TargetKind::Vendor, TargetKind::System]
    }
    fn description(&self) -> &'static str {
        "Reads the last 24h of audit-tail events under this target and warns when topics each carry ~1 message (the fan-out anti-pattern per pubsub-best-practices §60)."
    }

    async fn run(&self, ctx: &CheckCtx, target: &Target) -> CheckOutcome {
        if !ctx.audit.is_enabled() {
            return CheckOutcome::skipped(ID, "audit-tail not configured");
        }
        let vendor = target.vendor_prefix();
        if vendor.is_empty() {
            return CheckOutcome::skipped(ID, format!("target value missing {} prefix", ctx.org));
        }

        let (events, _stats) =
            match ctx.audit.recent_events(WINDOW_HOURS, Some(vendor)).await {
                Ok(r) => r,
                Err(e) => {
                    debug!(error = %e, "cardinality: audit_tail recent_events failed");
                    return CheckOutcome::skipped(ID, format!("audit-tail error: {e}"));
                }
            };

        // Narrow to the system prefix for system targets.
        let system_prefix = if matches!(target.kind, TargetKind::System) {
            format!("{}/", target.value)
        } else {
            String::new()
        };

        let mut counts: HashMap<&str, usize> = HashMap::new();
        for ev in &events {
            // Anchor-aware: a system target stays `<vendor>/<system>` while
            // slot-pool wire keys carry a leading UUID anchor.
            if !system_prefix.is_empty()
                && !ev.key_expr.starts_with(&system_prefix)
                && !panoscope_core::strip_anchor(&ev.key_expr).starts_with(&system_prefix)
            {
                continue;
            }
            *counts.entry(ev.key_expr.as_str()).or_insert(0) += 1;
        }

        let distinct = counts.len();
        let total: usize = counts.values().sum();

        if total == 0 {
            return CheckOutcome::pass(
                ID,
                format!("0 events in the last {WINDOW_HOURS}h — flow-rate owns the silent-publisher signal."),
            );
        }

        let median = median_count(counts.values().copied().collect::<Vec<_>>());
        let summary = format!(
            "{distinct} distinct topic{} · {total} event{} in last {WINDOW_HOURS}h · ~{median} event{}/topic (median).",
            if distinct == 1 { "" } else { "s" },
            if total == 1 { "" } else { "s" },
            if median == 1 { "" } else { "s" },
        );

        if distinct <= FAN_OUT_TOPIC_FLOOR {
            return CheckOutcome::pass(
                ID,
                format!("{summary} Below the fan-out floor of {FAN_OUT_TOPIC_FLOOR} distinct topics."),
            );
        }
        if median > 1 {
            return CheckOutcome::pass(
                ID,
                format!("{summary} Each topic is carrying multiple events — bounded per-entity fan-out, healthy shape."),
            );
        }

        // Fan-out signal. WARN + remediation pointing at canon.
        CheckOutcome::warn(
            ID,
            format!("{summary} Most topics carry ~1 event — the fan-out anti-pattern (each topic exists to carry one message)."),
        )
        .with_remediation(Remediation::new(
            "If per-event keys are accidental, consolidate",
            "Put the discriminator (UUID, event-id, version-bump) in the payload and publish on one stable key per long-lived entity. See `docs/design/pubsub-best-practices.md` §60 — 'one topic per logical entity, stable key per entity.'",
        ))
        .with_remediation(Remediation::new(
            "If per-entity fan-out is deliberate, confirm",
            "Long-lived entities with stable per-entity keys are correct. Re-run this check after the publisher has been live a while — the lookback may not yet have caught update samples. If entities really are short-lived, document the cardinality in the portal so operators don't surprise-page on it.",
        ))
    }
}

/// Median of an unsorted Vec<usize>. Empty → 0.
fn median_count(mut xs: Vec<usize>) -> usize {
    if xs.is_empty() {
        return 0;
    }
    xs.sort_unstable();
    xs[xs.len() / 2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::Check;
    use panoscope_core::{AuditTail, AuditTailConfig, PortalClient};

    fn disabled_ctx() -> CheckCtx {
        let portal = PortalClient::new(String::new(), String::new());
        let audit = AuditTail::new(AuditTailConfig::default()).unwrap();
        CheckCtx::new(portal, audit)
    }

    #[tokio::test]
    async fn skipped_when_audit_disabled() {
        let outcome = CardinalityCheck
            .run(&disabled_ctx(), &Target::vendor("vendor-c"))
            .await;
        assert_eq!(outcome.verdict, crate::outcome::Verdict::Skipped);
        assert!(outcome.summary.contains("audit-tail"));
    }

    #[tokio::test]
    async fn metadata_stable() {
        let c = CardinalityCheck;
        assert_eq!(c.id(), "cardinality");
        assert_eq!(c.kind(), CheckKind::Passive);
        assert_eq!(c.cost(), Cost::NetworkRead);
        assert!(c.applies_to().contains(&TargetKind::Vendor));
        assert!(c.applies_to().contains(&TargetKind::System));
        assert!(!c.applies_to().contains(&TargetKind::Topic));
    }

    #[test]
    fn median_count_basic() {
        assert_eq!(median_count(vec![]), 0);
        assert_eq!(median_count(vec![5]), 5);
        assert_eq!(median_count(vec![1, 2, 3]), 2);
        // even length — picks the upper-middle, fine for this use.
        assert_eq!(median_count(vec![1, 1, 2, 2]), 2);
        // fan-out shape: many 1s, one outlier.
        let xs = vec![1; 100];
        assert_eq!(median_count(xs), 1);
    }
}
