// SPDX-License-Identifier: Apache-2.0
//
// Event shape that crosses the JS↔WASM boundary (browser Tier 2) and
// the HTTP boundary (server `/api/compute` endpoint). Identical on both
// sides — equivalence tests assert byte-equal serde round-trip.
//
// Mirrors the SSE event shape flashlight Tier 1 emits (key + timestamp
// + payload bytes + optional format tag). Kept minimal in this phase;
// real protobuf decode lives in `decode.rs` and feeds richer fields in
// follow-on commits.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Zenoh key the event was published under.
    pub key: String,
    /// Unix-epoch milliseconds of the publish timestamp.
    pub ts_ms: i64,
    /// Wire payload (opaque until `decode::decode_payload` is wired up).
    pub payload: Vec<u8>,
    /// Optional schema-declared format tag (e.g. `protobuf:<MsgName>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}
