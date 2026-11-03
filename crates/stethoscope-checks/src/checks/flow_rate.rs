// SPDX-License-Identifier: Apache-2.0
//
// `flow-rate` check — counts events in the audit-tail over rolling
// 15-minute and 1-hour windows. A Contributor's design Phase B verdict:
//   Pass    = ≥1 event in last 15m
//   Warn    = events in last 1h but none in last 15m
//   Fail    = no events in last 1h
//   Skipped = audit-tail not configured
//
// Finding attached:
//   declared-but-never-published (severity: warn) — fires on a
//   Topic target when the topic is registered AND zero events have
//   landed. The MVP design gates this on `topic age > 30d`; for
//   Phase B trial-fabric posture we drop the age gate and let
//   operators see the finding immediately on a misconfigured topic.
//   The DRS case is exactly this shape — three topics registered an
//   hour ago, zero events on the registered key_patterns because
//   the publisher's keys live elsewhere.

use async_trait::async_trait;
use chrono::Utc;
use tracing::debug;

use crate::check::{Check, CheckKind, Cost};
use crate::context::{CheckCtx, Target, TargetKind};
use crate::outcome::{CheckOutcome, Finding, FindingSeverity};

pub struct FlowRateCheck;

const ID: &str = "flow-rate";
const NAME: &str = "Flow-rate";
const WINDOW_15M_MS: i64 = 15 * 60 * 1000;
const WINDOW_1H_MS: i64 = 60 * 60 * 1000;

#[async_trait]
impl Check for FlowRateCheck {
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
        Cost::NetworkRead
    }
    fn applies_to(&self) -> &'static [TargetKind] {
        &[TargetKind::Vendor, TargetKind::Topic, TargetKind::System]
    }
    fn description(&self) -> &'static str {
        "Counts events observed in the audit-tail over rolling 15-minute and 1-hour windows."
    }

    async fn run(&self, ctx: &CheckCtx, target: &Target) -> CheckOutcome {
        if !ctx.audit.is_enabled() {
            return CheckOutcome::skipped(ID, "audit-tail not configured");
        }
        let vendor = target.vendor_prefix();
        if vendor.is_empty() {
            return CheckOutcome::skipped(ID, format!("target value missing {} prefix", ctx.org));
        }

        let (events, _stats) = match ctx.audit.recent_events(1, Some(vendor)).await {
            Ok(r) => r,
            Err(e) => {
                debug!(error = %e, "flow-rate: audit_tail recent_events failed");
                return CheckOutcome::skipped(ID, format!("audit-tail error: {e}"));
            }
        };

        let now_ms = Utc::now().timestamp_millis();
        let mut last_15m = 0u64;
        let mut last_1h = 0u64;

        let filter_topic = matches!(target.kind, TargetKind::Topic);
        let filter_system = matches!(target.kind, TargetKind::System);
        let topic_key = &target.value;
        // System target filters by `<vendor>/<system>/...` prefix.
        let system_prefix = if filter_system {
            target.value.clone()
        } else {
            String::new()
        };

        for ev in &events {
            if filter_topic && ev.key_expr != *topic_key {
                continue;
            }
            // Anchor-aware: a system target stays `<vendor>/<system>` while
            // slot-pool wire keys carry a leading UUID anchor.
            if filter_system
                && !ev.key_expr.starts_with(&system_prefix)
                && !panoscope_core::strip_anchor(&ev.key_expr).starts_with(&system_prefix)
            {
                continue;
            }
            let Some(ts) = ev.ts_unix_ms() else { continue };
            let age = now_ms - ts;
            if age <= WINDOW_1H_MS {
                last_1h += 1;
            }
            if age <= WINDOW_15M_MS {
                last_15m += 1;
            }
        }

        let summary = format!(
            "{last_15m} event{} in last 15m · {last_1h} in last 1h.",
            if last_15m == 1 { "" } else { "s" }
        );

        let mut outcome = if last_15m >= 1 {
            CheckOutcome::pass(ID, summary)
        } else if last_1h >= 1 {
            CheckOutcome::warn(ID, summary.clone()).with_remediation(crate::outcome::Remediation::new(
                "Confirm publisher is still active",
                "No events in the last 15 minutes but some in the last hour. Either the publisher's cadence is sparse (expected — confirm), or it stalled recently (check the publisher's process + Zenoh peer status).",
            ))
        } else {
            CheckOutcome::fail(ID, summary.clone())
                .with_remediation(crate::outcome::Remediation::new(
                    "Check the publisher is running",
                    "No events in the last hour. SSH to the publisher's host and confirm the process is up; check Zenoh peer routing on panoscope.",
                ))
                .with_remediation(crate::outcome::Remediation::new(
                    "Check the publisher's cert + ACL",
                    "If the publisher is up, verify its cert is trusted by the Zenoh fabric and its key_expr matches the ACL allow-list — silent drops at ACL look identical to a stopped publisher at this layer.",
                ))
        };

        // declared-but-never-published finding for topic targets that
        // are registered + have zero events in the window.
        if filter_topic && last_1h == 0 && ctx.portal.is_enabled() {
            match ctx.portal.list_topics(vendor).await {
                Ok(topics) => {
                    if topics.iter().any(|t| t.key_pattern == *topic_key) {
                        outcome = outcome.with_finding(Finding::new(
                            "declared-but-never-published",
                            FindingSeverity::Warn,
                            format!(
                                "Topic '{topic_key}' is registered in the portal but no events have been observed on the audit-tail. The publisher may be using a different key_pattern."
                            ),
                        ));
                    }
                }
                Err(e) => debug!(error = %e, "flow-rate: portal list_topics failed for finding cross-ref"),
            }
        }

        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panoscope_core::{AuditTail, AuditTailConfig, PortalClient};

    fn disabled_ctx() -> CheckCtx {
        let portal = PortalClient::new(String::new(), String::new());
        let audit = AuditTail::new(AuditTailConfig::default()).unwrap();
        CheckCtx::new(portal, audit)
    }

    #[tokio::test]
    async fn skipped_when_audit_disabled() {
        let outcome = FlowRateCheck
            .run(&disabled_ctx(), &Target::vendor("drs"))
            .await;
        assert_eq!(outcome.verdict, crate::outcome::Verdict::Skipped);
        assert!(outcome.summary.contains("audit-tail"));
    }

    #[tokio::test]
    async fn check_id_and_metadata_stable() {
        let c = FlowRateCheck;
        assert_eq!(c.id(), "flow-rate");
        assert_eq!(c.kind(), CheckKind::Passive);
        assert!(c.applies_to().contains(&TargetKind::Vendor));
        assert!(c.applies_to().contains(&TargetKind::Topic));
        assert!(c.applies_to().contains(&TargetKind::System));
    }
}
