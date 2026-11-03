// SPDX-License-Identifier: Apache-2.0
//
// Schema diff — detects when a key's format-tag changes mid-stream.
//
// Per docs/design/flashlight.md §Tier 2 details: "Schema diff between
// any two uploaded `.fds` descriptors", with the in-stream tracking
// variant being "detects when the schema declared by a peer differs
// from the schema other peers on the same topic prefix are using, or
// differs from the previously-observed shape on the same key."
//
// v1 scope: per-key drift only (a key changes format between events).
// Cross-prefix disagreement detection (vendor/topic prefix sees two
// formats from different peers) is deferred — needs a prefix policy
// the operator can configure, which isn't worth designing until an
// operator hits the case.
//
// Determinism: `seen_formats` is a `BTreeMap` (sorted by key) so the
// snapshot serializes in key-order regardless of event arrival order.
// Drift events are a `Vec` in chronological order; that's also stable
// when events arrive in the same order across native + WASM (the only
// configuration that matters for the equivalence gate).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::events::Event;

/// One observed format-drift event. Records the moment a key's format
/// tag changed, with the previous + current format strings so a
/// frontend can render "key X: protobuf:A → protobuf:B".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriftEvent {
    pub ts_ms: i64,
    pub key: String,
    pub previous_format: Option<String>,
    pub current_format: Option<String>,
}

/// Snapshot of the schema-drift tracker. Serializes deterministically
/// — `seen_formats` is a BTreeMap (key-sorted), `drift_events` is
/// chronological.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaDiffSnapshot {
    /// Current (key → format) observation, with the most recent
    /// format the tracker saw for each key.
    pub seen_formats: BTreeMap<String, Option<String>>,
    /// Chronological list of detected drift events (format changes).
    pub drift_events: Vec<DriftEvent>,
}

impl SchemaDiffSnapshot {
    /// Human-readable summary for top-level `AnalyticsSnapshot` —
    /// "N drift event(s) across M key(s)" or None if nothing observed.
    pub fn summary(&self) -> Option<String> {
        if self.seen_formats.is_empty() {
            return None;
        }
        Some(format!(
            "{} drift event(s) across {} key(s)",
            self.drift_events.len(),
            self.seen_formats.len()
        ))
    }
}

#[derive(Debug, Default, Clone)]
pub struct SchemaDiffer {
    seen_formats: BTreeMap<String, Option<String>>,
    drift_events: Vec<DriftEvent>,
}

impl SchemaDiffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe one event. If the event's key has been seen before and
    /// its format differs from the prior observation (including
    /// None→Some or Some→None transitions), records a drift event.
    pub fn observe(&mut self, event: &Event) {
        let format = event.format.clone();
        match self.seen_formats.get(&event.key) {
            None => {
                // First observation — record the (key, format) pair
                // but don't emit a drift event (there's nothing to
                // drift from).
                self.seen_formats.insert(event.key.clone(), format);
            }
            Some(previous) if previous == &format => {
                // Same format as before — no drift.
            }
            Some(previous) => {
                self.drift_events.push(DriftEvent {
                    ts_ms: event.ts_ms,
                    key: event.key.clone(),
                    previous_format: previous.clone(),
                    current_format: format.clone(),
                });
                self.seen_formats.insert(event.key.clone(), format);
            }
        }
    }

    pub fn snapshot(&self) -> SchemaDiffSnapshot {
        SchemaDiffSnapshot {
            seen_formats: self.seen_formats.clone(),
            drift_events: self.drift_events.clone(),
        }
    }

    /// Cheap top-level summary string for `AnalyticsSnapshot`. Avoids
    /// cloning the full snapshot when only the headline string is
    /// needed.
    pub fn summary(&self) -> Option<String> {
        if self.seen_formats.is_empty() {
            return None;
        }
        Some(format!(
            "{} drift event(s) across {} key(s)",
            self.drift_events.len(),
            self.seen_formats.len()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(ts_ms: i64, key: &str, format: Option<&str>) -> Event {
        Event {
            key: key.to_string(),
            ts_ms,
            payload: vec![],
            format: format.map(|s| s.to_string()),
        }
    }

    #[test]
    fn empty_differ_has_no_summary() {
        let d = SchemaDiffer::new();
        assert!(d.summary().is_none());
        let snap = d.snapshot();
        assert!(snap.seen_formats.is_empty());
        assert!(snap.drift_events.is_empty());
        assert!(snap.summary().is_none());
    }

    #[test]
    fn first_observation_is_not_drift() {
        let mut d = SchemaDiffer::new();
        d.observe(&ev(1, "vendor-a/obs/v1", Some("protobuf:Observation")));
        let snap = d.snapshot();
        assert_eq!(snap.seen_formats.len(), 1);
        assert!(snap.drift_events.is_empty());
        assert_eq!(snap.summary().unwrap(), "0 drift event(s) across 1 key(s)");
    }

    #[test]
    fn same_format_repeat_does_not_emit_drift() {
        let mut d = SchemaDiffer::new();
        d.observe(&ev(1, "k", Some("protobuf:A")));
        d.observe(&ev(2, "k", Some("protobuf:A")));
        d.observe(&ev(3, "k", Some("protobuf:A")));
        assert!(d.snapshot().drift_events.is_empty());
    }

    #[test]
    fn format_change_emits_drift_event() {
        let mut d = SchemaDiffer::new();
        d.observe(&ev(1, "k", Some("protobuf:A")));
        d.observe(&ev(2, "k", Some("protobuf:B")));
        let snap = d.snapshot();
        assert_eq!(snap.drift_events.len(), 1);
        let drift = &snap.drift_events[0];
        assert_eq!(drift.ts_ms, 2);
        assert_eq!(drift.key, "k");
        assert_eq!(drift.previous_format.as_deref(), Some("protobuf:A"));
        assert_eq!(drift.current_format.as_deref(), Some("protobuf:B"));
        // After the drift, seen_formats reflects the NEW format.
        assert_eq!(snap.seen_formats["k"].as_deref(), Some("protobuf:B"));
    }

    #[test]
    fn none_to_some_transition_is_drift() {
        // A peer that was emitting untagged events starting to tag
        // them is a meaningful schema event — operator wants to see it.
        let mut d = SchemaDiffer::new();
        d.observe(&ev(1, "k", None));
        d.observe(&ev(2, "k", Some("protobuf:A")));
        let snap = d.snapshot();
        assert_eq!(snap.drift_events.len(), 1);
        assert_eq!(snap.drift_events[0].previous_format, None);
        assert_eq!(snap.drift_events[0].current_format.as_deref(), Some("protobuf:A"));
    }

    #[test]
    fn some_to_none_transition_is_drift() {
        // The dual case — schema getting dropped.
        let mut d = SchemaDiffer::new();
        d.observe(&ev(1, "k", Some("protobuf:A")));
        d.observe(&ev(2, "k", None));
        let snap = d.snapshot();
        assert_eq!(snap.drift_events.len(), 1);
        assert_eq!(snap.drift_events[0].previous_format.as_deref(), Some("protobuf:A"));
        assert_eq!(snap.drift_events[0].current_format, None);
    }

    #[test]
    fn drift_events_are_chronological() {
        let mut d = SchemaDiffer::new();
        d.observe(&ev(10, "k1", Some("A")));
        d.observe(&ev(20, "k2", Some("B")));
        d.observe(&ev(30, "k1", Some("A2"))); // drift on k1
        d.observe(&ev(40, "k2", Some("B2"))); // drift on k2
        let snap = d.snapshot();
        assert_eq!(snap.drift_events.len(), 2);
        assert_eq!(snap.drift_events[0].ts_ms, 30);
        assert_eq!(snap.drift_events[0].key, "k1");
        assert_eq!(snap.drift_events[1].ts_ms, 40);
        assert_eq!(snap.drift_events[1].key, "k2");
    }

    #[test]
    fn seen_formats_serializes_in_key_order() {
        // BTreeMap → JSON output ordered by key, regardless of insertion
        // order. Critical for the Phase D byte-equal gate.
        let mut d = SchemaDiffer::new();
        d.observe(&ev(1, "zebra", Some("Z")));
        d.observe(&ev(2, "alpha", Some("A")));
        d.observe(&ev(3, "monkey", Some("M")));
        let json = serde_json::to_string(&d.snapshot()).unwrap();
        // Alpha must appear before monkey, monkey before zebra.
        let alpha_at = json.find("alpha").unwrap();
        let monkey_at = json.find("monkey").unwrap();
        let zebra_at = json.find("zebra").unwrap();
        assert!(alpha_at < monkey_at);
        assert!(monkey_at < zebra_at);
    }

    #[test]
    fn snapshot_is_serde_stable() {
        // Phase D equivalence-test gate: same input → byte-equal JSON.
        let mut a = SchemaDiffer::new();
        let mut b = SchemaDiffer::new();
        for (ts, k, f) in [
            (1, "vendor-a/obs/v1", Some("protobuf:Observation")),
            (2, "vendor-a/obs/v1", Some("protobuf:Observation")),
            (3, "vendor-b/track/v1", Some("protobuf:Track")),
            (4, "vendor-a/obs/v1", Some("protobuf:ObservationV2")),
        ] {
            let event = ev(ts, k, f);
            a.observe(&event);
            b.observe(&event);
        }
        let ja = serde_json::to_string(&a.snapshot()).unwrap();
        let nova = serde_json::to_string(&b.snapshot()).unwrap();
        assert_eq!(ja, nova);
    }
}
