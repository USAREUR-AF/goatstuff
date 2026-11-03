// SPDX-License-Identifier: Apache-2.0
//
// `topic-roster` check — composite vendor/system check that walks
// each registered child topic, runs schema-body inline, and surfaces
// the per-topic verdict as a clickable Finding chip.
//
// Closes the gap from the DRS RCA: the vendor page used to say "Vendor
// 'drs' exists with 3 topics" and PASS even when every topic underneath
// FAILed schema-body, because the existing `registration` check is
// shallow at vendor level and `schema-body` is topic-only. After this
// check, the vendor card carries one chip per child topic with the
// child's verdict color + an href that drill-downs to /?topic=<key>;
// the roll-up reflects the worst child verdict.
//
// Topic targets get NotApplicable — at topic level the existing
// schema-body / registration / flow-rate checks already own that
// surface; we don't run topic-roster against a single topic because
// there are no children to roll up.

use async_trait::async_trait;

use panoscope_core::PortalError;

use crate::check::{Check, CheckKind, Cost};
use crate::checks::schema_body::SchemaBodyCheck;
use crate::context::{CheckCtx, Target, TargetKind};
use crate::outcome::{CheckOutcome, Finding, FindingSeverity, Remediation, Verdict};

pub struct TopicRosterCheck;

const ID: &str = "topic-roster";
const NAME: &str = "Topic roster";

#[async_trait]
impl Check for TopicRosterCheck {
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
        // Walks one portal call per child topic (schema-body fetches
        // the .fds body once for protobuf:* topics). At Block 90 Phase
        // B scales this is bounded by the registered-topics-per-vendor
        // count, typically O(10).
        Cost::NetworkRead
    }
    fn applies_to(&self) -> &'static [TargetKind] {
        &[TargetKind::Vendor, TargetKind::System]
    }
    fn description(&self) -> &'static str {
        "Walks each registered child topic and surfaces its worst per-topic verdict as a clickable finding so the vendor roll-up reflects child FAILs."
    }

    async fn run(&self, ctx: &CheckCtx, target: &Target) -> CheckOutcome {
        if !ctx.portal.is_enabled() {
            return CheckOutcome::skipped(ID, "portal client not configured");
        }
        let vendor = target.vendor_prefix();
        if vendor.is_empty() {
            return CheckOutcome::skipped(ID, format!("target value missing {} prefix", ctx.org));
        }

        // List + filter children. System target narrows to topics
        // under `<vendor>/<system>/...`.
        let mut topics = match ctx.portal.list_topics(vendor).await {
            Ok(t) => t,
            Err(PortalError::NotFound { .. }) => Vec::new(),
            Err(e) => return CheckOutcome::skipped(ID, format!("portal error: {e}")),
        };
        // Slot-pool goatnets file this vendor's topics under anchor-named
        // vendor rows (`<anchor>/<vendor>/...`, F-199) — walk those children
        // too or the roster reads empty while the topics exist.
        topics.extend(ctx.slot_anchored_topics(vendor).await);

        let prefix_for_system = if matches!(target.kind, TargetKind::System) {
            format!("{}/", target.value)
        } else {
            String::new()
        };
        let matching: Vec<_> = topics
            .iter()
            .filter(|t| {
                prefix_for_system.is_empty()
                    || t.key_pattern.starts_with(&prefix_for_system)
                    || panoscope_core::strip_anchor(&t.key_pattern)
                        .starts_with(&prefix_for_system)
            })
            .collect();

        if matching.is_empty() {
            // Vendor with zero registered topics, or system prefix
            // that doesn't match anything. Nothing to walk — the
            // registration check already flags this shape; we stay
            // silent (Pass on an empty set).
            return CheckOutcome::pass(ID, "No registered child topics to walk.");
        }

        // Run schema-body inline per child. Sequential — at the
        // registered-topics-per-vendor scale (typically ≤ 10) the
        // simpler shape beats a join_all + worktree-clone tax, and
        // the portal already caches the .fds bodies.
        let schema_check = SchemaBodyCheck;
        let mut findings: Vec<Finding> = Vec::new();
        let mut counts_pass = 0usize;
        let mut counts_warn = 0usize;
        let mut counts_fail = 0usize;
        let mut counts_skip = 0usize;
        let mut worst = Verdict::Pass;

        for t in &matching {
            let child_target = Target::topic(t.key_pattern.clone());
            let outcome = schema_check.run(ctx, &child_target).await;

            // Roll up worst-wins.
            if outcome.verdict.severity_rank() > worst.severity_rank() {
                worst = outcome.verdict;
            }

            let (sev, count) = match outcome.verdict {
                Verdict::Fail => (FindingSeverity::Fail, &mut counts_fail),
                Verdict::Warn => (FindingSeverity::Warn, &mut counts_warn),
                Verdict::Pass => (FindingSeverity::Pass, &mut counts_pass),
                Verdict::Skipped | Verdict::NotApplicable => {
                    (FindingSeverity::Info, &mut counts_skip)
                }
            };
            *count += 1;

            // Label: short enough to fit in a chip — chop the slot anchor
            // (if any) + the vendor prefix (already shown by the page
            // chrome) and truncate.
            let stripped = panoscope_core::strip_anchor(&t.key_pattern);
            let after_vendor = stripped
                .strip_prefix(&format!("{vendor}/"))
                .unwrap_or(stripped);
            let label = if after_vendor.len() > 40 {
                format!("{}…", &after_vendor[..40])
            } else {
                after_vendor.to_string()
            };

            let detail = format!(
                "{} — {}",
                outcome.verdict.as_str().to_uppercase(),
                outcome.summary
            );
            let href = format!("/?topic={}", url_safe(&t.key_pattern));
            findings.push(Finding::new(label, sev, detail).with_href(href));
        }

        let summary = format!(
            "{} topic{}: {} FAIL · {} WARN · {} PASS{}.",
            matching.len(),
            if matching.len() == 1 { "" } else { "s" },
            counts_fail,
            counts_warn,
            counts_pass,
            if counts_skip > 0 {
                format!(" · {counts_skip} SKIPPED")
            } else {
                String::new()
            },
        );

        let mut outcome = match worst {
            Verdict::Fail => CheckOutcome::fail(ID, summary),
            Verdict::Warn => CheckOutcome::warn(ID, summary),
            _ => CheckOutcome::pass(ID, summary),
        };

        // Attach a remediation hint when there's a child FAIL — point
        // the operator at the per-topic chip below.
        if counts_fail > 0 {
            outcome = outcome.with_remediation(Remediation::new(
                "Drill into a failing topic",
                "Click any red chip below to open that topic's diagnose page — schema-body there carries the exact fix (message-name mismatch or missing schema body).",
            ));
        }

        for f in findings {
            outcome = outcome.with_finding(f);
        }
        outcome
    }
}

/// Minimal percent-encoder for topic keys — same conservative set as
/// stethoscope/src/render.rs::urlencode (only `/`, `:`, space hit the
/// hot path here). Duplicated rather than re-exported because checks
/// don't depend on the binary crate.
fn url_safe(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(ch),
            other => {
                let mut buf = [0u8; 4];
                for b in other.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("%{:02X}", b));
                }
            }
        }
    }
    out
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
    async fn skipped_when_portal_disabled() {
        let outcome = TopicRosterCheck
            .run(&disabled_ctx(), &Target::vendor("drs"))
            .await;
        assert_eq!(outcome.verdict, Verdict::Skipped);
        assert!(outcome.summary.contains("portal"));
    }

    #[tokio::test]
    async fn metadata_stable() {
        let c = TopicRosterCheck;
        assert_eq!(c.id(), "topic-roster");
        assert_eq!(c.kind(), CheckKind::Passive);
        assert_eq!(c.cost(), Cost::NetworkRead);
        assert!(c.applies_to().contains(&TargetKind::Vendor));
        assert!(c.applies_to().contains(&TargetKind::System));
        assert!(!c.applies_to().contains(&TargetKind::Topic));
    }

    #[test]
    fn url_safe_handles_slashes_and_colons() {
        assert_eq!(
            url_safe("org-1/radar1radar/RADAR-EXAMPLE/tracks"),
            "drs%2Fradar1radar%2FRADAR-EXAMPLE%2Ftracks"
        );
    }
}
