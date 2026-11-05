// SPDX-License-Identifier: Apache-2.0
//
// Phase B passive checks — one module per check, registered via
// `register_phase_b(&mut Registry)` from the binary's main.rs.
//
// By design: adding a new diagnostic is a one-file PR under
// this directory + a one-line addition to `register_phase_b`. CI
// requires a regression-test fixture per check.

pub mod cardinality;
pub mod decode_roundtrip;
pub mod flow_rate;
pub mod registration;
pub mod schema_body;
pub mod topic_roster;

use crate::registry::Registry;

/// Wire the Phase B passive checks into a Registry. Binary's main()
/// calls this once on startup.
pub fn register_phase_b(reg: &mut Registry) {
    reg.register(registration::RegistrationCheck);
    reg.register(schema_body::SchemaBodyCheck);
    reg.register(decode_roundtrip::DecodeRoundtripCheck);
    reg.register(flow_rate::FlowRateCheck);
    reg.register(topic_roster::TopicRosterCheck);
    reg.register(cardinality::CardinalityCheck);
}
