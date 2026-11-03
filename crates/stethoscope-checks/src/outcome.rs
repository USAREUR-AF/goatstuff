// SPDX-License-Identifier: Apache-2.0
//
// CheckOutcome — what a check produces. Verdict + summary +
// optional findings + optional structured details (Phase C will
// wire details into the drawer; Phase B emits an empty Vec).

use serde::{Deserialize, Serialize};

/// Per the wireframe brief §"Check cards" verdict states. The
/// `Loading` state is rendered by the binary while a check is in
/// flight — checks themselves never return Loading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Pass,
    Warn,
    Fail,
    Skipped,
    /// This check doesn't apply to this target kind. The Registry
    /// returns this automatically when `Check::applies_to` excludes
    /// the target — checks never need to return it themselves.
    #[serde(rename = "na")]
    NotApplicable,
}

impl Verdict {
    /// Ordering for the header "worst-wins" roll-up. Higher = worse.
    /// Skipped + NotApplicable do not contribute to the roll-up.
    pub fn severity_rank(self) -> u8 {
        match self {
            Verdict::Fail => 3,
            Verdict::Warn => 2,
            Verdict::Pass => 1,
            Verdict::Skipped | Verdict::NotApplicable => 0,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Warn => "warn",
            Verdict::Fail => "fail",
            Verdict::Skipped => "skipped",
            Verdict::NotApplicable => "na",
        }
    }
}

/// Advisory chip attached beneath a card. Findings never change the
/// verdict — they communicate "your expectations may not match
/// reality." Phase B emits two: `published-but-never-declared` (new
/// dual surfaced by the DRS RCA) on the registration check, and the
/// existing `declared-but-never-published` on the flow-rate check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub label: String,
    pub severity: FindingSeverity,
    pub detail: String,
    /// Optional deep-link. When set, the chip renders as an anchor —
    /// used by the topic-roster check so each per-child finding click-
    /// throughs straight to the child's diagnose page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

impl Finding {
    pub fn new(
        label: impl Into<String>,
        severity: FindingSeverity,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            severity,
            detail: detail.into(),
            href: None,
        }
    }

    pub fn with_href(mut self, href: impl Into<String>) -> Self {
        self.href = Some(href.into());
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingSeverity {
    Info,
    Warn,
    /// Used by composite checks (e.g. topic-roster) that emit one
    /// finding per child whose verdict was Pass — render as a green
    /// chip so the operator sees diverse roll-up state at a glance.
    Pass,
    /// Used by composite checks for child verdicts that were Fail.
    Fail,
}

/// Actionable next step rendered under a Fail/Warn card. Each step
/// gets a short title + a longer explanation; optional href lets the
/// UI deep-link into the portal or another stethoscope page so the
/// operator can take action without leaving the diagnose surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Remediation {
    pub title: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

impl Remediation {
    pub fn new(title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
            href: None,
        }
    }

    pub fn with_href(mut self, href: impl Into<String>) -> Self {
        self.href = Some(href.into());
        self
    }
}

/// One check's verdict for one target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckOutcome {
    pub check_id: String,
    pub verdict: Verdict,
    /// One-line summary line rendered on the card body. Mirrors
    /// the data.js sample lines verbatim in spirit.
    pub summary: String,
    /// Advisory chips beneath the summary. Empty by default.
    #[serde(default)]
    pub findings: Vec<Finding>,
    /// Actionable next steps rendered as a block beneath the summary
    /// on Fail/Warn cards. Each step carries a short title + a longer
    /// detail + optional deep-link href. Empty by default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remediation: Vec<Remediation>,
    /// Structured drill-down details for Phase C drawer. Phase B
    /// emits None and the card is non-clickable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl CheckOutcome {
    pub fn pass(check_id: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            check_id: check_id.into(),
            verdict: Verdict::Pass,
            summary: summary.into(),
            findings: Vec::new(),
            remediation: Vec::new(),
            details: None,
        }
    }

    pub fn warn(check_id: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            check_id: check_id.into(),
            verdict: Verdict::Warn,
            summary: summary.into(),
            findings: Vec::new(),
            remediation: Vec::new(),
            details: None,
        }
    }

    pub fn fail(check_id: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            check_id: check_id.into(),
            verdict: Verdict::Fail,
            summary: summary.into(),
            findings: Vec::new(),
            remediation: Vec::new(),
            details: None,
        }
    }

    pub fn skipped(check_id: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            check_id: check_id.into(),
            verdict: Verdict::Skipped,
            summary: summary.into(),
            findings: Vec::new(),
            remediation: Vec::new(),
            details: None,
        }
    }

    pub fn not_applicable(check_id: impl Into<String>) -> Self {
        Self {
            check_id: check_id.into(),
            verdict: Verdict::NotApplicable,
            summary: "not applicable to this target kind".to_string(),
            findings: Vec::new(),
            remediation: Vec::new(),
            details: None,
        }
    }

    pub fn with_finding(mut self, finding: Finding) -> Self {
        self.findings.push(finding);
        self
    }

    pub fn with_remediation(mut self, step: Remediation) -> Self {
        self.remediation.push(step);
        self
    }
}
