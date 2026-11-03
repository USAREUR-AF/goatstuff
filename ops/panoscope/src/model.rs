// SPDX-License-Identifier: Apache-2.0
//
// Panoscope-specific viz state (FlowState, EdgeState, EdgeKey, TokenBucket).
// Substrate-layer concerns — PeerId, TopicKey, Role, NodeKind, PeerInfo,
// Topology, key-expression helpers, now_ms, extract_vendor — were factored
// out into the `panoscope-core` crate (docs/design/flashlight.md Phase B).
//
// They are re-exported below so existing consumers (`crate::model::*`) keep
// compiling unchanged.

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};

// Re-export substrate-layer types from panoscope-core so existing
// consumers using `crate::model::*` paths are unchanged.
// `#[allow(unused_imports)]` because some helpers (extract_vendor,
// keyexprs_related, strip_wildcards) are re-exported for the public
// API but not used inside panoscope itself.
#[allow(unused_imports)]
pub use panoscope_core::{
    extract_vendor, keyexprs_related, now_ms, strip_wildcards, NodeClass, NodeKind, PeerId,
    PeerInfo, Role, Topology, TopicKey,
};

// ---------------------------------------------------------------------------
// EdgeState + EMA decay (panoscope-specific viz state)
// ---------------------------------------------------------------------------

/// One edge of the flow graph — publisher peer → topic → subscriber peer.
/// In v1 we don't always know the subscriber, so `sink` is optional;
/// edges with `sink = None` represent observed publishes whose downstream
/// subscribers haven't been mapped yet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeState {
    pub source: PeerId,
    pub sink: Option<PeerId>,
    pub topic: TopicKey,
    /// Events-per-second rate: a count of events per closed one-second
    /// bucket, smoothed by an exponential moving average (time constant
    /// `tau_secs`). A COUNT, never one-over-gap — see `record_event`.
    pub ema_eps: f64,
    /// Last event timestamp (millis since UNIX epoch).
    pub last_event_ms: u64,
    /// Start of the open counting bucket (millis since UNIX epoch); `None`
    /// until the first event. `#[serde(default)]` so a state snapshot
    /// written before this field existed still loads.
    #[serde(default)]
    pub bucket_start_ms: Option<u64>,
    /// Events counted in the open bucket so far.
    #[serde(default)]
    pub bucket_count: u32,
    /// Total events observed since panoscope start.
    pub total_events: u64,
    /// Schema family resolved by the SchemaResolver; `None` when unresolved.
    pub schema_family: Option<String>,
}

impl EdgeState {
    pub fn new(source: PeerId, topic: TopicKey, now_ms: u64) -> Self {
        Self {
            source,
            sink: None,
            topic,
            ema_eps: 0.0,
            last_event_ms: now_ms,
            total_events: 0,
            schema_family: None,
            bucket_start_ms: None,
            bucket_count: 0,
        }
    }

    /// Record an event at time `now_ms`.
    ///
    /// The rate is a COUNT of events per second, not an average of
    /// one-over-gap. The old form (`ema += (1/dt) * (1 - decay)`) read a
    /// bursty publisher at several times its true rate: thirteen messages
    /// inside one millisecond once a second is 13 msg/s, but each of those
    /// twelve sub-millisecond gaps contributed a 1000 msg/s sample to the
    /// average. That reads a bursty publisher at about three times its rate
    /// and trips the packet-dot throttle on a quiet fabric. Events are now
    /// counted into a one-second bucket; when the bucket closes its count
    /// over its length is folded into the EMA. The fabric-health meter
    /// (`ops/fabric/footprint/<peer>`) counts the same way, which is why
    /// /top and the COP now agree.
    pub fn record_event(&mut self, now_ms: u64, tau_secs: f64) {
        let start = *self.bucket_start_ms.get_or_insert(now_ms);
        // Close a bucket that is already a second old BEFORE counting this
        // event, so the event that opens a new bucket stays with the rest
        // of its burst instead of being folded away from it.
        if now_ms.saturating_sub(start) >= RATE_BUCKET_MS {
            self.fold_bucket(now_ms, tau_secs);
        }
        self.bucket_count = self.bucket_count.saturating_add(1);
        self.last_event_ms = now_ms;
        self.total_events = self.total_events.saturating_add(1);
    }

    /// Close the open bucket at `now_ms`: fold its count-per-second into
    /// the EMA and start a new bucket.
    fn fold_bucket(&mut self, now_ms: u64, tau_secs: f64) {
        let Some(start) = self.bucket_start_ms else { return };
        let elapsed_secs = now_ms.saturating_sub(start) as f64 / 1000.0;
        if elapsed_secs <= 0.0 {
            return;
        }
        let rate = self.bucket_count as f64 / elapsed_secs;
        let decay = (-elapsed_secs / tau_secs).exp();
        self.ema_eps = self.ema_eps * decay + rate * (1.0 - decay);
        self.bucket_start_ms = Some(now_ms);
        self.bucket_count = 0;
    }

    /// The rate as of `now_ms` without recording an event — for read
    /// paths. The larger of two honest numbers: the closed-bucket EMA,
    /// decayed only for silence beyond one bucket length (a bucket is
    /// normally open for up to a second, and that is not silence), and the
    /// open bucket's count over at least one full bucket length. A burst
    /// that just landed reads as its count per second, steady traffic
    /// reads the same mid-bucket as after a fold, and a quiet edge fades
    /// toward zero with the time constant.
    pub fn projected_eps(&self, now_ms: u64, tau_secs: f64) -> f64 {
        let Some(start) = self.bucket_start_ms else { return 0.0 };
        let bucket_secs = RATE_BUCKET_MS as f64 / 1000.0;
        let elapsed_secs = now_ms.saturating_sub(start) as f64 / 1000.0;
        // Time past one bucket length with no new event is silence; an open
        // bucket only closes on the next event, so both terms fade with it.
        let silence_secs = (elapsed_secs - bucket_secs).max(0.0);
        let fade = (-silence_secs / tau_secs).exp();
        let pending_rate = self.bucket_count as f64 / elapsed_secs.max(bucket_secs);
        self.ema_eps.max(pending_rate) * fade
    }
}

/// Length of one rate-counting bucket. One second: the rate is "events per
/// second", so a bucket shorter than that would re-introduce the burst
/// inflation the count is there to remove.
pub const RATE_BUCKET_MS: u64 = 1_000;

// ---------------------------------------------------------------------------
// TokenBucket
// ---------------------------------------------------------------------------

/// Per-key sampling rate limiter. Defaults sized per the design doc:
/// 50 events/sec capacity, refill 50 events/sec.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    pub capacity: f64,
    pub refill_per_sec: f64,
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucket {
    pub fn new(capacity: f64, refill_per_sec: f64) -> Self {
        Self {
            capacity,
            refill_per_sec,
            tokens: capacity,
            last_refill: Instant::now(),
        }
    }

    /// Attempt to consume one token at `now`. Returns true if granted.
    pub fn try_consume(&mut self, now: Instant) -> bool {
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        self.last_refill = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    #[allow(dead_code)] // surfaced on /api/stats in 82Q.3
    pub fn available(&self) -> f64 {
        self.tokens
    }
}

// ---------------------------------------------------------------------------
// FlowState — per-edge flow aggregator (panoscope-specific)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct FlowState {
    /// Keyed by (source_peer, topic). Subscriber resolution happens at
    /// snapshot time by intersecting with Topology subscribes.
    pub edges: HashMap<EdgeKey, EdgeState>,
    /// Per-topic sampling counters, surfaced in /legend for visibility
    /// into how much we're dropping.
    pub sampling_drops: HashMap<TopicKey, u64>,
}

/// Composite key for FlowState.edges.
///
/// A struct key means the containing map cannot be serialised to JSON as a
/// map (object keys must be strings; serde_json fails with "key must be a
/// string"). Anything writing FlowState to disk must flatten the map to pairs;
/// see `state_snapshot::FlowStateDisk`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeKey {
    pub source: PeerId,
    pub topic: TopicKey,
}

impl EdgeKey {
    pub fn new(source: PeerId, topic: TopicKey) -> Self {
        Self { source, topic }
    }
}

impl FlowState {
    pub fn record_event(
        &mut self,
        source: PeerId,
        topic: TopicKey,
        now_ms: u64,
        tau_secs: f64,
    ) {
        let key = EdgeKey::new(source.clone(), topic.clone());
        let edge = self
            .edges
            .entry(key)
            .or_insert_with(|| EdgeState::new(source, topic, now_ms));
        edge.record_event(now_ms, tau_secs);
    }

    pub fn record_drop(&mut self, topic: TopicKey) {
        *self.sampling_drops.entry(topic).or_insert(0) += 1;
    }
}

// ---------------------------------------------------------------------------
// Tests — panoscope-specific behavior only. Substrate-layer tests live in
// panoscope-core alongside the moved code.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn rate_decays_over_time() {
        let mut e = EdgeState::new("p1".into(), "t".into(), 0);
        // 10 Hz for 60 s (well past the 5 s time constant), then silence.
        for i in 0..620u64 {
            e.record_event(i * 100, 5.0);
        }
        // Read mid-stream (traffic still flowing at 60 s).
        let live = e.projected_eps(60_500, 5.0);
        assert!((live - 10.0).abs() < 1.0, "steady 10 Hz reads {live}");
        // 30 s of silence: approaches zero.
        let projected = e.projected_eps(60_000 + 30_000, 5.0);
        assert!(projected < live * 0.01, "projected {projected} should be < 1% of {live}");
    }

    #[test]
    fn same_millisecond_events_count_once_each() {
        let mut e = EdgeState::new("p1".into(), "t".into(), 100);
        e.record_event(100, 5.0);
        e.record_event(100, 5.0); // dt = 0: counted, never a 1/0 rate
        assert_eq!(e.total_events, 2);
        assert_eq!(e.bucket_count, 2);
        // Read a full second later: two events over one second.
        let r = e.projected_eps(1_100, 5.0);
        assert!((r - 2.0).abs() < 0.2, "two events in one bucket read {r}");
    }

    /// The case that motivates the count: 13 messages inside a
    /// millisecond, once a second. True rate 13 msg/s. The one-over-gap
    /// average reads this shape at ~3x.
    #[test]
    fn bursty_publisher_reads_as_its_count_not_its_gaps() {
        let mut e = EdgeState::new("p1".into(), "t".into(), 0);
        let mut last = 0;
        for sec in 0..30u64 {
            for k in 0..13u64 {
                last = sec * 1_000 + k / 10; // 13 events across ~1.3 ms
                e.record_event(last, 5.0);
            }
        }
        let r = e.projected_eps(last + 1_000, 5.0);
        assert!((r - 13.0).abs() < 1.5, "bursty 13/s reads {r}");
        assert!(r < 20.0, "burst inflation is back: {r}");
    }

    /// Steady traffic reads the same whether the EMA is read mid-bucket
    /// or right after a fold.
    #[test]
    fn steady_rate_is_stable_across_bucket_boundaries() {
        let mut e = EdgeState::new("p1".into(), "t".into(), 0);
        for i in 0..1240u64 {
            e.record_event(i * 50, 5.0); // 20 Hz through 62 s
        }
        for probe in [60_000u64, 60_250, 60_500, 60_999] {
            let r = e.projected_eps(probe, 5.0);
            assert!((r - 20.0).abs() < 2.5, "20 Hz read at +{}ms as {r}", probe - 60_000);
        }
    }

    #[test]
    fn token_bucket_allows_burst_then_refills() {
        let mut tb = TokenBucket::new(5.0, 1.0); // 5 tokens, refill 1/sec
        let t0 = Instant::now();
        // Burst — consume 5
        for _ in 0..5 {
            assert!(tb.try_consume(t0));
        }
        // 6th in same instant — denied
        assert!(!tb.try_consume(t0));
        // After 2 seconds, ~2 tokens refilled
        let t2 = t0 + Duration::from_secs(2);
        assert!(tb.try_consume(t2));
        assert!(tb.try_consume(t2));
        // 3rd in t2 — denied (only ~2 refilled, both consumed)
        assert!(!tb.try_consume(t2));
    }

    #[test]
    fn token_bucket_caps_at_capacity() {
        let mut tb = TokenBucket::new(5.0, 100.0);
        let t0 = Instant::now();
        // Drain to zero
        for _ in 0..5 {
            tb.try_consume(t0);
        }
        // Wait 10 seconds — refill is 100/sec, but capacity is 5
        let t10 = t0 + Duration::from_secs(10);
        tb.try_consume(t10);
        // Only 4 should remain (5 - 1 just consumed), NOT 999
        assert!(tb.available() <= 5.0);
        assert!(tb.available() >= 3.9);
    }

    #[test]
    fn flowstate_record_event_creates_edge() {
        let mut fs = FlowState::default();
        fs.record_event("p1".into(), "vendor-b/sensorgrid/raw".into(), 100, 5.0);
        fs.record_event("p1".into(), "vendor-b/sensorgrid/raw".into(), 200, 5.0);
        assert_eq!(fs.edges.len(), 1);
        let edge = fs.edges.values().next().unwrap();
        assert_eq!(edge.total_events, 2);
        assert!(edge.projected_eps(1_200, 5.0) > 0.0);
    }

    #[test]
    fn flowstate_records_drops() {
        let mut fs = FlowState::default();
        fs.record_drop("topic-a".into());
        fs.record_drop("topic-a".into());
        fs.record_drop("topic-b".into());
        assert_eq!(*fs.sampling_drops.get("topic-a").unwrap(), 2);
        assert_eq!(*fs.sampling_drops.get("topic-b").unwrap(), 1);
    }
}
