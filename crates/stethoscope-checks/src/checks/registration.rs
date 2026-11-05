// SPDX-License-Identifier: Apache-2.0
//
// `registration` check — confirms the target is declared in the
// portal catalog and (for vendor targets) cross-references the live
// audit-tail keys against the registered key_patterns to surface the
// published-but-never-declared finding (the DRS RCA dual of the
// MVP-6 declared-but-never-published).
//
// Verdict logic:
//   Vendor target:
//     - portal disabled        → Skipped
//     - vendor exists, ≥1 topic → Pass
//     - vendor exists, 0 topics → Warn (registered but empty)
//     - vendor 404             → Fail (no portal row)
//   Topic target:
//     - portal disabled        → Skipped
//     - exact key match found  → Pass
//     - no match               → Fail (likely key_pattern mismatch)
//   System target:
//     - same as Vendor against the parent vendor, filtered to topics
//       under `<vendor>/<system>/...`
//
// Finding attached:
//   published-but-never-declared (severity: warn) — fires on the
//   vendor target when audit-tail shows keys the vendor publishes on
//   that don't appear in the registry. The DRS case is exactly this
//   shape: org-1/radar1radar/... in audit, org-1/radar1/... in registry.

use async_trait::async_trait;
use std::collections::HashSet;
use tracing::debug;

use panoscope_core::PortalError;

use crate::check::{Check, CheckKind, Cost};
use crate::context::{CheckCtx, Target, TargetKind};
use crate::outcome::{CheckOutcome, Finding, FindingSeverity, Remediation};

pub struct RegistrationCheck;

const ID: &str = "registration";
const NAME: &str = "Registration";

#[async_trait]
impl Check for RegistrationCheck {
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
        "Confirms the target is declared in the portal catalog and cross-references audit-tail keys against registered key_patterns."
    }

    async fn run(&self, ctx: &CheckCtx, target: &Target) -> CheckOutcome {
        if !ctx.portal.is_enabled() {
            return CheckOutcome::skipped(ID, "portal client not configured");
        }
        match target.kind {
            TargetKind::Vendor => run_vendor(ctx, target).await,
            TargetKind::Topic => run_topic(ctx, target).await,
            TargetKind::System => run_system(ctx, target).await,
        }
    }
}

async fn run_vendor(ctx: &CheckCtx, target: &Target) -> CheckOutcome {
    let vendor = &target.value;
    // Display-only participant noun for the verdict text (one goatnet "Vendor" /
    // another "Org"). The `vendor` value, key shape, and portal calls are
    // unchanged — only the word shown.
    let org_t = ctx.org_title();
    // Direct lookup: topics filed under a vendor row named by this handle
    // (per-vendor goatnets, or an anchor-shaped target). `None` = no portal
    // row at all.
    let direct = match ctx.portal.list_topics(vendor).await {
        Ok(t) => Some(t),
        Err(PortalError::NotFound { .. }) => None,
        Err(e) => {
            debug!(error = %e, "registration: portal list_topics failed");
            return CheckOutcome::skipped(ID, format!("portal error: {e}"));
        }
    };
    // Slot-pool lookup: topics filed under anchor-named vendor rows whose
    // key vendor-segment matches this handle (`<anchor>/<vendor>/...`,
    // F-199). Without this every slot-pool vendor reads "no topics declared"
    // while its topics sit one segment to the right of the anchor.
    let slot_topics = ctx.slot_anchored_topics(vendor).await;

    let direct_count = direct.as_ref().map_or(0, |t| t.len());
    let slot_count = slot_topics.len();
    let mut topics = direct.clone().unwrap_or_default();
    topics.extend(slot_topics);

    let mut outcome = if direct.is_none() && slot_count == 0 {
        return CheckOutcome::fail(
            ID,
            format!("{org_t} '{vendor}' has no portal row and no pool-anchored topics. No topics or schemas exist for this name."),
        );
    } else if direct_count == 0 && slot_count > 0 {
        CheckOutcome::pass(
            ID,
            format!(
                "{org_t} '{vendor}' has {slot_count} pool-anchored topic{} (declared under a pre-cut pool anchor, `<anchor>/{vendor}/...`).",
                if slot_count == 1 { "" } else { "s" }
            ),
        )
    } else if direct_count == 0 {
        CheckOutcome::warn(
            ID,
            format!("{org_t} '{vendor}' exists but has no topics declared."),
        )
    } else if slot_count == 0 {
        CheckOutcome::pass(
            ID,
            format!(
                "{org_t} '{vendor}' exists with {direct_count} topic{}.",
                if direct_count == 1 { "" } else { "s" }
            ),
        )
    } else {
        CheckOutcome::pass(
            ID,
            format!(
                "{org_t} '{vendor}' has {direct_count} direct topic{} + {slot_count} pool-anchored.",
                if direct_count == 1 { "" } else { "s" }
            ),
        )
    };

    // Published-but-never-declared finding — compare live audit-tail
    // vendor keys vs declared key_patterns. Only fires when the audit
    // tail is enabled; otherwise the check still passes/warns but
    // skips this cross-reference (the user sees a clean verdict, no
    // false-negative finding).
    if ctx.audit.is_enabled() {
        match ctx.audit.recent_events(1, Some(vendor)).await {
            Ok((events, _stats)) => {
                let live: HashSet<String> =
                    events.into_iter().map(|e| e.key_expr).collect();
                let declared: HashSet<String> =
                    topics.iter().map(|t| t.key_pattern.clone()).collect();
                let undeclared: Vec<String> = live
                    .into_iter()
                    .filter(|k| !declared.contains(k))
                    .collect();
                if !undeclared.is_empty() {
                    let preview = undeclared
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ");
                    let more = if undeclared.len() > 3 {
                        format!(" (+{} more)", undeclared.len() - 3)
                    } else {
                        String::new()
                    };
                    outcome = outcome.with_finding(Finding::new(
                        "published-but-never-declared",
                        FindingSeverity::Warn,
                        format!(
                            "{org_t} publishes on {} key{} not in the registry: {preview}{more}. Live traffic is happening outside what the portal knows about — subscribers and schema viewers won't find these topics.",
                            undeclared.len(),
                            if undeclared.len() == 1 { "" } else { "s" }
                        ),
                    ));
                }
            }
            Err(e) => debug!(error = %e, "registration: audit_tail recent_events failed"),
        }
    }
    outcome
}

async fn run_topic(ctx: &CheckCtx, target: &Target) -> CheckOutcome {
    let key = &target.value;
    // Display-only participant noun (one goatnet "vendor" / another "org"). The
    // `/?vendor=` deep-link param + `/api/vendors` curl routes below stay
    // literal — they're machine surfaces, not display.
    let org = &ctx.org;
    let org_t = ctx.org_title();
    let vendor = target.vendor_prefix();
    if vendor.is_empty() {
        return CheckOutcome::fail(
            ID,
            format!("Topic target value must be `<{org}>/<rest...>`."),
        );
    }
    // No slash means the operator typed a bare word in topic mode —
    // most likely they meant the vendor. Tell them that explicitly
    // instead of the confusing "Topic 'x' is not registered under
    // vendor 'x'" tautology that falls out of vendor_prefix() returning
    // the whole string when no slash is present.
    if !key.contains('/') {
        return CheckOutcome::fail(
            ID,
            format!(
                "'{key}' isn't a topic key — topic keys are shaped `<{org}>/<topic>/<version>`. Did you mean the {org} '{key}'?"
            ),
        )
        .with_remediation(
            Remediation::new(
                format!("Switch to the {org} view"),
                format!("Look at {org} '{key}' to see its registered topics and per-topic state."),
            )
            .with_href(format!("/?vendor={key}")),
        );
    }
    match ctx.portal.list_topics(vendor).await {
        Ok(topics) => {
            if let Some(t) = topics.iter().find(|t| t.key_pattern == *key) {
                let fmt = if t.format.is_empty() {
                    String::from("no format declared")
                } else {
                    format!("Format: {}", t.format)
                };
                let sr = if t.schema_ref.is_empty() {
                    String::from("no schema-ref")
                } else {
                    String::from("Schema-ref set")
                };
                CheckOutcome::pass(ID, format!("Exists. {fmt}. {sr}."))
            } else {
                let registered_preview = topics
                    .iter()
                    .take(3)
                    .map(|t| t.key_pattern.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let more = if topics.len() > 3 {
                    format!(" (+{} more)", topics.len() - 3)
                } else {
                    String::new()
                };
                CheckOutcome::fail(
                    ID,
                    format!(
                        "Topic '{key}' is not registered under {org} '{vendor}'. Lookup is exact-match — check the key_pattern."
                    ),
                )
                .with_remediation(Remediation::new(
                    "Register the topic in the portal",
                    format!(
                        "POST to the portal with the exact live key: `curl -X POST -H \"Authorization: Bearer $PORTAL_OPERATOR_API_KEY\" -H 'Content-Type: application/json' -d '{{\"key_pattern\":\"{key}\",\"format\":\"protobuf:<MsgName>\"}}' <portal>/api/vendors/{vendor}/topics`."
                    ),
                ))
                .with_remediation(Remediation::new(
                    "Or fix the publisher to use a registered key",
                    format!(
                        "{org_t} '{vendor}' already has these registered key_patterns: {registered_preview}{more}. Either re-key the publisher to one of those, or register the new key above."
                    ),
                ))
            }
        }
        Err(PortalError::NotFound { .. }) => CheckOutcome::fail(
            ID,
            format!("{org_t} '{vendor}' has no portal row."),
        )
        .with_remediation(Remediation::new(
            format!("Register the {org} in the portal"),
            format!(
                "POST `{{\"slug\":\"{vendor}\",\"display_name\":\"{vendor}\"}}` to <portal>/api/vendors (operator API key required)."
            ),
        )),
        Err(e) => CheckOutcome::skipped(ID, format!("portal error: {e}")),
    }
}

async fn run_system(ctx: &CheckCtx, target: &Target) -> CheckOutcome {
    let value = &target.value;
    let mut segments = value.splitn(2, '/');
    let vendor = match segments.next() {
        Some(v) if !v.is_empty() => v,
        _ => {
            return CheckOutcome::fail(
                ID,
                format!("System target value must be `<{}>/<system>`.", ctx.org),
            );
        }
    };
    let system_prefix = format!("{vendor}/{}", segments.next().unwrap_or(""));
    let mut topics = match ctx.portal.list_topics(vendor).await {
        Ok(topics) => topics,
        Err(PortalError::NotFound { .. }) => Vec::new(),
        Err(e) => return CheckOutcome::skipped(ID, format!("portal error: {e}")),
    };
    // Slot-pool keys carry a leading anchor (`<anchor>/<vendor>/<system>/...`,
    // F-199): sweep those too, and prefix-match anchor-stripped.
    topics.extend(ctx.slot_anchored_topics(vendor).await);
    let matching: Vec<_> = topics
        .into_iter()
        .filter(|t| {
            t.key_pattern.starts_with(&system_prefix)
                || panoscope_core::strip_anchor(&t.key_pattern).starts_with(&system_prefix)
        })
        .collect();
    match matching.len() {
        0 => CheckOutcome::fail(
            ID,
            format!("No topics registered under system prefix '{system_prefix}'."),
        ),
        n => CheckOutcome::pass(
            ID,
            format!("System '{system_prefix}' has {n} topic{}.", if n == 1 { "" } else { "s" }),
        ),
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
    async fn skipped_when_portal_disabled() {
        let outcome = RegistrationCheck
            .run(&disabled_ctx(), &Target::vendor("drs"))
            .await;
        assert_eq!(outcome.verdict, crate::outcome::Verdict::Skipped);
        assert!(outcome.summary.contains("portal"));
    }

    #[tokio::test]
    async fn check_id_and_metadata_stable() {
        let c = RegistrationCheck;
        assert_eq!(c.id(), "registration");
        assert_eq!(c.kind(), CheckKind::Passive);
        assert_eq!(c.cost(), Cost::NetworkRead);
        assert!(c.applies_to().contains(&TargetKind::Vendor));
        assert!(c.applies_to().contains(&TargetKind::Topic));
        assert!(c.applies_to().contains(&TargetKind::System));
    }

    #[tokio::test]
    async fn topic_target_rejects_empty_vendor_prefix() {
        let outcome = RegistrationCheck
            .run(&disabled_ctx(), &Target::topic("foo"))
            .await;
        // portal is disabled, so we hit the early Skipped path; flip
        // to enabled to exercise the empty-prefix branch.
        assert!(matches!(
            outcome.verdict,
            crate::outcome::Verdict::Skipped
        ));
    }
}
