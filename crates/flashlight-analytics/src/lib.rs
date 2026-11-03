// SPDX-License-Identifier: Apache-2.0
//
// flashlight-analytics — Tier 2 analytics core for flashlight.
//
// Dual compile target per docs/design/flashlight.md §"Same Rust code, two
// compile targets":
//
//   * `wasm32-unknown-unknown` → browser-side Tier 2 module loaded by
//     flashlight's Tier 1 page; events forwarded into the module via
//     `push_event(ptr, len)` (one copy per event, no SharedArrayBuffer).
//   * native → linked into flashlight's `/api/compute` server endpoint
//     for operators whose device cannot run WASM efficiently
//     (selected via `?compute=server` per design doc Principle 13).
//
// Equivalence is the gate: the same input must produce binary-identical
// output regardless of target. Equivalence tests live in tests/.
//
// Phase D scope: skeleton + the four v1 capabilities stubbed. Real logic
// lands in follow-on commits, each with its own equivalence test.

pub mod anomaly;
pub mod decode;
pub mod events;
pub mod schema_diff;
pub mod sparkline;
pub mod stats;

pub use anomaly::{AnomalyDetector, AnomalySnapshot};
pub use events::Event;
pub use schema_diff::{DriftEvent, SchemaDiffSnapshot, SchemaDiffer};
pub use sparkline::{Sparkline, SparklineSnapshot};
pub use stats::{RollingStats, StatsSnapshot};

/// Snapshot the analytics module can serialize to either the browser
/// (via wasm-bindgen) or the `/api/compute` HTTP response (via JSON).
///
/// The byte representation must be identical across compile targets —
/// equivalence tests in `tests/equivalence.rs` assert this.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AnalyticsSnapshot {
    pub event_count: u64,
    /// Sliding-window rolling stats: rate + inter-arrival + byte
    /// percentiles. Populated from the engine's `RollingStats` tracker.
    pub stats: StatsSnapshot,
    /// Per-key format-drift tracking. Records when a key changes
    /// format-tag mid-stream (e.g. `protobuf:A` → `protobuf:B`).
    pub schema_diff: SchemaDiffSnapshot,
    /// Welford-online z-score anomaly tracking over per-event inter-
    /// arrival deltas. `latest_z_score` answers "how unusual was the
    /// most recent event's spacing relative to history?"
    pub anomaly: AnomalySnapshot,
    /// Higher-fidelity sparkline bin counts over the rolling window.
    /// Tier 2 augments Tier 1's coarse Canvas 2D sparkline with these
    /// finer-grained points.
    pub sparkline: SparklineSnapshot,
}

/// Top-level analytics engine. Owns the rolling state across events.
#[derive(Debug)]
pub struct Engine {
    event_count: u64,
    stats: RollingStats,
    schema_differ: SchemaDiffer,
    anomaly: AnomalyDetector,
    sparkline: Sparkline,
    /// Timestamp of the previously observed event. Used to compute
    /// inter-arrival delta to feed into the anomaly detector. None
    /// until the first event lands.
    prev_event_ts_ms: Option<i64>,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            event_count: 0,
            stats: RollingStats::default_window(),
            schema_differ: SchemaDiffer::new(),
            anomaly: AnomalyDetector::new(),
            sparkline: Sparkline::default_window(),
            prev_event_ts_ms: None,
        }
    }
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct an Engine with a custom rolling-stats / sparkline
    /// window. The same `window_ms` is used for both — operators
    /// expect a single "?window=" URL knob to control all of the
    /// rolling surfaces. Sparkline bin count stays at the default
    /// (60 bins per the design doc).
    pub fn with_window_ms(window_ms: i64) -> Self {
        Self {
            event_count: 0,
            stats: RollingStats::new(window_ms),
            schema_differ: SchemaDiffer::new(),
            anomaly: AnomalyDetector::new(),
            sparkline: Sparkline::new(60, window_ms),
            prev_event_ts_ms: None,
        }
    }

    /// Observe one event. Native callers pass `&Event` directly; the WASM
    /// bridge deserializes JSON-encoded event bytes from JS first.
    pub fn push_event(&mut self, event: &Event) {
        self.event_count = self.event_count.saturating_add(1);
        self.stats.observe(event);
        self.schema_differ.observe(event);
        self.sparkline.observe(event.ts_ms);
        if let Some(prev) = self.prev_event_ts_ms {
            // Inter-arrival ms — operator-meaningful "how often are
            // events coming?" series. Welford absorbs out-of-order
            // negative deltas, but Tier 1's emitter is monotonic so
            // this is normally non-negative.
            let delta_ms = (event.ts_ms - prev) as f64;
            self.anomaly.observe(delta_ms);
        }
        self.prev_event_ts_ms = Some(event.ts_ms);
    }

    pub fn snapshot(&self) -> AnalyticsSnapshot {
        AnalyticsSnapshot {
            event_count: self.event_count,
            stats: self.stats.snapshot(),
            schema_diff: self.schema_differ.snapshot(),
            anomaly: self.anomaly.snapshot(),
            sparkline: self.sparkline.snapshot(),
        }
    }
}

// ---------------------------------------------------------------------------
// WASM bridge — only compiled when targeting wasm32. The native build never
// pulls in wasm-bindgen, so the server linkage stays clean.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use std::cell::RefCell;
    use wasm_bindgen::prelude::*;

    thread_local! {
        static ENGINE: RefCell<Engine> = RefCell::new(Engine::new());
    }

    /// JS-side calls `instance.exports.push_event(jsonBytes)` once per
    /// SSE-delivered event. `event_json` is the Event struct serialized
    /// with `serde_json` (matching the wire shape Tier 1 SSE emits).
    /// Silent skip on malformed input — Tier 1's emitter is trusted.
    #[wasm_bindgen]
    pub fn push_event(event_json: &[u8]) {
        if let Ok(event) = serde_json::from_slice::<Event>(event_json) {
            ENGINE.with(|e| e.borrow_mut().push_event(&event));
        }
    }

    #[wasm_bindgen]
    pub fn snapshot_json() -> String {
        ENGINE.with(|e| {
            serde_json::to_string(&e.borrow().snapshot()).unwrap_or_default()
        })
    }

    /// Decode `payload` (protobuf wire bytes) against `fds_bytes`
    /// (a FileDescriptorSet, typically returned by the portal
    /// `/api/schemas/<key>` endpoint). `message_name` is the
    /// fully-qualified protobuf message name; pass `null` from JS
    /// to fall back to the first-message convention per ADR 0516.
    ///
    /// Returns a JSON string: either the decoded message (on success)
    /// or a `{"error": {...}}` envelope wrapping the `DecodeError`
    /// (on failure). Caller-side JS distinguishes the two by checking
    /// for the `error` key.
    #[wasm_bindgen]
    pub fn decode_payload_json(
        payload: &[u8],
        fds_bytes: &[u8],
        message_name: Option<String>,
    ) -> String {
        match crate::decode::decode_payload(payload, fds_bytes, message_name.as_deref()) {
            Ok(value) => value.to_string(),
            Err(err) => serde_json::json!({ "error": err }).to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(ts_ms: i64, payload: &[u8]) -> Event {
        Event {
            key: "t".into(),
            ts_ms,
            payload: payload.to_vec(),
            format: None,
        }
    }

    #[test]
    fn engine_counts_pushed_events() {
        let mut e = Engine::new();
        e.push_event(&ev(1, b"first"));
        e.push_event(&ev(2, b"second"));
        assert_eq!(e.snapshot().event_count, 2);
    }

    #[test]
    fn default_snapshot_is_zero_state() {
        let snap = Engine::new().snapshot();
        assert_eq!(snap.event_count, 0);
        assert_eq!(snap.stats.rate_hz, 0.0);
        assert_eq!(snap.stats.sample_count, 0);
        assert!(snap.schema_diff.seen_formats.is_empty());
        assert!(snap.schema_diff.drift_events.is_empty());
        assert_eq!(snap.anomaly.sample_count, 0);
        assert!(snap.anomaly.latest_z_score.is_none());
        // Empty engine snapshots the default sparkline (60 bins, all
        // zero) — bin_count is part of the contract even with no data.
        assert_eq!(snap.sparkline.bin_count, 60);
        assert_eq!(snap.sparkline.window_ms, 5 * 60 * 1000);
        assert_eq!(snap.sparkline.points, vec![0.0; 60]);
    }
}
