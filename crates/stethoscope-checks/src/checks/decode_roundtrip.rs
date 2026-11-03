// SPDX-License-Identifier: Apache-2.0
//
// `decode-roundtrip` check (MVP-6 check #3) — samples recent payloads
// for a topic and reports the fraction that actually decode against the
// declared schema.
//
// This is the necessary complement to `schema-body`. Schema-body is a
// static check: it proves the `.fds` parses and the message named by
// `format` exists in the descriptor. It does NOT prove that a single
// wire byte conforms to that message. The DRS RCA is the
// motivating case — schema-body went green after the `format` PATCH,
// but `tracks` / `position_attitude` still threw `invalid wire type`
// errors because the bytes were being decoded against the wrong message
// in the descriptor. A check that samples real payloads and counts
// decode successes catches that "descriptor parses but bytes don't
// decode" state read-side, instead of an operator spotting it by eye in
// panoscope. A <50% rate here is what raises the design's
// `schema-doesn't-match-bytes` finding.
//
// Data source (as-built deviation from the design's #3 spec): rather
// than pull audit-tail samples and decode locally via dynamicpb, this
// calls `GET /api/topics/decoded?key=<key>&n=N`, which decodes each
// sample server-side against the topic's declared format — the same
// path the portal viewer + downstream subscribers use. One round-trip,
// and the decode honors `format` uniformly with the rest of the portal.
//
// Verdict logic (thresholds per the MVP-6 catalog):
//   Portal disabled                        → Skipped
//   Topic not registered                   → Skipped (registration owns)
//   Format != protobuf:*                    → Skipped (raw/json render as-is)
//   No recent samples to measure            → Skipped (cardinality/flow own traffic)
//   ≥95% of sampled payloads decode         → Pass
//   50–95% decode                           → Warn (mixed types / drift)
//   <50% decode                             → Fail (raises schema-doesn't-match-bytes)

use async_trait::async_trait;

use panoscope_core::PortalError;

use crate::check::{Check, CheckKind, Cost};
use crate::context::{CheckCtx, Target, TargetKind};
use crate::outcome::{CheckOutcome, Remediation};

pub struct DecodeRoundtripCheck;

const ID: &str = "decode-roundtrip";
const NAME: &str = "Decode-roundtrip";

/// How many recent samples to pull per topic. Matches the portal
/// viewer's default page and the vendor-decode-rca tool's default.
const SAMPLE_N: usize = 20;

/// Pass at ≥95% decoded, Fail below 50% — per the MVP-6 catalog bands.
const PASS_RATE: f64 = 0.95;
const FAIL_RATE: f64 = 0.50;

#[async_trait]
impl Check for DecodeRoundtripCheck {
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
        &[TargetKind::Topic]
    }
    fn description(&self) -> &'static str {
        "Samples recent payloads and reports the fraction that decode against the declared schema — catches the 'descriptor parses but wire bytes don't decode' state that schema-body cannot see."
    }

    async fn run(&self, ctx: &CheckCtx, target: &Target) -> CheckOutcome {
        if !ctx.portal.is_enabled() {
            return CheckOutcome::skipped(ID, "portal client not configured");
        }
        let key = &target.value;
        let vendor = target.vendor_prefix();
        if vendor.is_empty() {
            return CheckOutcome::skipped(
                ID,
                format!("topic target value must be `<{}>/<rest...>`", ctx.org),
            );
        }

        // Only protobuf topics have a decode step worth measuring; json /
        // cbor / raw render as-is regardless of any schema.
        let topic = match ctx.portal.find_topic_by_key(key).await {
            Ok(Some(t)) => t,
            Ok(None) => {
                return CheckOutcome::skipped(
                    ID,
                    "topic not registered — registration check owns this state",
                );
            }
            Err(PortalError::NotFound { .. }) => {
                return CheckOutcome::skipped(
                    ID,
                    format!("{} '{vendor}' not in portal — registration check owns this state", ctx.org_title()),
                );
            }
            Err(e) => return CheckOutcome::skipped(ID, format!("portal error: {e}")),
        };
        if !topic.format.starts_with("protobuf:") {
            return CheckOutcome::skipped(
                ID,
                format!(
                    "Format '{}' renders as-is — no protobuf decode step to measure.",
                    topic.format
                ),
            );
        }

        let samples = match ctx.portal.decoded_samples(key, SAMPLE_N).await {
            Ok(s) => s,
            Err(PortalError::NotFound { .. }) => {
                return CheckOutcome::skipped(ID, "no decoded-samples endpoint result for this key");
            }
            Err(e) => return CheckOutcome::skipped(ID, format!("decoded_samples: {e}")),
        };

        // Only payload-bearing samples count toward the rate; the portal
        // emits neither `decoded` nor `error` for empty samples.
        let measurable: Vec<&panoscope_core::DecodedSample> = samples
            .iter()
            .filter(|s| s.decoded.is_some() || s.error.is_some())
            .collect();
        let total = measurable.len();
        if total == 0 {
            return CheckOutcome::skipped(
                ID,
                "no recent payload-bearing samples to measure — cardinality / flow-rate own the no-traffic state",
            );
        }

        let decoded_ok = measurable.iter().filter(|s| s.is_decoded()).count();
        let first_error = measurable
            .iter()
            .find_map(|s| s.error.as_deref())
            .unwrap_or("(no error string)");

        decode_verdict(decoded_ok, total, &topic.format, first_error, vendor)
    }
}

/// Map a decode success ratio to a verdict using the MVP-6 catalog bands
/// (Pass ≥95% / Warn 50-95% / Fail <50%). Pure — no I/O — so the band
/// boundaries are unit-tested directly without a mock portal. Caller
/// guarantees `total > 0` (the no-samples case is a Skipped verdict
/// raised before this is reached), so the ratio never divides by zero.
fn decode_verdict(
    decoded_ok: usize,
    total: usize,
    format: &str,
    first_error: &str,
    vendor: &str,
) -> CheckOutcome {
    let rate = decoded_ok as f64 / total as f64;
    let pct = (rate * 100.0).round() as u32;
    if rate >= PASS_RATE {
        CheckOutcome::pass(
            ID,
            format!("{decoded_ok}/{total} recent samples ({pct}%) decode cleanly against '{format}'."),
        )
    } else if rate < FAIL_RATE {
        CheckOutcome::fail(
            ID,
            format!(
                "Only {decoded_ok}/{total} recent samples ({pct}%) decode against '{format}'. First error: {first_error}"
            ),
        )
        .with_remediation(decode_remediation(vendor))
    } else {
        CheckOutcome::warn(
            ID,
            format!(
                "{decoded_ok}/{total} recent samples ({pct}%) decode against '{format}' — the rest fail. First error: {first_error}. Usually a publisher emitting more than one message type on one key, or a schema-version drift."
            ),
        )
        .with_remediation(decode_remediation(vendor))
    }
}

fn decode_remediation(vendor: &str) -> Remediation {
    Remediation::new(
        "Diagnose which failure mode is in play",
        format!(
            "Run the vendor-decode-rca tool against this vendor — it probes every schema version × every message in each .fds and classifies the failure (wrong declared message, schema-version drift, envelope wrapping, or mixed message types): `cd ops/zenoh-trial-peers/vendor-decode-rca && go build . && ./vendor-decode-rca {vendor}`. See F-162 + docs/operations/vendor-schema-fixup.md. If the topic recently went green on schema-body but still fails here, confirm the panoscope/portal image carries the F-162 decoder fix."
        ),
    )
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
        let outcome = DecodeRoundtripCheck
            .run(&disabled_ctx(), &Target::topic("drs/foo/bar/v1"))
            .await;
        assert_eq!(outcome.verdict, crate::outcome::Verdict::Skipped);
    }

    #[tokio::test]
    async fn only_applies_to_topics() {
        assert_eq!(DecodeRoundtripCheck.applies_to(), &[TargetKind::Topic]);
    }

    // Band boundaries (Pass ≥95% / Warn 50-95% / Fail <50%). These are
    // the verdicts an operator actually acts on, so pin each edge.
    #[test]
    fn decode_verdict_bands() {
        use crate::outcome::Verdict;
        let v = |ok, total| decode_verdict(ok, total, "protobuf:X", "err", "drs").verdict;
        // Pass: all decode, and exactly the 95% threshold.
        assert_eq!(v(20, 20), Verdict::Pass);
        assert_eq!(v(19, 20), Verdict::Pass); // 95.0% — inclusive lower edge of Pass
        // Warn: below 95% down to (and including) 50%.
        assert_eq!(v(18, 20), Verdict::Warn); // 90%
        assert_eq!(v(10, 20), Verdict::Warn); // 50.0% — inclusive lower edge of Warn
        // Fail: below 50%, including none-decode.
        assert_eq!(v(9, 20), Verdict::Fail); // 45%
        assert_eq!(v(1, 20), Verdict::Fail); // 5%
        assert_eq!(v(0, 20), Verdict::Fail); // 0%
        // Single-sample topics still classify (no div-by-zero; total>0 contract).
        assert_eq!(v(1, 1), Verdict::Pass);
        assert_eq!(v(0, 1), Verdict::Fail);
    }
}
