// SPDX-License-Identifier: Apache-2.0
//
// stethoscope-checks — Check trait + plugin registry + Phase B checks.
// Filled in across subsequent commits. See docs/design/stethoscope.md.

pub mod check;
pub mod registry;
pub mod context;
pub mod checks;
pub mod outcome;

pub use check::{Check, CheckKind, Cost};
pub use context::{CheckCtx, Target};
pub use outcome::{CheckOutcome, Finding, FindingSeverity, Remediation, Verdict};
pub use registry::Registry;
