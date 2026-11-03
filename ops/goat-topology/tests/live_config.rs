// SPDX-License-Identifier: Apache-2.0
//! Integration check: run the real binary against a REAL deployed router config.
//!
//! The config file is a goatnet's live ACL, so it is deliberately NOT committed.
//! Point this at one before shipping a parser change:
//!
//!   GOAT_TOPOLOGY_LIVE_CONFIG=/path/to/config.json5 \
//!     cargo test --test live_config -- --nocapture
//!
//! Skips (rather than fails) when the variable is unset, so CI stays green
//! without carrying an ACL in the repo. What it proves that a unit test cannot:
//! the brace-matcher survives ~900 KB of real key-expressions and comments, and
//! the binary reports the posture the fleet actually has.

use std::process::Command;

#[test]
fn parses_a_real_router_config_without_choking_on_the_acl() {
    let Ok(path) = std::env::var("GOAT_TOPOLOGY_LIVE_CONFIG") else {
        eprintln!("skip: set GOAT_TOPOLOGY_LIVE_CONFIG to a real router config.json5");
        return;
    };
    let text = std::fs::read_to_string(&path).expect("read live config");
    assert!(
        text.len() > 100_000,
        "expected a real config; this one is {} bytes and a toy file proves nothing here",
        text.len()
    );

    let out = Command::new(env!("CARGO_BIN_EXE_goat-topology"))
        .args(["inspect", "--config-only", "--config-path", &path])
        .output()
        .expect("run goat-topology inspect");
    assert!(
        out.status.success(),
        "inspect failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("inspect emits JSON");
    let mode = v["gatewayMode"]
        .as_str()
        .or_else(|| v["gateway_mode"].as_str());
    eprintln!(
        "live config {} bytes -> gateway_mode {:?}",
        text.len(),
        mode
    );

    // Every goat router today has NO `gateway` key, which IS zenoh's Auto
    // default. If this ever reads `unreadable` the parser has broken
    // against a real file; if it reads `custom`, a goatnet has been carved and
    // the fleet-wide claim in zenoh-data-fabric.md needs revisiting.
    assert_eq!(mode, Some("auto"), "unexpected declared posture: {v}");

    // The ACL must never reach the envelope.
    let json = String::from_utf8_lossy(&out.stdout);
    assert!(
        !json.contains("access_control") && !json.contains("key_exprs"),
        "ACL content leaked into the snapshot"
    );
}
