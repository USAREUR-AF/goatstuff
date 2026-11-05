// SPDX-License-Identifier: Apache-2.0
//
// Sliding-window rolling statistics — rate (events/sec), inter-arrival
// percentiles (p50/p95/p99 in ms), and payload-byte percentiles
// (p50/p95/p99 in bytes), all over an operator-specified window. Window
// defaults to 5 minutes per docs/design/flashlight.md §Tier 2 details.
//
// Eviction semantics: the window slides relative to the most recently
// observed event's `ts_ms`, not wall-clock — flashlight has no clock
// on the WASM side anyway, and using event-ts means the same fixture
// produces deterministic output regardless of when the test runs.
//
// Percentile method: nearest-rank, index = ceil(p/100 * n) - 1. Chosen
// because (a) it's the simplest method that returns an actual observed
// value (no interpolation between samples), (b) it has no float
// instability — bit-identical output across native + WASM is the Phase D
// gate to Phase E, and interpolation-based methods can diverge across
// rounding modes.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

use serde::{Deserialize, Serialize};

use crate::events::Event;

/// Sliding-window rolling-statistics snapshot. Serialized identically
/// on native + WASM — fields in declaration order so the JSON shape is
/// stable.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatsSnapshot {
    /// Window the stats were computed over (operator-controllable).
    pub window_ms: i64,
    /// Number of samples retained in the window at snapshot time.
    pub sample_count: usize,
    /// Events per second over the window. `sample_count / (window_ms /
    /// 1000)`. Smoothed across the window — for instantaneous rate,
    /// shrink the window.
    pub rate_hz: f64,
    /// p50 / p95 / p99 of inter-arrival deltas between consecutive
    /// samples, in milliseconds. Zero when fewer than 2 samples.
    pub inter_arrival_p50_ms: f64,
    pub inter_arrival_p95_ms: f64,
    pub inter_arrival_p99_ms: f64,
    /// p50 / p95 / p99 of payload byte counts, in bytes.
    pub byte_p50: u32,
    pub byte_p95: u32,
    pub byte_p99: u32,
}

#[derive(Debug, Clone, Copy)]
struct Sample {
    ts_ms: i64,
    bytes: u32,
}

/// Sliding-window rolling statistics over event timestamps and payload
/// sizes. Memory grows with the in-window sample count, bounded by the
/// window length divided by minimum inter-arrival time.
///
/// Eviction is O(log n) per observed event: in-window samples live in
/// arrival order keyed by an ingest sequence number, and a min-heap on
/// `ts_ms` finds every sample below the cutoff without scanning the whole
/// window. (A `Vec::retain` per event was O(window) per event, so at
/// 1000 events/s over a 5-minute window each event walked ~300k samples.)
/// The retained set is exactly what the earlier retain-per-event produced,
/// including for out-of-order timestamps.
#[derive(Debug, Clone)]
pub struct RollingStats {
    window_ms: i64,
    next_seq: u64,
    samples: BTreeMap<u64, Sample>,
    by_ts: BinaryHeap<Reverse<(i64, u64)>>,
}

impl RollingStats {
    /// Construct a fresh tracker with the given window length in
    /// milliseconds. `window_ms <= 0` produces a tracker that retains
    /// nothing and reports rate 0.
    pub fn new(window_ms: i64) -> Self {
        Self {
            window_ms,
            next_seq: 0,
            samples: BTreeMap::new(),
            by_ts: BinaryHeap::new(),
        }
    }

    /// Default per design doc: 5-minute window.
    pub fn default_window() -> Self {
        Self::new(5 * 60 * 1_000)
    }

    /// Observe one event. Pushes a (ts_ms, byte-size) sample and evicts
    /// any samples older than `event.ts_ms - window_ms`.
    pub fn observe(&mut self, event: &Event) {
        if self.window_ms <= 0 {
            return;
        }
        let bytes = u32::try_from(event.payload.len()).unwrap_or(u32::MAX);
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.samples.insert(
            seq,
            Sample {
                ts_ms: event.ts_ms,
                bytes,
            },
        );
        self.by_ts.push(Reverse((event.ts_ms, seq)));
        let cutoff = event.ts_ms.saturating_sub(self.window_ms);
        while let Some(&Reverse((ts, old))) = self.by_ts.peek() {
            if ts >= cutoff {
                break;
            }
            self.by_ts.pop();
            self.samples.remove(&old);
        }
    }

    pub fn window_ms(&self) -> i64 {
        self.window_ms
    }

    pub fn snapshot(&self) -> StatsSnapshot {
        let n = self.samples.len();
        if n == 0 || self.window_ms <= 0 {
            return StatsSnapshot {
                window_ms: self.window_ms,
                ..Default::default()
            };
        }

        let window_seconds = (self.window_ms as f64) / 1000.0;
        let rate_hz = n as f64 / window_seconds;

        let ordered: Vec<Sample> = self.samples.values().copied().collect();
        let mut deltas: Vec<f64> = Vec::with_capacity(n.saturating_sub(1));
        for win in ordered.windows(2) {
            deltas.push((win[1].ts_ms - win[0].ts_ms) as f64);
        }
        // f64 inter-arrival values are non-negative integers cast from
        // i64; total_cmp gives a deterministic strict ordering.
        deltas.sort_by(f64::total_cmp);

        let mut bytes_sorted: Vec<u32> = ordered.iter().map(|s| s.bytes).collect();
        bytes_sorted.sort_unstable();

        StatsSnapshot {
            window_ms: self.window_ms,
            sample_count: n,
            rate_hz,
            inter_arrival_p50_ms: percentile_f64(&deltas, 50),
            inter_arrival_p95_ms: percentile_f64(&deltas, 95),
            inter_arrival_p99_ms: percentile_f64(&deltas, 99),
            byte_p50: percentile_u32(&bytes_sorted, 50),
            byte_p95: percentile_u32(&bytes_sorted, 95),
            byte_p99: percentile_u32(&bytes_sorted, 99),
        }
    }
}

/// Nearest-rank percentile index: `ceil(p/100 * n) - 1`, clamped to
/// `[0, n-1]`. Returns None for empty input. Pure integer math — no
/// float intermediate, no rounding-mode divergence.
fn nearest_rank_index(n: usize, p: u32) -> Option<usize> {
    if n == 0 {
        return None;
    }
    // ceil((p * n) / 100) without floats: (p * n + 99) / 100.
    let numerator = (p as u64).saturating_mul(n as u64);
    let raw = (numerator + 99) / 100;
    let idx = raw.saturating_sub(1) as usize;
    Some(idx.min(n - 1))
}

fn percentile_f64(sorted: &[f64], p: u32) -> f64 {
    nearest_rank_index(sorted.len(), p)
        .map(|i| sorted[i])
        .unwrap_or(0.0)
}

fn percentile_u32(sorted: &[u32], p: u32) -> u32 {
    nearest_rank_index(sorted.len(), p)
        .map(|i| sorted[i])
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(ts_ms: i64, bytes: usize) -> Event {
        Event {
            key: "vendor-a/observations/v1".into(),
            ts_ms,
            payload: vec![0u8; bytes],
            format: None,
        }
    }

    #[test]
    fn empty_tracker_yields_zero_state() {
        let s = RollingStats::default_window().snapshot();
        assert_eq!(s.sample_count, 0);
        assert_eq!(s.rate_hz, 0.0);
        assert_eq!(s.inter_arrival_p50_ms, 0.0);
        assert_eq!(s.byte_p50, 0);
        assert_eq!(s.window_ms, 5 * 60 * 1000);
    }

    #[test]
    fn rate_is_count_per_window_seconds() {
        // 60 samples in a 60-second window → 1 Hz.
        let mut s = RollingStats::new(60_000);
        for i in 0..60 {
            s.observe(&ev(i * 1_000, 100));
        }
        let snap = s.snapshot();
        assert_eq!(snap.sample_count, 60);
        assert!((snap.rate_hz - 1.0).abs() < 1e-9);
    }

    #[test]
    fn samples_outside_window_are_evicted() {
        let mut s = RollingStats::new(10_000); // 10 sec window
        s.observe(&ev(0, 10));
        s.observe(&ev(5_000, 10));
        s.observe(&ev(20_000, 10)); // pushes window to [10_000, 20_000]
        let snap = s.snapshot();
        // Only the ts=20_000 sample survives — ts=0 and ts=5_000 fell
        // outside the new [10_000, 20_000] window.
        assert_eq!(snap.sample_count, 1);
    }

    #[test]
    fn inter_arrival_percentiles_nearest_rank() {
        // 11 samples spaced 1ms apart → 10 deltas, all equal to 1ms.
        let mut s = RollingStats::new(10_000_000);
        for i in 0..11 {
            s.observe(&ev(i, 100));
        }
        let snap = s.snapshot();
        // All deltas are 1ms; every percentile = 1.0.
        assert_eq!(snap.inter_arrival_p50_ms, 1.0);
        assert_eq!(snap.inter_arrival_p95_ms, 1.0);
        assert_eq!(snap.inter_arrival_p99_ms, 1.0);
    }

    #[test]
    fn byte_percentiles_nearest_rank() {
        let mut s = RollingStats::new(10_000_000);
        // 10 samples with byte sizes 1..=10, evenly spaced ts.
        for (i, sz) in (1..=10).enumerate() {
            s.observe(&ev(i as i64 * 1_000, sz));
        }
        let snap = s.snapshot();
        // Sorted: [1,2,3,4,5,6,7,8,9,10]. Nearest-rank indices:
        //   p50 → ceil(50*10/100) - 1 = 5 - 1 = 4 → sorted[4] = 5
        //   p95 → ceil(95*10/100) - 1 = 10 - 1 = 9 → sorted[9] = 10
        //   p99 → ceil(99*10/100) - 1 = 10 - 1 = 9 → sorted[9] = 10
        assert_eq!(snap.byte_p50, 5);
        assert_eq!(snap.byte_p95, 10);
        assert_eq!(snap.byte_p99, 10);
    }

    #[test]
    fn nearest_rank_indices_for_canonical_sizes() {
        // Spot-check the index function directly against the values
        // panoscope's existing rolling-stats commentary uses.
        assert_eq!(nearest_rank_index(0, 50), None);
        assert_eq!(nearest_rank_index(1, 50), Some(0));
        assert_eq!(nearest_rank_index(10, 50), Some(4));
        assert_eq!(nearest_rank_index(100, 50), Some(49));
        assert_eq!(nearest_rank_index(100, 95), Some(94));
        assert_eq!(nearest_rank_index(100, 99), Some(98));
        // p100 clamps to last index.
        assert_eq!(nearest_rank_index(10, 100), Some(9));
    }

    #[test]
    fn single_sample_has_no_inter_arrival_but_has_bytes() {
        let mut s = RollingStats::new(60_000);
        s.observe(&ev(1_000, 42));
        let snap = s.snapshot();
        assert_eq!(snap.sample_count, 1);
        // 1 sample → no deltas exist → percentiles fall through to 0.
        assert_eq!(snap.inter_arrival_p50_ms, 0.0);
        // Byte percentile of a 1-element slice resolves to that element.
        assert_eq!(snap.byte_p50, 42);
        assert_eq!(snap.byte_p95, 42);
        assert_eq!(snap.byte_p99, 42);
    }

    #[test]
    fn nonpositive_window_disables_tracking() {
        let mut s = RollingStats::new(0);
        s.observe(&ev(0, 100));
        s.observe(&ev(1, 100));
        let snap = s.snapshot();
        assert_eq!(snap.sample_count, 0);
        assert_eq!(snap.rate_hz, 0.0);
    }

    /// Reference model: the earlier implementation, which retained by
    /// scanning the whole window on every event.
    fn retain_reference(window_ms: i64, events: &[(i64, usize)]) -> Vec<(i64, u32)> {
        let mut v: Vec<(i64, u32)> = Vec::new();
        for &(ts, bytes) in events {
            v.push((ts, bytes as u32));
            let cutoff = ts.saturating_sub(window_ms);
            v.retain(|&(t, _)| t >= cutoff);
        }
        v
    }

    #[test]
    fn heap_eviction_matches_retain_reference_with_out_of_order_ts() {
        // Deterministic LCG; timestamps jitter backwards and forwards.
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut ts: i64 = 10_000;
        let mut events = Vec::new();
        for _ in 0..2_000 {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            ts += ((x >> 33) % 400) as i64 - 120;
            events.push((ts, ((x >> 20) % 900) as usize));
        }
        let window_ms = 3_000;
        let mut s = RollingStats::new(window_ms);
        for &(t, b) in &events {
            s.observe(&ev(t, b));
        }
        let expected = retain_reference(window_ms, &events);
        let got: Vec<(i64, u32)> = s.samples.values().map(|x| (x.ts_ms, x.bytes)).collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn snapshot_is_serde_stable() {
        // Phase D equivalence-test gate: same input → byte-equal JSON.
        // Run twice; assert the serialized output is identical so any
        // future divergence (HashMap-style ordering bug, float locale
        // drift) gets caught by the unit test before CI sees it.
        let mut a = RollingStats::new(10_000);
        let mut b = RollingStats::new(10_000);
        for i in 0..10 {
            let event = ev(i * 100, (i % 5) as usize * 10);
            a.observe(&event);
            b.observe(&event);
        }
        let ja = serde_json::to_string(&a.snapshot()).unwrap();
        let nova = serde_json::to_string(&b.snapshot()).unwrap();
        assert_eq!(ja, nova);
    }
}
