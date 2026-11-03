// SPDX-License-Identifier: Apache-2.0
//
// Z-score anomaly detection over the per-event inter-arrival series.
//
// Streaming mean + variance via Welford's online algorithm. The
// returned z-score is `(latest_sample - mean) / stddev`, where the
// mean and stddev come from the full history of samples observed so
// far. Operator-facing semantics: "how unusual is the current event's
// spacing relative to the long-term mean spacing?"
//
// Equivalence: Welford is order-sensitive (each step depends on the
// current running mean). The Phase D equivalence-test harness feeds
// events in the same order across native + WASM, so order-sensitivity
// doesn't break byte-equal output. Welford itself uses only f64 add/
// sub/mul/div + cast-to-f64 — all IEEE 754, all target-independent.

use serde::{Deserialize, Serialize};

/// Snapshot of the anomaly detector's running state. Serializes
/// deterministically — field-order fixed, all-f64.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnomalySnapshot {
    /// Number of samples observed so far.
    pub sample_count: u64,
    /// Running mean of all observed samples.
    pub mean: f64,
    /// Running standard deviation (sqrt of Welford's sample variance).
    /// Zero until `sample_count >= 2`.
    pub stddev: f64,
    /// Value of the most recently observed sample, if any. None when
    /// no sample has been observed yet.
    pub latest_sample: Option<f64>,
    /// `(latest_sample - mean) / stddev`. None when fewer than 2
    /// samples exist or when variance is degenerate (all samples
    /// equal — stddev is exactly 0, z would be NaN/inf).
    pub latest_z_score: Option<f64>,
}

/// Welford-online running statistics with latest-sample tracking.
#[derive(Debug, Default, Clone)]
pub struct AnomalyDetector {
    n: u64,
    mean: f64,
    /// M2 = sum of squared differences from the current mean.
    /// Welford's recurrence: M2_n = M2_{n-1} + (x_n - mean_{n-1}) * (x_n - mean_n).
    m2: f64,
    latest_sample: Option<f64>,
}

impl AnomalyDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe one sample and update the Welford state in place.
    pub fn observe(&mut self, sample: f64) {
        self.n = self.n.saturating_add(1);
        let delta = sample - self.mean;
        self.mean += delta / self.n as f64;
        let delta2 = sample - self.mean;
        self.m2 += delta * delta2;
        self.latest_sample = Some(sample);
    }

    /// `(sample - mean) / stddev`. Returns None when fewer than 2
    /// samples have been observed (variance is undefined) or when
    /// variance is exactly zero (all-equal samples — z-score would be
    /// NaN or ±infinity, neither of which is operator-actionable).
    pub fn z_score(&self, sample: f64) -> Option<f64> {
        if self.n < 2 {
            return None;
        }
        let variance = self.m2 / (self.n - 1) as f64;
        if variance <= 0.0 {
            return None;
        }
        Some((sample - self.mean) / variance.sqrt())
    }

    /// Running stddev. Zero when fewer than 2 samples — distinguishes
    /// "no data yet" from "no spread observed" via `sample_count` on
    /// the snapshot.
    pub fn stddev(&self) -> f64 {
        if self.n < 2 {
            return 0.0;
        }
        let variance = self.m2 / (self.n - 1) as f64;
        if variance <= 0.0 {
            return 0.0;
        }
        variance.sqrt()
    }

    pub fn snapshot(&self) -> AnomalySnapshot {
        AnomalySnapshot {
            sample_count: self.n,
            mean: if self.n == 0 { 0.0 } else { self.mean },
            stddev: self.stddev(),
            latest_sample: self.latest_sample,
            latest_z_score: self
                .latest_sample
                .and_then(|s| self.z_score(s)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_detector_snapshot_is_zero_state() {
        let snap = AnomalyDetector::new().snapshot();
        assert_eq!(snap.sample_count, 0);
        assert_eq!(snap.mean, 0.0);
        assert_eq!(snap.stddev, 0.0);
        assert!(snap.latest_sample.is_none());
        assert!(snap.latest_z_score.is_none());
    }

    #[test]
    fn single_sample_has_no_z_score() {
        let mut d = AnomalyDetector::new();
        d.observe(42.0);
        let snap = d.snapshot();
        assert_eq!(snap.sample_count, 1);
        assert_eq!(snap.mean, 42.0);
        assert_eq!(snap.stddev, 0.0);
        assert_eq!(snap.latest_sample, Some(42.0));
        // Welford variance is m2/(n-1) — undefined for n<2.
        assert!(snap.latest_z_score.is_none());
    }

    #[test]
    fn welford_mean_and_stddev_match_textbook_for_small_series() {
        // Series: 2, 4, 4, 4, 5, 5, 7, 9 (textbook Welford example).
        // Sample mean = 5, sample variance = 32/7 ≈ 4.571428..., so
        // sample stddev = sqrt(32/7) ≈ 2.13808993...
        let mut d = AnomalyDetector::new();
        for s in [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0] {
            d.observe(s);
        }
        let snap = d.snapshot();
        assert_eq!(snap.sample_count, 8);
        assert!((snap.mean - 5.0).abs() < 1e-9);
        assert!((snap.stddev - (32.0_f64 / 7.0).sqrt()).abs() < 1e-9);
    }

    #[test]
    fn z_score_centers_on_mean() {
        // Series of {1, 2, 3, 4, 5} → mean = 3, sample variance =
        // sum((x-3)^2) / (n-1) = 10/4 = 2.5, stddev = sqrt(2.5).
        let mut d = AnomalyDetector::new();
        for s in [1.0, 2.0, 3.0, 4.0, 5.0] {
            d.observe(s);
        }
        let stddev = 2.5_f64.sqrt();
        // z-score at the mean = 0.
        assert!((d.z_score(3.0).unwrap() - 0.0).abs() < 1e-9);
        // z-score at mean + 1 stddev = 1.
        assert!((d.z_score(3.0 + stddev).unwrap() - 1.0).abs() < 1e-9);
        // z-score at mean - 1 stddev = -1.
        assert!((d.z_score(3.0 - stddev).unwrap() - -1.0).abs() < 1e-9);
    }

    #[test]
    fn all_equal_samples_yield_none_z_score() {
        // Welford variance = 0; z-score would be NaN. Detector returns
        // None instead — operator-facing surface stays clean.
        let mut d = AnomalyDetector::new();
        for _ in 0..10 {
            d.observe(1.0);
        }
        let snap = d.snapshot();
        assert_eq!(snap.sample_count, 10);
        assert_eq!(snap.mean, 1.0);
        assert_eq!(snap.stddev, 0.0);
        assert_eq!(snap.latest_sample, Some(1.0));
        assert!(snap.latest_z_score.is_none());
    }

    #[test]
    fn snapshot_latest_z_score_uses_most_recent_sample() {
        let mut d = AnomalyDetector::new();
        for s in [1.0, 2.0, 3.0, 4.0, 5.0] {
            d.observe(s);
        }
        // Then observe a far outlier.
        d.observe(100.0);
        let snap = d.snapshot();
        assert_eq!(snap.latest_sample, Some(100.0));
        // The most recent sample should have a large positive z-score.
        let z = snap.latest_z_score.unwrap();
        assert!(z > 2.0, "expected outlier z-score > 2, got {z}");
    }

    #[test]
    fn snapshot_is_serde_stable() {
        // Phase D equivalence-test gate: same input → byte-equal JSON.
        let mut a = AnomalyDetector::new();
        let mut b = AnomalyDetector::new();
        for s in [1.0, 2.0, 3.0, 4.0, 5.0, 10.0, 20.0] {
            a.observe(s);
            b.observe(s);
        }
        let ja = serde_json::to_string(&a.snapshot()).unwrap();
        let nova = serde_json::to_string(&b.snapshot()).unwrap();
        assert_eq!(ja, nova);
    }
}
