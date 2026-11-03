// SPDX-License-Identifier: Apache-2.0
//
// The Check trait — the substrate every diagnostic implements.
// Per docs/design/stethoscope.md §"Plugin interface for checks".

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::context::{CheckCtx, Target, TargetKind};
use crate::outcome::CheckOutcome;

/// Passive vs active (blood-dye). Phase B only ships Passive checks;
/// Active probes land in Phase D once the offline-CA cert + reserved
/// `diagnose/<v>/**` prefix substrate is in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckKind {
    Passive,
    Active,
}

/// Cost label — surfaced on cards + configure-checks drawer so the
/// operator can reason about which checks to mute on a slow link.
/// Mirrors the wireframe brief's vocabulary verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cost {
    /// No I/O — pure local compute on already-fetched context.
    None,
    /// Reads from a substrate (portal / panoscope / netbird-mgmt).
    NetworkRead,
    /// Injects new traffic onto the fabric — blood-dye only.
    NetworkProbe,
    /// Heavy local compute (descriptor parse + sample decode).
    Compute,
}

#[async_trait]
pub trait Check: Send + Sync + 'static {
    /// Stable identifier used in URL params, SSE event payload `check_id`,
    /// CSS class suffixes, and the Configure-checks drawer. Lower-kebab.
    fn id(&self) -> &'static str;
    /// Human-facing name rendered on the card title row.
    fn name(&self) -> &'static str;
    /// Passive vs active.
    fn kind(&self) -> CheckKind;
    /// Cost class.
    fn cost(&self) -> Cost;
    /// Target kinds this check produces a verdict against. Others
    /// land as `NotApplicable` automatically — checks should not need
    /// to no-op themselves on the wrong target.
    fn applies_to(&self) -> &'static [TargetKind];
    /// One-line description for the Configure-checks drawer.
    fn description(&self) -> &'static str;
    /// Run against the resolved target. Returns the verdict; errors
    /// surface as `Verdict::Skipped` with `summary` describing why,
    /// so the page renders an honest "couldn't check" card instead
    /// of swallowing the failure.
    async fn run(&self, ctx: &CheckCtx, target: &Target) -> CheckOutcome;
}
