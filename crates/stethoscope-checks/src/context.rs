// SPDX-License-Identifier: Apache-2.0
//
// Target + CheckCtx — the inputs every check gets at run time.
// Targets are URL-parsed; CheckCtx carries the data-source clients
// the checks share across one diagnosis session.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use panoscope_core::{AuditTail, PortalClient};

/// Which kind of thing the operator is diagnosing. The set determines
/// which checks `applies_to` returns true for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetKind {
    Vendor,
    Topic,
    System,
}

impl TargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetKind::Vendor => "vendor",
            TargetKind::Topic => "topic",
            TargetKind::System => "system",
        }
    }
}

/// Resolved diagnosis target. Builders only — checks read fields.
#[derive(Debug, Clone)]
pub struct Target {
    pub kind: TargetKind,
    /// The full URL-param value: vendor prefix, topic key_pattern, or
    /// `<vendor>/<system>` name.
    pub value: String,
}

impl Target {
    pub fn vendor(name: impl Into<String>) -> Self {
        Self {
            kind: TargetKind::Vendor,
            value: name.into(),
        }
    }

    pub fn topic(key: impl Into<String>) -> Self {
        Self {
            kind: TargetKind::Topic,
            value: key.into(),
        }
    }

    pub fn system(name: impl Into<String>) -> Self {
        Self {
            kind: TargetKind::System,
            value: name.into(),
        }
    }

    /// First slash-segment of `value` — the vendor prefix for any
    /// target kind. Returns an empty `&str` if value has no slash and
    /// kind is not `Vendor` (callers should treat that as a parse
    /// error).
    pub fn vendor_prefix(&self) -> &str {
        match self.kind {
            TargetKind::Vendor => &self.value,
            TargetKind::Topic | TargetKind::System => {
                self.value.split('/').next().unwrap_or("")
            }
        }
    }
}

/// Shared substrate handle every check sees. Cheap to clone (everything
/// inside is `Arc` or `Clone` already).
#[derive(Clone)]
pub struct CheckCtx {
    pub portal: Arc<PortalClient>,
    pub audit: Arc<AuditTail>,
    /// Display-only participant noun (nova-a "vendor" / ZAGROS "org") for
    /// operator-facing verdict messages. Render-time wording only — the
    /// `<anchor>/<rest>` key shape, the `vendor`/`key` filter kind, the
    /// portal `list_vendors` call, and the `vendor-decode-rca` tool name
    /// are all untouched. Defaults to "vendor" (nova-a, byte-stable); the
    /// stethoscope binary sets it from `STETHOSCOPE_LEX_ORG` via
    /// [`CheckCtx::with_org`]. See docs/design/portal-lexicon.md.
    pub org: String,
}

impl CheckCtx {
    pub fn new(portal: PortalClient, audit: AuditTail) -> Self {
        Self {
            portal: Arc::new(portal),
            audit: Arc::new(audit),
            org: "vendor".to_string(),
        }
    }

    /// Set the display noun (chained at construction in the binary). Empty
    /// input is ignored so the nova-a default survives.
    pub fn with_org(mut self, org: impl Into<String>) -> Self {
        let org = org.into();
        if !org.trim().is_empty() {
            self.org = org;
        }
        self
    }

    /// Title-case form of the display noun ("Vendor" / "Org"), for
    /// sentence-leading verdict messages.
    pub fn org_title(&self) -> String {
        let mut c = self.org.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => self.org.clone(),
        }
    }

    /// Topics declared for `vendor` under slot-anchor vendor rows — the
    /// slot-pool registry shape. On slot-pool goatnets (ZAGROS) the
    /// portal files topics under anchor-named vendor rows
    /// (`<anchor>/<vendor>/...`), so `list_topics(vendor)` by handle comes
    /// back empty even though the vendor's topics exist. This sweep walks the
    /// anchor-shaped vendor rows and keeps the topics whose vendor segment
    /// matches. Bounded by the assigned-slot count (pool sized in the tens per
    /// ADR 1025). Returns an empty vec on any portal error — callers treat it
    /// as "nothing extra found", never a verdict-blocking failure.
    pub async fn slot_anchored_topics(&self, vendor: &str) -> Vec<panoscope_core::Topic> {
        // An anchor-shaped target already names a vendor row directly; only
        // handle-shaped targets need the sweep.
        if panoscope_core::alias::is_anchor_shaped(vendor) {
            return Vec::new();
        }
        let Ok(vendors) = self.portal.list_vendors().await else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for v in vendors
            .iter()
            .filter(|v| panoscope_core::alias::is_anchor_shaped(&v.prefix))
        {
            if let Ok(topics) = self.portal.list_topics(&v.prefix).await {
                out.extend(
                    topics
                        .into_iter()
                        .filter(|t| panoscope_core::vendor_segment(&t.key_pattern) == vendor),
                );
            }
        }
        out
    }
}
