// SPDX-License-Identifier: Apache-2.0
//
// Higher-fidelity sparkline binning. Tier 1's `flashlight` SSE page
// already ships a coarse Canvas 2D sparkline driven by raw event-rate;
// Tier 2 augments it with finer-grained bin counts computed by this
// module. Same source data, different visual fidelity.
//
// Bin placement: window-relative — the most recently observed event's
// `ts_ms` anchors the right edge of the window; the left edge is at
// `window_end - window_ms`. Each bin spans `window_ms / bin_count`
// milliseconds. f64 division is used for placement (i64 numerator,
// i64 denominator → exact integer cast for values within window
// range, since they're all < 2^53). usize cast truncates toward zero;
// the right-edge sample at exactly window_end clamps to the last bin.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use serde::{Deserialize, Serialize};

/// Snapshot of the sparkline tracker. Serializes deterministically —
/// fixed field order, `points` is a `Vec<f64>` of length `bin_count`.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct SparklineSnapshot {
    pub window_ms: i64,
    pub bin_count: usize,
    /// Bin counts. `points[0]` is the oldest bin (left edge); the
    /// last element is the most-recent bin (right edge / current
    /// moment in time). f64 even though counts are integer — leaves
    /// room for normalization / smoothing variants without breaking
    /// the wire shape.
    pub points: Vec<f64>,
}

/// Rolling sparkline tracker. Retains in-window timestamps; recomputes
/// bin counts on snapshot. Timestamps sit in a min-heap so eviction pops
/// only the expired ones (O(log n) per event) instead of scanning the
/// whole window; binning is order-independent, so the heap order is
/// never observed.
#[derive(Debug, Clone)]
pub struct Sparkline {
    bin_count: usize,
    window_ms: i64,
    timestamps: BinaryHeap<Reverse<i64>>,
}

impl Sparkline {
    /// Construct with operator-supplied bin count + window length.
    pub fn new(bin_count: usize, window_ms: i64) -> Self {
        Self {
            bin_count,
            window_ms,
            timestamps: BinaryHeap::new(),
        }
    }

    /// Default per design doc: 5-minute window, 60 bins (5-second
    /// bins). Matches the granularity Tier 1's coarse sparkline aims
    /// at while keeping the wire payload tiny.
    pub fn default_window() -> Self {
        Self::new(60, 5 * 60 * 1_000)
    }

    pub fn bin_count(&self) -> usize {
        self.bin_count
    }

    pub fn window_ms(&self) -> i64 {
        self.window_ms
    }

    /// Push an event timestamp. Evicts any timestamps older than
    /// `ts_ms - window_ms`.
    pub fn observe(&mut self, ts_ms: i64) {
        if self.window_ms <= 0 || self.bin_count == 0 {
            return;
        }
        self.timestamps.push(Reverse(ts_ms));
        let cutoff = ts_ms.saturating_sub(self.window_ms);
        while let Some(&Reverse(t)) = self.timestamps.peek() {
            if t >= cutoff {
                break;
            }
            self.timestamps.pop();
        }
    }

    pub fn snapshot(&self) -> SparklineSnapshot {
        if self.bin_count == 0 || self.window_ms <= 0 || self.timestamps.is_empty() {
            return SparklineSnapshot {
                window_ms: self.window_ms,
                bin_count: self.bin_count,
                points: vec![0.0; self.bin_count],
            };
        }

        // Right edge of the window = most-recent timestamp; left edge
        // = right - window_ms. Bins divide [left, right] evenly.
        let window_end = self
            .timestamps
            .iter()
            .map(|&Reverse(t)| t)
            .max()
            .expect("non-empty");
        let window_start = window_end.saturating_sub(self.window_ms);
        let window_ms = self.window_ms as f64;
        let bin_count = self.bin_count as f64;

        let mut counts = vec![0_u64; self.bin_count];
        for &Reverse(ts) in &self.timestamps {
            if ts < window_start {
                // Eviction is by ts_ms.saturating_sub(window_ms) so the
                // strictly-less case shouldn't happen, but guard so an
                // out-of-order historical sample never wraps to bin N.
                continue;
            }
            // Position in [0, window_ms]; ratio in [0, 1]; bin in
            // [0, bin_count]. Right-edge sample at ratio==1 clamps to
            // the last bin (otherwise it'd land at index = bin_count
            // which is out of range).
            let offset = (ts - window_start) as f64;
            let bin_f = (offset / window_ms) * bin_count;
            let bin = (bin_f as usize).min(self.bin_count - 1);
            counts[bin] = counts[bin].saturating_add(1);
        }

        SparklineSnapshot {
            window_ms: self.window_ms,
            bin_count: self.bin_count,
            points: counts.into_iter().map(|n| n as f64).collect(),
        }
    }
}

impl Default for Sparkline {
    fn default() -> Self {
        Self::default_window()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_tracker_yields_zero_points() {
        let snap = Sparkline::new(10, 1_000).snapshot();
        assert_eq!(snap.window_ms, 1_000);
        assert_eq!(snap.bin_count, 10);
        assert_eq!(snap.points, vec![0.0; 10]);
    }

    #[test]
    fn observations_skew_right_when_evenly_spaced_at_bin_boundaries() {
        // 10-second window, 10 bins → bin_width = 1000ms. 10 events at
        // ts = 100, 1_100, ..., 9_100. Right edge = 9_100, left edge =
        // -900. Per-event offset from window_start = 1000, 2000, ...,
        // 10000. Lower-bound-inclusive binning + right-edge clamp:
        // offset 1000 → bin 1, offset 2000 → bin 2, ..., offset 9000 →
        // bin 9, offset 10000 → bin 10 → clamp to 9. So bin 0 empty,
        // bins 1..8 get one each, bin 9 gets two. Recent activity
        // skews to the right of the sparkline — operator-meaningful.
        let mut s = Sparkline::new(10, 10_000);
        for i in 0..10 {
            s.observe(100 + i * 1_000);
        }
        let snap = s.snapshot();
        assert_eq!(snap.points, vec![0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 2.0]);
    }

    #[test]
    fn samples_outside_window_are_evicted() {
        let mut s = Sparkline::new(5, 1_000); // 5 bins, 1 sec window
        s.observe(0);
        s.observe(500);
        s.observe(2_000); // pushes window to [1_000, 2_000]
        let snap = s.snapshot();
        // Only the ts=2_000 sample survives. Right-edge sample lands
        // in the last bin (index 4).
        assert_eq!(snap.points, vec![0.0, 0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn right_edge_sample_clamps_to_last_bin() {
        let mut s = Sparkline::new(4, 1_000);
        s.observe(1_000); // Sole sample; window = [0, 1_000].
        let snap = s.snapshot();
        // Ratio 1_000/1_000 = 1.0 → bin_f = 4.0 → usize 4 → clamp to 3.
        assert_eq!(snap.points, vec![0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn single_sample_lands_in_last_bin_via_clamp() {
        let mut s = Sparkline::new(4, 1_000);
        s.observe(0);
        let snap = s.snapshot();
        // Window-relative: this is the only sample, so it IS the
        // right edge. window_end = 0, window_start = -1000. Offset
        // = 1000, ratio = 1.0, bin_f = 4.0 → clamp to 3 (last bin).
        // Operator-meaningful: a lone event is the most recent
        // event, so it sits at the right edge of the sparkline.
        assert_eq!(snap.points, vec![0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn multiple_events_at_same_ts_all_land_in_last_bin() {
        let mut s = Sparkline::new(2, 1_000); // 2 bins, 1 sec window
        for _ in 0..5 {
            s.observe(250);
        }
        let snap = s.snapshot();
        // All five events share the right edge → all in last bin.
        assert_eq!(snap.points, vec![0.0, 5.0]);
    }

    #[test]
    fn nonpositive_window_or_zero_bins_disables_tracking() {
        let mut s_no_window = Sparkline::new(10, 0);
        s_no_window.observe(0);
        s_no_window.observe(1);
        assert_eq!(s_no_window.snapshot().points, vec![0.0; 10]);

        let mut s_no_bins = Sparkline::new(0, 1_000);
        s_no_bins.observe(0);
        // 0 bins → empty points vec.
        assert!(s_no_bins.snapshot().points.is_empty());
    }

    #[test]
    fn default_window_has_60_bins_over_5_minutes() {
        let s = Sparkline::default_window();
        assert_eq!(s.bin_count(), 60);
        assert_eq!(s.window_ms(), 5 * 60 * 1_000);
    }

    #[test]
    fn heap_eviction_matches_retain_reference_with_out_of_order_ts() {
        let mut x: u64 = 0x2545_f491_4f6c_dd1d;
        let mut ts: i64 = 50_000;
        let mut reference: Vec<i64> = Vec::new();
        let mut s = Sparkline::new(12, 2_000);
        for _ in 0..2_000 {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            ts += ((x >> 33) % 300) as i64 - 90;
            s.observe(ts);
            reference.push(ts);
            let cutoff = ts.saturating_sub(2_000);
            reference.retain(|&t| t >= cutoff);
        }
        let mut got: Vec<i64> = s.timestamps.iter().map(|&Reverse(t)| t).collect();
        got.sort_unstable();
        reference.sort_unstable();
        assert_eq!(got, reference);
    }

    #[test]
    fn snapshot_is_serde_stable() {
        // Phase D equivalence-test gate: same input → byte-equal JSON.
        let mut a = Sparkline::new(8, 10_000);
        let mut b = Sparkline::new(8, 10_000);
        for ts in [100, 500, 1_200, 2_300, 3_400, 5_000, 7_500, 9_900] {
            a.observe(ts);
            b.observe(ts);
        }
        let ja = serde_json::to_string(&a.snapshot()).unwrap();
        let nova = serde_json::to_string(&b.snapshot()).unwrap();
        assert_eq!(ja, nova);
    }
}
