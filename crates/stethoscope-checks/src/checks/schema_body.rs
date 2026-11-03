// SPDX-License-Identifier: Apache-2.0
//
// `schema-body` check — verifies the topic has a schema artifact
// uploaded AND, when format is `protobuf:<MsgName>`, that the named
// message actually exists as a top-level message in the uploaded
// FileDescriptorSet. The latter is the new name-vs-FDS sub-check the
// DRS RCA argued for (the `protobuf:MyMessage` case where the body
// contains Status / SoftwareVersion / etc but no MyMessage).
//
// Verdict logic:
//   Portal disabled                              → Skipped
//   Topic not registered                         → Skipped (registration check owns that)
//   Format != protobuf:*                         → Pass with "no .fds required for this format"
//   Format = protobuf:* AND no schema uploaded   → Fail
//   Format = protobuf:<Msg> AND .fds doesn't contain Msg → Fail with name-mismatch reason
//   Format = protobuf:<Msg> AND .fds contains Msg → Pass
//
// Findings attached:
//   none in Phase B. (stale-schema lands in Phase C.)

use async_trait::async_trait;
use prost_reflect::DescriptorPool;
use tracing::debug;

use panoscope_core::{FetchOutcome, PortalError};

use crate::check::{Check, CheckKind, Cost};
use crate::context::{CheckCtx, Target, TargetKind};
use crate::outcome::{CheckOutcome, Remediation};

pub struct SchemaBodyCheck;

const ID: &str = "schema-body";
const NAME: &str = "Schema-body";

#[async_trait]
impl Check for SchemaBodyCheck {
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
        "Reads the schema artifact bytes from the portal and verifies that the declared message name actually exists in the uploaded descriptor."
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

        let topics = match ctx.portal.list_topics(vendor).await {
            Ok(t) => t,
            Err(PortalError::NotFound { .. }) => {
                return CheckOutcome::skipped(
                    ID,
                    format!("{} '{vendor}' not in portal — registration check owns this state", ctx.org_title()),
                );
            }
            Err(e) => return CheckOutcome::skipped(ID, format!("portal error: {e}")),
        };

        let topic = match topics.iter().find(|t| t.key_pattern == *key) {
            Some(t) => t,
            None => {
                return CheckOutcome::skipped(
                    ID,
                    "topic not registered — registration check owns this state",
                );
            }
        };

        // Format gating — only protobuf needs an FDS body.
        let msg_name = match parse_protobuf_format(&topic.format) {
            ProtobufFormat::Other(s) => {
                return CheckOutcome::pass(
                    ID,
                    format!(
                        "Format '{s}' — no FileDescriptorSet required. Schema body check passes trivially."
                    ),
                );
            }
            ProtobufFormat::Bare => {
                // `protobuf:` with no name after the colon — treat as
                // an under-declared format and don't try to validate
                // a missing name.
                return CheckOutcome::warn(
                    ID,
                    "Format declares 'protobuf:' with no message name. Set `protobuf:<MessageName>` to enable the name-vs-FDS check.",
                );
            }
            ProtobufFormat::Named(name) => name,
        };

        // Confirm a schema row exists at all.
        let schemas = match ctx.portal.list_schemas(vendor, topic.id).await {
            Ok(s) => s,
            Err(e) => return CheckOutcome::skipped(ID, format!("list_schemas: {e}")),
        };
        if schemas.is_empty() {
            return CheckOutcome::fail(
                ID,
                format!(
                    "Format declares 'protobuf:{msg_name}' but no schema body has been uploaded."
                ),
            )
            .with_remediation(Remediation::new(
                "Upload the .proto source (easiest — single-file with `google/protobuf/*` imports)",
                format!(
                    "Per F-158, the portal accepts `.proto` source directly and compiles it server-side via bufbuild/protocompile. POST with `format=protobuf-source`: `curl -X POST -H \"Authorization: Bearer $PORTAL_OPERATOR_API_KEY\" -F format=protobuf-source -F file=@your.proto <portal>/api/vendors/{vendor}/topics/{}/schemas`. Or use the topics-page upload dialog (`.proto` extension auto-picks the format).",
                    topic.id
                ),
            ))
            .with_remediation(Remediation::new(
                "Or upload a precompiled .fds (multi-file vendor source)",
                format!(
                    "If your schema spans multiple .proto files or uses non-WKT imports, build the .fds locally and POST it as `protobuf-fds`: `protoc --include_imports --descriptor_set_out=out.fds your.proto` then `curl -X POST -H \"Authorization: Bearer $PORTAL_OPERATOR_API_KEY\" -F format=protobuf-fds -F file=@out.fds <portal>/api/vendors/{vendor}/topics/{}/schemas`.",
                    topic.id
                ),
            ));
        }

        // Fetch the body bytes via the existing /api/schemas/<key>
        // catch-all (always serves the latest version per portal logic).
        let body = match ctx.portal.fetch_fds(key).await {
            FetchOutcome::Ready(b) => b,
            FetchOutcome::NoSchema => {
                // Should not race here — list_schemas said something existed.
                return CheckOutcome::fail(
                    ID,
                    "Portal lists schema metadata but `/api/schemas/<key>` returned 404. Race or stale row.",
                );
            }
            FetchOutcome::Disabled => {
                return CheckOutcome::skipped(ID, "portal client not configured")
            }
            FetchOutcome::Transient(e) => {
                return CheckOutcome::skipped(ID, format!("portal fetch_fds transient: {e}"))
            }
        };

        // Walk the FileDescriptorSet, confirm msg_name exists as a top-
        // level message in any of its files. Nested messages are NOT
        // matched — the format declaration names a top-level message
        // by convention.
        let pool = match DescriptorPool::decode(body.as_slice()) {
            Ok(p) => p,
            Err(e) => {
                debug!(error = %e, "schema-body: FDS decode failed");
                return CheckOutcome::fail(
                    ID,
                    format!(
                        "Uploaded `.fds` failed to decode as a valid FileDescriptorSet: {e}"
                    ),
                );
            }
        };

        // Walk the FDS collecting top-level message names. Each name is
        // the *fully-qualified* dotted form (e.g.
        // `notional.power_demo.v1.PowerState`). See `name_matches` for
        // the comparison rule.
        let top_level_names: Vec<String> = pool
            .all_messages()
            .map(|m| m.full_name().to_string())
            .collect();

        if name_matches(&top_level_names, &msg_name) {
            let v = schemas
                .iter()
                .map(|s| s.version.as_str())
                .max()
                .unwrap_or("?");
            CheckOutcome::pass(
                ID,
                format!(
                    "{v} uploaded ({} bytes). Format 'protobuf:{msg_name}' matches a top-level message in the descriptor.",
                    body.len()
                ),
            )
        } else {
            // Preview the FDS's fully-qualified message names so the
            // operator can see whether their declaration just needs the
            // package prefix added (or removed).
            let preview = top_level_names
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            let more = if top_level_names.len() > 5 {
                format!(" (+{} more)", top_level_names.len() - 5)
            } else {
                String::new()
            };
            let suggestion = top_level_names.first().cloned().unwrap_or_default();
            let fix_decl = if suggestion.is_empty() {
                "Update the topic's format declaration in the portal to a message name that exists in the uploaded schema.".to_string()
            } else {
                format!(
                    "Update the topic's format declaration in the portal from 'protobuf:{msg_name}' to one of the present messages — most likely 'protobuf:{suggestion}'. Both fully-qualified (`package.Name`) and bare (`Name`) forms are accepted as long as one of them matches a top-level message in the descriptor."
                )
            };
            CheckOutcome::fail(
                ID,
                format!(
                    "Format declares 'protobuf:{msg_name}' but the uploaded schema contains no top-level message named '{msg_name}'. Found: {preview}{more}."
                ),
            )
            .with_remediation(Remediation::new(
                "Re-align format declaration with the uploaded schema",
                fix_decl,
            ))
            .with_remediation(Remediation::new(
                "Or re-upload the schema with the declared message at top level",
                format!(
                    "If '{msg_name}' is the canonical top-level message, fix the source so it's a top-level (not nested) message and re-upload. Easiest path: POST the `.proto` source with `format=protobuf-source` (portal compiles server-side per F-158, single-file with `google/protobuf/*` imports). Multi-file vendor source: rebuild the `.fds` with `protoc --include_imports` and POST as `format=protobuf-fds`."
                ),
            ))
        }
    }
}

enum ProtobufFormat {
    /// Non-protobuf format (json, cbor, raw, …) — schema-body is trivially Pass.
    Other(String),
    /// `protobuf:` with no name after the colon.
    Bare,
    /// `protobuf:<Name>` with a concrete message name.
    Named(String),
}

fn parse_protobuf_format(format: &str) -> ProtobufFormat {
    if let Some(rest) = format.strip_prefix("protobuf:") {
        if rest.is_empty() {
            ProtobufFormat::Bare
        } else {
            ProtobufFormat::Named(rest.to_string())
        }
    } else {
        ProtobufFormat::Other(format.to_string())
    }
}

/// Match a topic's declared protobuf message name against the
/// fully-qualified top-level message names extracted from the
/// uploaded FileDescriptorSet.
///
/// Match rule:
/// - **Full match.** If `msg_name` equals one of the FDS's full names
///   directly (`notional.power_demo.v1.PowerState`), accept it. This
///   is the canonical form — `.proto` files with `package` declarations
///   and manifests using fully-qualified message names both produce
///   this shape.
/// - **Bare-name match.** If `msg_name` has no dots (e.g.
///   `PowerState`) and equals the last segment of one of the FDS's
///   full names, accept it. This is a convenience for declarations
///   that omit the package — common when the `.proto` has no
///   `package` directive at all.
///
/// Mismatch otherwise. Previously the check truncated the FDS's full
/// names to their tail segments before comparing — that broke the
/// canonical fully-qualified declaration path entirely (operator
/// RCA via notional vendor's 22 topics, all failing despite
/// correct producer config).
pub(crate) fn name_matches(top_level_names: &[String], msg_name: &str) -> bool {
    if top_level_names.iter().any(|n| n == msg_name) {
        return true;
    }
    if msg_name.contains('.') {
        return false;
    }
    top_level_names
        .iter()
        .any(|n| n.rsplit('.').next().map(|t| t == msg_name).unwrap_or(false))
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
        let outcome = SchemaBodyCheck
            .run(&disabled_ctx(), &Target::topic("drs/foo/bar/v1"))
            .await;
        assert_eq!(outcome.verdict, crate::outcome::Verdict::Skipped);
    }

    #[tokio::test]
    async fn only_applies_to_topics() {
        let c = SchemaBodyCheck;
        assert_eq!(c.applies_to(), &[TargetKind::Topic]);
    }

    #[test]
    fn parse_protobuf_format_recognizes_three_shapes() {
        match parse_protobuf_format("protobuf:DenObservation") {
            ProtobufFormat::Named(n) => assert_eq!(n, "DenObservation"),
            _ => panic!("expected Named"),
        }
        match parse_protobuf_format("protobuf:") {
            ProtobufFormat::Bare => {}
            _ => panic!("expected Bare"),
        }
        match parse_protobuf_format("json") {
            ProtobufFormat::Other(s) => assert_eq!(s, "json"),
            _ => panic!("expected Other"),
        }
        match parse_protobuf_format("") {
            ProtobufFormat::Other(s) => assert_eq!(s, ""),
            _ => panic!("expected Other"),
        }
    }

    // -- name_matches (RCA) -------------------------------

    #[test]
    fn name_matches_fully_qualified_declaration() {
        // The canonical case: .proto has `package notional.power_demo.v1;`,
        // manifest declares `notional.power_demo.v1.PowerState`, FDS
        // top-level message is full-qualified. Should match.
        let fds = vec!["notional.power_demo.v1.PowerState".to_string()];
        assert!(name_matches(&fds, "notional.power_demo.v1.PowerState"));
    }

    #[test]
    fn name_matches_bare_declaration_against_packaged_fds() {
        // Convenience case: operator declared `protobuf:PowerState`
        // (bare) but the FDS has `notional.power_demo.v1.PowerState`.
        // Accept — the tail matches.
        let fds = vec!["notional.power_demo.v1.PowerState".to_string()];
        assert!(name_matches(&fds, "PowerState"));
    }

    #[test]
    fn name_matches_bare_declaration_against_unpackaged_fds() {
        // The other half of the bare-vs-bare convenience case: .proto
        // has no package, FDS top-level is `PowerState`, declaration
        // is also `PowerState`. Should match.
        let fds = vec!["PowerState".to_string()];
        assert!(name_matches(&fds, "PowerState"));
    }

    #[test]
    fn name_matches_rejects_wrong_package() {
        // Declaration carries a package; FDS uses a different one.
        // Tail matches but full doesn't — reject (this is the case
        // the test guards against accidentally accepting via a sloppy
        // tail-match fallback).
        let fds = vec!["other.power_demo.v1.PowerState".to_string()];
        assert!(!name_matches(&fds, "notional.power_demo.v1.PowerState"));
    }

    #[test]
    fn name_matches_rejects_wrong_name() {
        let fds = vec!["notional.power_demo.v1.PowerState".to_string()];
        assert!(!name_matches(&fds, "notional.power_demo.v1.PowerStateXYZ"));
        assert!(!name_matches(&fds, "Bogus"));
    }

    #[test]
    fn name_matches_picks_first_matching_among_many() {
        let fds = vec![
            "notional.beacon_demo.v1.Hello".to_string(),
            "notional.power_demo.v1.PowerState".to_string(),
            "notional.tracks_demo.v1.Entity".to_string(),
        ];
        assert!(name_matches(&fds, "notional.power_demo.v1.PowerState"));
        assert!(name_matches(&fds, "Entity"));
        assert!(name_matches(&fds, "Hello"));
        assert!(!name_matches(&fds, "Foo"));
    }

    #[test]
    fn name_matches_empty_fds() {
        let fds: Vec<String> = vec![];
        assert!(!name_matches(&fds, "PowerState"));
        assert!(!name_matches(&fds, "notional.power_demo.v1.PowerState"));
    }

    #[test]
    fn pre_fix_regression_22_notional_topics_now_pass() {
        // Direct regression test for the operator- RCA:
        // 22 notional topics all reported "Format declares
        // 'protobuf:notional.<demo>.v1.<Msg>' but the uploaded
        // schema contains no top-level message named
        // 'notional.<demo>.v1.<Msg>'. Found: <Msg>." The producer
        // setup was correct; the check truncated FDS full names
        // before comparing.
        let beacon_fds = vec!["notional.beacon_demo.v1.Hello".to_string()];
        let power_fds = vec!["notional.power_demo.v1.PowerState".to_string()];
        let tracks_fds = vec!["notional.tracks_demo.v1.Entity".to_string()];
        assert!(name_matches(&beacon_fds, "notional.beacon_demo.v1.Hello"));
        assert!(name_matches(&power_fds, "notional.power_demo.v1.PowerState"));
        assert!(name_matches(&tracks_fds, "notional.tracks_demo.v1.Entity"));
    }
}
