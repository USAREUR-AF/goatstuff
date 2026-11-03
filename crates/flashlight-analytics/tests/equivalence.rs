// SPDX-License-Identifier: Apache-2.0
//
// Equivalence-test substrate. Phase D gate to Phase E (per
// docs/design/flashlight.md): "equivalence tests pass on CI for all
// four capabilities; binary-identical output asserted."
//
// Dual-target: each test runs on both native and wasm32. The native
// pass uses `cargo test`; the wasm32 pass uses `cargo test --target
// wasm32-unknown-unknown` driven by `wasm-bindgen-test-runner`
// (installed in CI via `cargo install wasm-bindgen-cli`). The same
// fixture asserts the same expected JSON on both targets, so any
// divergence in target-specific f64 / integer / serialization
// behaviour fails the gate.
//
// Fixtures are loaded via `include_str!` so they're compiled into the
// test binary — no filesystem dependency on the WASM side.

use flashlight_analytics::{Engine, Event};
use serde::Deserialize;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test;

#[derive(Debug, Deserialize)]
struct Fixture {
    events: Vec<Event>,
    expected_snapshot_json: serde_json::Value,
}

const EMPTY_FIXTURE_JSON: &str = include_str!("fixtures/empty.json");
const TWO_EVENTS_FIXTURE_JSON: &str = include_str!("fixtures/two-events.json");

fn parse_fixture(raw: &str, name: &str) -> Fixture {
    serde_json::from_str(raw).unwrap_or_else(|e| panic!("parse fixture `{name}`: {e}"))
}

fn run_fixture(fixture: &Fixture) -> serde_json::Value {
    let mut engine = Engine::new();
    for event in &fixture.events {
        engine.push_event(event);
    }
    let snap = engine.snapshot();
    serde_json::to_value(&snap).expect("serialize snapshot")
}

fn assert_fixture_equivalence(name: &str, raw: &str) {
    let fixture = parse_fixture(raw, name);
    let output = run_fixture(&fixture);
    assert_eq!(
        output, fixture.expected_snapshot_json,
        "output diverged from fixture `{name}` expected snapshot",
    );
}

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
fn empty_fixture_matches_expected() {
    assert_fixture_equivalence("empty", EMPTY_FIXTURE_JSON);
}

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
fn two_events_fixture_matches_expected() {
    assert_fixture_equivalence("two-events", TWO_EVENTS_FIXTURE_JSON);
}
