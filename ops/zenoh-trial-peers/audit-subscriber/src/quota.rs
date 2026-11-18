//! Per-producer audit ingress metering, with opt-in enforcement (GHSA-hq6c).
//!
//! Every persisted sample becomes one audit record in a retention-locked
//! bucket, so an authorized producer (or a stolen producer credential) sets the
//! store's growth rate with nothing but its own publish rate. This module
//! meters the rate at which each producer adds records, before anything is
//! hashed or queued, and alerts when one goes over its budget.
//!
//! OBSERVE-ONLY BY DEFAULT. Nothing is dropped unless enforcement is turned on
//! (`GOAT_AUDIT_PRODUCER_QUOTA_ENFORCE=true`). The subscriber sees keys, not
//! credentials, so a bucket is a key prefix, not a writer identity, and an
//! enforcing cap would let one producer get another's records refused wherever
//! two writers share a bucket. Known limits under enforcement:
//!   - Shared buckets: `ops/names/claim` and `attest/schema` are writable by
//!     every peer; `ops/consistency/report`, `ops/time/attest`,
//!     `ops/net/health` are one certificate on every node; a multi-publisher
//!     topic is shared by its co-publishers. Any writer in a shared bucket can
//!     exhaust it for all the others.
//!   - Overlapping grants: a wider grant (a fabric-health leaf's
//!     `ops/fabric/**`, a federated router's `ops/**`) can drain a narrower
//!     writer's bucket (`ops/fabric/topology`, `ops/time/attest`, `ops`).
//!   - Bucket growth: under `from/`, `release/`, `hello/` and `multipub/`
//!     the bucket is chosen by the next segment(s); a vendor registered under
//!     one of those names chooses them. Each such namespace keeps at most 256
//!     buckets FOR THE LIFE OF THE PROCESS, then charges `<namespace>/~other`:
//!     whoever fills the 256 first (for example just after a restart) shares
//!     `<namespace>/~other` with every legitimate name seen later, and under
//!     enforcement can keep it drained. Each of those namespaces also has an
//!     aggregate budget (`<namespace>/*`, the per-bucket budget times
//!     GOAT_AUDIT_PRODUCER_QUOTA_NAMESPACE_FACTOR); under enforcement a sample
//!     over the aggregate is dropped too, so one writer spread over many
//!     buckets can exhaust the whole namespace for the others. The aggregate is
//!     shared across identities: a slot holding several co-publish grants can
//!     use them to starve an unrelated multi-publisher topic (`multipub/*`),
//!     and `from/*` covers every federated sovereign, so one sovereign can
//!     starve the others.
//!   - `ops/audit/**` is never dropped, under enforcement too; it is metered
//!     and alerted like everything else. Its writers (the operator, a
//!     federated router, the surface-visit leaf) therefore have no rate bound.
//!   - The writer queue is shared: the burst (default 2,000 records, under
//!     half the 4,096-sample queue) holds PER BUCKET only. `ops/audit/**`, or
//!     one credential writing several buckets at once (its anchor plus
//!     `attest/schema`, `ops/names/claim`, `release/<set>`, a multipub topic),
//!     can still overflow the queue, and then OTHER producers' records are
//!     dropped (counted, AuditSubscriberQueueDropping).
//!   - The subscriber counts only what reaches it: a flood faster than it can
//!     receive backs up into the router, which drops put samples to it under
//!     its congestion policy, for every producer, with no metric here.
//!
//! Per-credential limits need the writer's identity at the subscriber, or
//! limits at the router; neither is in this release.
//!
//! What it does not do: it bounds nothing in observe-only mode, and even
//! enforcing it bounds rate, not total storage. Total storage is bounded only by
//! the bucket's own quota and retention (minio_audit_bucket_quota_gib,
//! lifecycle expiry).
//!
//! Who is a producer (the bucket a sample is metered to), from the router's
//! ACL ingress put rules (goat-zenoh config.json5.j2):
//!   - The first key segment for everything a single identity owns: a vendor
//!     prefix, a slot anchor, `keys`, `founder`.
//!   - `from/<sovereign>`, `release/<set>`, `hello/<who>`: the next segment.
//!   - `multipub/<pool>/<topic>`: one bucket per multi-publisher topic.
//!   - Under `ops`, each infra writer's own prefix (`ops/time/attest`, ...);
//!     `ops/names/claim` and `attest/schema` are one bucket each.
//!   - `ops/audit/operator`, `ops/audit/surface`, and `ops/audit` for the rest
//!     of that subtree, metered and never dropped.
//!
//! A namespace added to the ACL without a line here falls back to its first
//! segment.
//!
//! The budget is a token bucket per producer: `records` and `bytes` per
//! `window` on average, at most `burst` seconds' worth at once. Bytes are
//! charged at an upper bound of what is stored for the sample, under both the
//! v1 record (one record, payload inline) and the v2 record (an audit record
//! plus, with replay on, a replay record repeating its fields with the payload):
//! the JSON-escaped key six times (key_expr, anchor and vendor in each of two
//! records; a control character in a key is stored as a 6-byte `\u00XX`), the
//! payload as base64 up to the inline cap (past it only a hash is stored), and
//! an envelope allowance for both records' other fields.
//!
//! The caller-chosen namespaces also carry an aggregate budget,
//! `<namespace>/*`: the per-bucket budget times
//! GOAT_AUDIT_PRODUCER_QUOTA_NAMESPACE_FACTOR (default 4: 2,000 records/s,
//! 43x the whole measured fabric), so one writer spread over many buckets
//! still trips AuditProducerOverRate.
//!
//! Fail visible, never silent: every sample beyond its producer's budget is
//! counted in `goat_audit_producer_over_rate_total{vendor,limit}` (alert
//! AuditProducerOverRate); when enforcing, those that were refused are also
//! counted in `goat_audit_quota_dropped_total{vendor,limit}` and
//! `goat_audit_quota_dropped_bytes_total{vendor}`. Each producer's series is
//! created at 0 the first time it is seen, so the first excess is an increase a
//! rule can see. The first excess for a producer in a window logs a WARN; the
//! window's totals are logged when it closes (ERROR when records were dropped).
//! Records are never sampled.
//!
//! Configuration (environment). `0` disables a limit; both `0` disables
//! metering. An unparsable value is logged at ERROR and the default is used:
//!   GOAT_AUDIT_PRODUCER_QUOTA_ENFORCE      true to drop over-budget samples (default false)
//!   GOAT_AUDIT_PRODUCER_QUOTA_RECORDS      records per producer per window
//!   GOAT_AUDIT_PRODUCER_QUOTA_BYTES        charged bytes per producer per window
//!   GOAT_AUDIT_PRODUCER_QUOTA_WINDOW_SECS  averaging window in seconds
//!   GOAT_AUDIT_PRODUCER_QUOTA_BURST_SECS   seconds of budget admitted at once
//!   GOAT_AUDIT_PRODUCER_QUOTA_MAX_TRACKED  distinct producers tracked
//!   GOAT_AUDIT_PRODUCER_QUOTA_NAMESPACE_FACTOR  aggregate budget per caller-chosen
//!                                         namespace, in per-bucket budgets (default 4)

use prometheus::{IntCounterVec, IntGaugeVec, Opts, Registry};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use tracing::{error, warn};

/// Measured whole-fabric persisted rate on the busiest production goatnet (
/// 2026-08-20, capacity-and-austerity.md §2): 46.4 msg/s across ~22 active
/// producers, ~730 bytes per audited event, ~2.7 GiB of audit per day.
#[allow(dead_code)] // the measurement the defaults are set from; checked by the margin test
pub const MEASURED_FABRIC_MSGS_PER_SEC: f64 = 46.4;
/// Measured audit bytes per second for the whole fabric (2.7 GiB/day).
#[allow(dead_code)] // the measurement the defaults are set from; checked by the margin test
pub const MEASURED_FABRIC_AUDIT_BYTES_PER_SEC: f64 = 2.7 * 1024.0 * 1024.0 * 1024.0 / 86_400.0;

/// 30,000 records per minute per producer = 500 records/s: 10.8x the whole
/// measured fabric rate, ~240x the measured per-producer mean (2.1 msg/s).
pub const DEFAULT_RECORDS_PER_WINDOW: u64 = 30_000;
/// 128 MiB of charged bytes per minute per producer (~2.2 MB/s): ~69x the
/// whole fabric's measured audit bytes. Sized so the record limit, not the
/// bytes limit, binds first for a typical sample (30-byte key, 730-byte
/// payload, charged ~3.2 KB because the charge is a worst-case bound).
pub const DEFAULT_BYTES_PER_WINDOW: u64 = 128 * 1024 * 1024;
/// One-minute averaging window, the writer's batch interval.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(60);
/// At most 4 s of budget at once: 2,000 records at the default rate.
pub const DEFAULT_BURST: Duration = Duration::from_secs(4);
/// Producers tracked. Past this, new producers share one bucket (`~other`).
pub const DEFAULT_MAX_TRACKED: usize = 4096;
/// Buckets kept under one caller-chosen namespace (`from`, `release`,
/// `hello`, `multipub`) before the rest charge `<namespace>/~other`. The
/// default slot pool is 256, so legitimate use stays below it.
pub const MAX_BUCKETS_PER_NAMESPACE: usize = 256;

/// The shared bucket for producers past `max_tracked`.
pub const OVERFLOW_ANCHOR: &str = "~other";

/// Aggregate budget of a caller-chosen namespace, in per-bucket budgets: 4 x
/// 500 records/s = 2,000 records/s, 43x the whole measured fabric (46.4/s).
pub const DEFAULT_NAMESPACE_FACTOR: u64 = 4;

/// Allowance for every field of the stored record(s) other than the key and
/// the payload: field names, hash, timestamps, ids, event id and attribution
/// of the v2 audit record AND its replay copy, with every configured id
/// (receiving site, subscriber id) up to 32 characters. Measured: ~1,840 B
/// for both records at those maxima with a 1-byte key (the portable test
/// below); ~780 B at the reviewer's realistic sizes.
pub const ENVELOPE_BYTES: u64 = 2048;

const ENV_ENFORCE: &str = "GOAT_AUDIT_PRODUCER_QUOTA_ENFORCE";
const ENV_RECORDS: &str = "GOAT_AUDIT_PRODUCER_QUOTA_RECORDS";
const ENV_BYTES: &str = "GOAT_AUDIT_PRODUCER_QUOTA_BYTES";
const ENV_WINDOW: &str = "GOAT_AUDIT_PRODUCER_QUOTA_WINDOW_SECS";
const ENV_BURST: &str = "GOAT_AUDIT_PRODUCER_QUOTA_BURST_SECS";
const ENV_MAX_TRACKED: &str = "GOAT_AUDIT_PRODUCER_QUOTA_MAX_TRACKED";
const ENV_NAMESPACE_FACTOR: &str = "GOAT_AUDIT_PRODUCER_QUOTA_NAMESPACE_FACTOR";

/// `ops/...` prefixes with their own writer in the router ACL (ingress put
/// rules in goat-zenoh config.json5.j2). Longest match wins; anything else
/// under `ops` is the operator's or a federated router's and charges `ops`.
const OPS_WRITER_PREFIXES: &[&str] = &[
    "ops/consistency/health",
    "ops/consistency/report",
    "ops/fabric/cardinality",
    "ops/fabric/census",
    "ops/fabric/topology",
    "ops/presence/health",
    "ops/names/claim",
    "ops/mesh/health",
    "ops/time/attest",
    "ops/time/health",
    "ops/net/health",
    "ops/fabric",
];

/// First segments split at the next segment, which the ACL binds per writer
/// (or per release set). Bucket creation under each is bounded.
const TWO_SEGMENT_NAMESPACES: &[&str] = &["from", "release", "hello"];
/// `multipub/<pool>/<topic>`: one bucket per multi-publisher topic.
const MULTIPUB: &str = "multipub";

/// Where a sample is metered.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Bucket<'a> {
    /// The bucket name, a prefix of the key.
    pub name: &'a str,
    /// The namespace whose bucket count is bounded, when the bucket is
    /// chosen by a segment past the first.
    pub namespace: Option<&'static str>,
    /// Whether enforcement may drop this bucket's samples (`ops/audit/**`
    /// is never dropped).
    pub enforceable: bool,
}

impl<'a> Bucket<'a> {
    fn of(name: &'a str) -> Self {
        Self {
            name,
            namespace: None,
            enforceable: true,
        }
    }
}

/// The first `n` `/`-segments of `key` (all of it when it has fewer).
fn first_segments(key: &str, n: usize) -> &str {
    match key.match_indices('/').nth(n.saturating_sub(1)) {
        Some((i, _)) if n > 0 => &key[..i],
        _ => key,
    }
}

/// True when `key` is `prefix` or lies under it.
fn under(key: &str, prefix: &str) -> bool {
    key.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// The bucket a sample on `key` is metered to; see the module comment.
pub fn bucket_of_key(key: &str) -> Bucket<'_> {
    let first = first_segments(key, 1);
    match first {
        "ops" => {
            if under(key, "ops/audit") {
                let name = ["ops/audit/operator", "ops/audit/surface"]
                    .into_iter()
                    .find(|p| under(key, p))
                    .map_or("ops/audit", |p| &key[..p.len()]);
                return Bucket {
                    name,
                    namespace: None,
                    enforceable: false,
                };
            }
            let p = OPS_WRITER_PREFIXES
                .iter()
                .filter(|p| under(key, p))
                .max_by_key(|p| p.len());
            Bucket::of(p.map_or("ops", |p| &key[..p.len()]))
        }
        "attest" if under(key, "attest/schema") => Bucket::of(&key[.."attest/schema".len()]),
        MULTIPUB => Bucket {
            name: first_segments(key, 3),
            namespace: Some(MULTIPUB),
            enforceable: true,
        },
        f => match TWO_SEGMENT_NAMESPACES.iter().find(|n| **n == f) {
            Some(ns) => Bucket {
                name: first_segments(key, 2),
                namespace: Some(ns),
                enforceable: true,
            },
            None => Bucket::of(f),
        },
    }
}

/// The length of `s` once JSON-encoded, without the quotes: what a key costs
/// in a stored record. `"` and `\` and the short control escapes take 2
/// bytes, every other control character 6 (`\u00XX`); everything else is
/// written as is (review r3 F7).
pub fn json_escaped_len(s: &str) -> usize {
    s.bytes()
        .map(|b| match b {
            b'"' | b'\\' | b'\n' | b'\r' | b'\t' | 0x08 | 0x0c => 2,
            0x00..=0x1f => 6,
            _ => 1,
        })
        .sum()
}

/// The bytes charged for one sample: an upper bound of what is stored for it
/// under the v1 record and the v2 audit + replay records (see the module
/// comment). `key_len` is the key's JSON-escaped length (`json_escaped_len`).
pub fn charge_bytes(key_len: usize, payload_len: usize, inline_cap: usize) -> u64 {
    let inline = payload_len.min(inline_cap) as u64;
    6 * key_len as u64 + inline.div_ceil(3) * 4 + ENVELOPE_BYTES
}

/// The per-producer limits. `records == 0 && bytes == 0` disables metering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    pub enforce: bool,
    pub records: u64,
    pub bytes: u64,
    pub window: Duration,
    pub burst: Duration,
    pub max_tracked: usize,
    /// Aggregate budget of each caller-chosen namespace, in per-bucket budgets.
    pub namespace_factor: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            enforce: false,
            records: DEFAULT_RECORDS_PER_WINDOW,
            bytes: DEFAULT_BYTES_PER_WINDOW,
            window: DEFAULT_WINDOW,
            burst: DEFAULT_BURST,
            max_tracked: DEFAULT_MAX_TRACKED,
            namespace_factor: DEFAULT_NAMESPACE_FACTOR,
        }
    }
}

impl Limits {
    /// Limits from the process environment; see the module comment.
    pub fn from_env() -> Self {
        let (limits, problems) = Self::from_lookup(|k| std::env::var(k).ok());
        for p in &problems {
            error!(
                problem = p.as_str(),
                "audit producer quota: bad setting; using the default"
            );
        }
        limits
    }

    /// Limits from a key lookup, plus a description of every value that
    /// could not be used (each replaced by its default).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> (Self, Vec<String>) {
        let mut out = Self::default();
        let mut problems = Vec::new();
        let value = |key: &str| {
            get(key)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        out.enforce = match value(ENV_ENFORCE).map(|v| v.to_ascii_lowercase()) {
            None => false,
            Some(v) if ["true", "1", "yes"].contains(&v.as_str()) => true,
            Some(v) if ["false", "0", "no"].contains(&v.as_str()) => false,
            Some(v) => {
                problems.push(format!(
                    "{ENV_ENFORCE}={v:?} is not true or false; observe-only"
                ));
                false
            }
        };
        let mut num = |key: &str, default: u64, min: u64| -> u64 {
            match value(key) {
                None => default,
                Some(v) => match v.parse::<u64>() {
                    Ok(n) if n >= min => n,
                    _ => {
                        problems.push(format!("{key}={v:?} is not a whole number >= {min}"));
                        default
                    }
                },
            }
        };
        out.records = num(ENV_RECORDS, DEFAULT_RECORDS_PER_WINDOW, 0);
        out.bytes = num(ENV_BYTES, DEFAULT_BYTES_PER_WINDOW, 0);
        out.window = Duration::from_secs(num(ENV_WINDOW, DEFAULT_WINDOW.as_secs(), 1));
        out.burst = Duration::from_secs(num(ENV_BURST, DEFAULT_BURST.as_secs(), 1));
        out.max_tracked = usize::try_from(num(ENV_MAX_TRACKED, DEFAULT_MAX_TRACKED as u64, 1))
            .unwrap_or(DEFAULT_MAX_TRACKED);
        out.namespace_factor = num(ENV_NAMESPACE_FACTOR, DEFAULT_NAMESPACE_FACTOR, 1);
        if out.burst > out.window {
            problems.push(format!(
                "{ENV_BURST}={} is longer than the window; using the window",
                out.burst.as_secs()
            ));
            out.burst = out.window;
        }
        (out, problems)
    }

    /// Whether any limit is in force.
    pub fn enabled(&self) -> bool {
        self.records > 0 || self.bytes > 0
    }

    fn rate(limit: u64, window: Duration) -> f64 {
        limit as f64 / window.as_secs_f64()
    }

    /// Bucket capacity: `burst` seconds of budget, at least one record and at
    /// least one maximal record's bytes (or the whole limit, if smaller).
    #[cfg(test)]
    fn capacities(&self) -> (f64, f64) {
        self.scaled_capacities(1.0)
    }

    /// Capacity of a budget `scale` times the per-bucket one.
    fn scaled_capacities(&self, scale: f64) -> (f64, f64) {
        let burst = self.burst.as_secs_f64();
        let rec = (Self::rate(self.records, self.window) * burst * scale).max(1.0);
        let floor = (256.0 * 1024.0_f64).min(self.bytes as f64 * scale);
        let bytes = (Self::rate(self.bytes, self.window) * burst * scale).max(floor);
        (rec, bytes)
    }
}

#[derive(Debug)]
struct Usage {
    /// This entry's budget in per-bucket budgets (namespace aggregates > 1).
    scale: f64,
    record_tokens: f64,
    byte_tokens: f64,
    last: Instant,
    admitted_records: u64,
    over_records: u64,
    dropped_records: u64,
    dropped_bytes: u64,
}

#[derive(Clone)]
struct QuotaMetrics {
    over_rate_total: IntCounterVec,
    dropped_total: IntCounterVec,
    dropped_bytes_total: IntCounterVec,
    queue_dropped_total: IntCounterVec,
}

impl QuotaMetrics {
    fn register(
        registry: &Registry,
        audit_subscriber_id: &str,
        limits: &Limits,
    ) -> prometheus::Result<Self> {
        let const_labels: HashMap<String, String> = [(
            "audit_subscriber_id".to_string(),
            audit_subscriber_id.to_string(),
        )]
        .into();
        let vec = |name: &str, help: &str, labels: &[&str]| {
            IntCounterVec::new(
                Opts::new(name, help).const_labels(const_labels.clone()),
                labels,
            )
        };
        let over_rate_total = vec(
            "goat_audit_producer_over_rate_total",
            "Samples beyond their producer's audit budget, by producer bucket and the limit hit. Kept unless enforcing.",
            &["vendor", "limit"],
        )?;
        let dropped_total = vec(
            "goat_audit_quota_dropped_total",
            "Samples NOT persisted because enforcement refused them, by producer bucket and the limit hit.",
            &["vendor", "limit"],
        )?;
        let dropped_bytes_total = vec(
            "goat_audit_quota_dropped_bytes_total",
            "Charged bytes of samples NOT persisted because enforcement refused them, by producer bucket.",
            &["vendor"],
        )?;
        let queue_dropped_total = vec(
            "goat_audit_queue_dropped_total",
            "Samples NOT persisted because the subscriber's writer queue was full, by producer bucket. Whole-batch drops at an upload queue are in goat_audit_drop_total only.",
            &["vendor"],
        )?;
        let limit = IntGaugeVec::new(
            Opts::new(
                "goat_audit_quota_limit",
                "The per-producer audit budget in force (0 = that limit is off; enforce 1 = over-budget samples are dropped).",
            )
            .const_labels(const_labels),
            &["limit"],
        )?;
        registry.register(Box::new(over_rate_total.clone()))?;
        registry.register(Box::new(dropped_total.clone()))?;
        registry.register(Box::new(dropped_bytes_total.clone()))?;
        registry.register(Box::new(queue_dropped_total.clone()))?;
        registry.register(Box::new(limit.clone()))?;
        let gauge = |v: u64| i64::try_from(v).unwrap_or(i64::MAX);
        for (which, v) in [
            ("records", gauge(limits.records)),
            ("bytes", gauge(limits.bytes)),
            ("window_seconds", gauge(limits.window.as_secs())),
            ("burst_seconds", gauge(limits.burst.as_secs())),
            ("enforce", i64::from(limits.enforce)),
        ] {
            limit.with_label_values(&[which]).set(v);
        }
        Ok(Self {
            over_rate_total,
            dropped_total,
            dropped_bytes_total,
            queue_dropped_total,
        })
    }

    /// Create a producer's series at 0, so its first excess is an increase.
    fn touch(&self, bucket: &str) {
        for which in ["records", "bytes"] {
            let _ = self.over_rate_total.with_label_values(&[bucket, which]);
            let _ = self.dropped_total.with_label_values(&[bucket, which]);
        }
        let _ = self.dropped_bytes_total.with_label_values(&[bucket]);
        let _ = self.queue_dropped_total.with_label_values(&[bucket]);
    }
}

/// Per-producer token-bucket meter. One instance lives in the subscriber loop.
pub struct ProducerQuota {
    limits: Limits,
    window_start: Instant,
    usage: HashMap<String, Usage>,
    seen: HashSet<String>,
    namespace_buckets: HashMap<&'static str, HashSet<String>>,
    metrics: Option<QuotaMetrics>,
}

impl ProducerQuota {
    /// A meter with `limits`, registering its metrics on `registry` when one
    /// is given. It never fails: a subscriber that stopped over a metrics
    /// problem would lose ALL audit, so a registration failure is logged at
    /// ERROR and the meter carries on with its log lines only.
    pub fn new(limits: Limits, registry: Option<&Registry>, audit_subscriber_id: &str) -> Self {
        let metrics =
            registry.and_then(
                |r| match QuotaMetrics::register(r, audit_subscriber_id, &limits) {
                    Ok(m) => Some(m),
                    Err(e) => {
                        error!(error = %e, "audit producer quota: metrics NOT registered; excess is only logged");
                        None
                    }
                },
            );
        if !limits.enabled() {
            warn!(
                "audit producer quota DISABLED (both limits 0): producer audit rate is not metered"
            );
        } else {
            tracing::info!(
                enforce = limits.enforce,
                records = limits.records,
                bytes = limits.bytes,
                window_s = limits.window.as_secs(),
                burst_s = limits.burst.as_secs(),
                "audit producer quota {}",
                if limits.enforce {
                    "ENFORCING: over-budget samples are dropped"
                } else {
                    "observe-only: over-budget samples are counted and kept"
                }
            );
        }
        Self {
            limits,
            window_start: Instant::now(),
            usage: HashMap::new(),
            seen: HashSet::new(),
            namespace_buckets: HashMap::new(),
            metrics,
        }
    }

    /// The tracked bucket name for `b`: bounded per namespace, then globally.
    fn resolve(&mut self, b: &Bucket<'_>) -> String {
        let mut name = b.name.to_string();
        if let Some(ns) = b.namespace {
            let set = self.namespace_buckets.entry(ns).or_default();
            if !set.contains(&name) {
                if set.len() < MAX_BUCKETS_PER_NAMESPACE {
                    set.insert(name.clone());
                } else {
                    name = format!("{ns}/{OVERFLOW_ANCHOR}");
                }
            }
        }
        let tracked = self.usage.len() - self.usage.keys().filter(|k| k.ends_with("/*")).count();
        if self.usage.contains_key(&name) || tracked < self.limits.max_tracked {
            name
        } else {
            OVERFLOW_ANCHOR.to_string()
        }
    }

    /// Whether a sample on `key` with a `payload_len`-byte payload may be
    /// persisted (`inline_cap` is the subscriber's inline payload limit). It
    /// is metered either way; `false` only when enforcing and the sample is
    /// over its producer's budget, in which case it has been counted and the
    /// caller must not persist it.
    pub fn admit(&mut self, key: &str, payload_len: usize, inline_cap: usize) -> bool {
        self.admit_at(key, payload_len, inline_cap, Instant::now())
    }

    /// `admit` at an explicit time (tests).
    pub fn admit_at(
        &mut self,
        key: &str,
        payload_len: usize,
        inline_cap: usize,
        now: Instant,
    ) -> bool {
        if !self.limits.enabled() {
            return true;
        }
        let b = bucket_of_key(key);
        if now.saturating_duration_since(self.window_start) >= self.limits.window {
            self.close_window(now);
        }
        let bucket = self.resolve(&b);
        if !self.seen.contains(&bucket) && self.seen.len() <= self.limits.max_tracked {
            if let Some(m) = &self.metrics {
                m.touch(&bucket);
            }
            self.seen.insert(bucket.clone());
        }
        let charge = charge_bytes(json_escaped_len(key), payload_len, inline_cap);
        let enforce = self.limits.enforce && b.enforceable;
        // A caller-chosen namespace also has an aggregate budget, so a writer
        // spread over many of its buckets is still metered (review r3 F1).
        let aggregate = b.namespace.map(|ns| format!("{ns}/*"));
        if let Some(a) = &aggregate {
            if !self.seen.contains(a) {
                if let Some(m) = &self.metrics {
                    m.touch(a);
                }
                self.seen.insert(a.clone());
            }
        }
        let factor = self.limits.namespace_factor as f64;
        let over_bucket = self.refill_and_check(&bucket, 1.0, charge, now);
        let over_aggregate = aggregate
            .as_deref()
            .and_then(|a| self.refill_and_check(a, factor, charge, now));
        if over_bucket.is_none() && over_aggregate.is_none() {
            self.consume(&bucket, charge, true);
            if let Some(a) = &aggregate {
                self.consume(a, charge, false);
            }
            return true;
        }
        for (name, which) in [
            (Some(bucket.as_str()), over_bucket),
            (aggregate.as_deref(), over_aggregate),
        ] {
            if let (Some(name), Some(which)) = (name, which) {
                self.note_over(name, which, enforce);
            }
        }
        if !enforce {
            if let Some(u) = self.usage.get_mut(&bucket) {
                u.admitted_records += 1;
            }
            return true;
        }
        // Dropped: charged to the budget that refused it.
        let (name, which) = match (over_bucket, over_aggregate) {
            (Some(w), _) => (bucket.clone(), w),
            (None, Some(w)) => (aggregate.clone().unwrap_or_default(), w),
            (None, None) => unreachable!("handled above"),
        };
        if let Some(u) = self.usage.get_mut(&name) {
            u.dropped_records += 1;
            u.dropped_bytes = u.dropped_bytes.saturating_add(charge);
        }
        if let Some(m) = &self.metrics {
            m.dropped_total.with_label_values(&[&name, which]).inc();
            m.dropped_bytes_total
                .with_label_values(&[&name])
                .inc_by(charge);
        }
        false
    }

    /// Refill `name`'s budget (created full, at `scale` per-bucket budgets) up
    /// to `now`, and say which limit a `charge`-byte sample would exceed.
    fn refill_and_check(
        &mut self,
        name: &str,
        scale: f64,
        charge: u64,
        now: Instant,
    ) -> Option<&'static str> {
        let (cap_records, cap_bytes) = self.limits.scaled_capacities(scale);
        let rate_records = Limits::rate(self.limits.records, self.limits.window) * scale;
        let rate_bytes = Limits::rate(self.limits.bytes, self.limits.window) * scale;
        let u = self.usage.entry(name.to_string()).or_insert(Usage {
            scale,
            record_tokens: cap_records,
            byte_tokens: cap_bytes,
            last: now,
            admitted_records: 0,
            over_records: 0,
            dropped_records: 0,
            dropped_bytes: 0,
        });
        let elapsed = now.saturating_duration_since(u.last).as_secs_f64();
        u.last = now;
        u.record_tokens = (u.record_tokens + elapsed * rate_records).min(cap_records);
        u.byte_tokens = (u.byte_tokens + elapsed * rate_bytes).min(cap_bytes);
        if self.limits.records > 0 && u.record_tokens < 1.0 {
            Some("records")
        } else if self.limits.bytes > 0 && u.byte_tokens < charge as f64 {
            Some("bytes")
        } else {
            None
        }
    }

    /// Spend one sample of `name`'s budget.
    fn consume(&mut self, name: &str, charge: u64, count_admitted: bool) {
        let (records, bytes) = (self.limits.records, self.limits.bytes);
        if let Some(u) = self.usage.get_mut(name) {
            if records > 0 {
                u.record_tokens -= 1.0;
            }
            if bytes > 0 {
                u.byte_tokens -= charge as f64;
            }
            if count_admitted {
                u.admitted_records += 1;
            }
        }
    }

    /// Count one sample over `name`'s budget, logging the first in a window.
    fn note_over(&mut self, name: &str, which: &'static str, enforce: bool) {
        let window_s = self.limits.window.as_secs();
        if let Some(u) = self.usage.get_mut(name) {
            if u.over_records == 0 {
                warn!(
                    vendor = name,
                    limit = which,
                    admitted_records = u.admitted_records,
                    enforced = enforce,
                    window_s,
                    "audit producer over its rate budget: further samples are {} until its budget refills (counted in goat_audit_producer_over_rate_total)",
                    if enforce { "NOT persisted" } else { "kept" }
                );
            }
            u.over_records += 1;
        }
        if let Some(m) = &self.metrics {
            m.over_rate_total.with_label_values(&[name, which]).inc();
        }
    }

    /// Count a sample on `key` that the writer queue had no room for.
    pub fn note_queue_drop(&self, key: &str) {
        let Some(m) = &self.metrics else { return };
        let b = bucket_of_key(key);
        let mut name = b.name.to_string();
        if let Some(ns) = b.namespace {
            if !self
                .namespace_buckets
                .get(ns)
                .is_some_and(|set| set.contains(&name))
            {
                name = format!("{ns}/{OVERFLOW_ANCHOR}");
            }
        }
        if !self.seen.contains(&name) {
            name = OVERFLOW_ANCHOR.to_string();
        }
        m.queue_dropped_total.with_label_values(&[&name]).inc();
    }

    /// Close the current logging window: log every producer that went over
    /// budget in it, forget producers that are back at full budget, and start
    /// a fresh window at `now`.
    fn close_window(&mut self, now: Instant) {
        let limits = self.limits.clone();
        let rate_records = Limits::rate(limits.records, limits.window);
        let rate_bytes = Limits::rate(limits.bytes, limits.window);
        for (bucket, u) in &self.usage {
            if u.dropped_records > 0 {
                error!(
                    vendor = bucket.as_str(),
                    over_records = u.over_records,
                    dropped_records = u.dropped_records,
                    dropped_bytes = u.dropped_bytes,
                    admitted_records = u.admitted_records,
                    window_s = self.limits.window.as_secs(),
                    "audit producer window closed with records NOT persisted (enforcing)"
                );
            } else if u.over_records > 0 {
                warn!(
                    vendor = bucket.as_str(),
                    over_records = u.over_records,
                    admitted_records = u.admitted_records,
                    window_s = self.limits.window.as_secs(),
                    "audit producer window closed over its rate budget (observe-only: all kept)"
                );
            }
        }
        self.usage.retain(|_, u| {
            let (cap_records, cap_bytes) = limits.scaled_capacities(u.scale);
            let elapsed = now.saturating_duration_since(u.last).as_secs_f64();
            let full = u.record_tokens + elapsed * rate_records * u.scale >= cap_records
                && u.byte_tokens + elapsed * rate_bytes * u.scale >= cap_bytes;
            u.admitted_records = 0;
            u.over_records = 0;
            u.dropped_records = 0;
            u.dropped_bytes = 0;
            !full
        });
        self.window_start = now;
    }

    /// Records over budget for `bucket` in the current window (tests).
    #[cfg(test)]
    fn over(&self, bucket: &str) -> u64 {
        self.usage.get(bucket).map_or(0, |u| u.over_records)
    }

    /// Records dropped for `bucket` in the current window (tests).
    #[cfg(test)]
    fn dropped(&self, bucket: &str) -> u64 {
        self.usage.get(bucket).map_or(0, |u| u.dropped_records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAP: usize = 16 * 1024;

    fn limits(records: u64, bytes: u64) -> Limits {
        Limits {
            enforce: true,
            records,
            bytes,
            window: Duration::from_secs(60),
            burst: Duration::from_secs(60),
            max_tracked: 3,
            namespace_factor: DEFAULT_NAMESPACE_FACTOR,
        }
    }

    fn enforcing() -> Limits {
        Limits {
            enforce: true,
            ..Limits::default()
        }
    }

    fn counter(registry: &Registry, name: &str, labels: &[(&str, &str)]) -> Option<u64> {
        let fams = registry.gather();
        let ms: Vec<_> = fams
            .iter()
            .filter(|f| f.get_name() == name)
            .flat_map(|f| f.get_metric().iter())
            .filter(|m| {
                labels.iter().all(|(k, v)| {
                    m.get_label()
                        .iter()
                        .any(|l| l.get_name() == *k && l.get_value() == *v)
                })
            })
            .collect();
        if ms.is_empty() {
            return None;
        }
        Some(ms.iter().map(|m| m.get_counter().get_value() as u64).sum())
    }

    #[test]
    fn defaults_are_observe_only_and_far_above_the_measured_fabric() {
        let l = Limits::default();
        assert!(!l.enforce, "enforcement is opt-in");
        let per_sec = l.records as f64 / l.window.as_secs_f64();
        assert!(
            per_sec >= 10.0 * MEASURED_FABRIC_MSGS_PER_SEC,
            "records {per_sec}/s"
        );
        let bytes_per_sec = l.bytes as f64 / l.window.as_secs_f64();
        assert!(
            bytes_per_sec >= 10.0 * MEASURED_FABRIC_AUDIT_BYTES_PER_SEC,
            "bytes {bytes_per_sec}/s"
        );
        let (burst_records, _) = l.capacities();
        assert!(burst_records <= 4096.0 / 2.0, "burst {burst_records}");
        assert!(l.records * 730 < l.bytes);
        // The record limit binds before the bytes limit for a typical sample.
        assert!(l.records * charge_bytes(30, 730, CAP) < l.bytes);
        assert!(l.enabled());
    }

    // Revision 3: by default nothing is ever dropped; the excess is counted.
    #[test]
    fn observe_only_keeps_every_sample_and_counts_the_excess() {
        let registry = Registry::new();
        let mut q = ProducerQuota::new(Limits::default(), Some(&registry), "lab");
        let t0 = Instant::now();
        assert!((0..10_000).all(|_| q.admit_at("vendor-a/x", 100, CAP, t0)));
        assert_eq!(
            q.over("vendor-a"),
            8_000,
            "2,000 burst, the rest over budget"
        );
        assert_eq!(q.dropped("vendor-a"), 0);
        assert_eq!(
            counter(
                &registry,
                "goat_audit_producer_over_rate_total",
                &[("vendor", "vendor-a"), ("limit", "records")]
            ),
            Some(8_000)
        );
        assert_eq!(
            counter(
                &registry,
                "goat_audit_quota_dropped_total",
                &[("vendor", "vendor-a")]
            ),
            Some(0)
        );
    }

    #[test]
    fn buckets_follow_the_acl_bound_writer() {
        for (key, want, enforceable) in [
            ("vendor-a/sensorgrid/track/v1", "vendor-a", true),
            (
                "61dc1e84ca2c7cfea827728a0f4bfa4d/data/x",
                "61dc1e84ca2c7cfea827728a0f4bfa4d",
                true,
            ),
            ("solo", "solo", true),
            ("ops/audit/operator/alice", "ops/audit/operator", false),
            ("ops/audit/surface/console/x", "ops/audit/surface", false),
            ("ops/audit/forged/x", "ops/audit", false),
            ("ops/audit", "ops/audit", false),
            ("ops/names/claim/attacker-host", "ops/names/claim", true),
            ("ops/time/attest/core", "ops/time/attest", true),
            ("ops/fabric/topology/edges", "ops/fabric/topology", true),
            ("ops/fabric/health/x", "ops/fabric", true),
            ("ops/somethingelse/x", "ops", true),
            ("ops/auditx/y", "ops", true),
            ("attest/schema/vendor-a/x", "attest/schema", true),
            ("attest/other", "attest", true),
            ("from/sovA/x/y", "from/sovA", true),
            ("release/set-1/x", "release/set-1", true),
            ("hello/assay-probe", "hello/assay-probe", true),
            ("multipub/poolA/topicA/x/y", "multipub/poolA/topicA", true),
            ("multipub/poolB/topicB", "multipub/poolB/topicB", true),
            ("keys/x", "keys", true),
        ] {
            let b = bucket_of_key(key);
            assert_eq!((b.name, b.enforceable), (want, enforceable), "{key}");
        }
    }

    // Review r2 F1: ops/audit/** is never dropped, but it is metered and
    // alerted, so a flood there is visible.
    #[test]
    fn ops_audit_is_never_dropped_but_its_excess_is_counted() {
        let registry = Registry::new();
        let mut q = ProducerQuota::new(enforcing(), Some(&registry), "lab");
        let t0 = Instant::now();
        assert!((0..10_000).all(|_| q.admit_at("ops/audit/surface/console/x", 16 * 1024, CAP, t0)));
        assert!(q.over("ops/audit/surface") > 0);
        assert_eq!(q.dropped("ops/audit/surface"), 0);
        assert!(
            counter(
                &registry,
                "goat_audit_producer_over_rate_total",
                &[("vendor", "ops/audit/surface")]
            )
            .unwrap()
                > 0
        );
        assert!(q.admit_at("ops/audit/operator/alice", 400, CAP, t0));
    }

    // Review r1 F1, under enforcement: a name-claim flood never costs the
    // operator trail, time attestations or any other producer a record.
    #[test]
    fn a_name_claim_flood_never_touches_operator_or_infra_audit() {
        let l = enforcing();
        let mut q = ProducerQuota::new(l.clone(), None, "lab");
        let t0 = Instant::now();
        let admitted = (0..l.records)
            .filter(|i| q.admit_at(&format!("ops/names/claim/h{i}"), 64, CAP, t0))
            .count() as u64;
        assert!(admitted < l.records, "the claim flood is itself bounded");
        for key in [
            "ops/audit/operator/alice",
            "ops/audit/surface/console",
            "ops/time/attest/core",
            "vendor-a/track/v1",
            "from/sovA/x",
            "multipub/poolB/topicB/y",
        ] {
            for _ in 0..100 {
                assert!(
                    q.admit_at(key, 400, CAP, t0),
                    "{key} refused after a claim flood"
                );
            }
        }
    }

    // Review r2 F2: each multi-publisher topic is its own bucket.
    #[test]
    fn multipub_topics_are_metered_separately() {
        let mut q = ProducerQuota::new(enforcing(), None, "lab");
        let t0 = Instant::now();
        for i in 0..10_000u64 {
            q.admit_at(
                "multipub/poolA/topicA/x",
                64,
                CAP,
                t0 + Duration::from_micros(i),
            );
        }
        let ok = (0..60)
            .filter(|s| {
                q.admit_at(
                    "multipub/poolB/topicB/y",
                    64,
                    CAP,
                    t0 + Duration::from_secs(*s),
                )
            })
            .count();
        assert_eq!(ok, 60);
    }

    // Review r2 F3: a vendor named like a split namespace cannot grow the
    // table: at most MAX_BUCKETS_PER_NAMESPACE buckets, then one overflow, and
    // a producer first seen afterwards still gets its own bucket.
    #[test]
    fn caller_chosen_namespace_buckets_are_bounded() {
        let registry = Registry::new();
        let mut l = enforcing();
        l.max_tracked = 1024;
        let mut q = ProducerQuota::new(l, Some(&registry), "lab");
        let t0 = Instant::now();
        let admitted = (0..5_000)
            .flat_map(|i| (0..3).map(move |_| i))
            .filter(|i| q.admit_at(&format!("release/r{i}/x"), 64, CAP, t0))
            .count();
        let release_buckets = q
            .usage
            .keys()
            .filter(|k| k.starts_with("release/") && !k.ends_with("/*"))
            .count();
        assert_eq!(release_buckets, MAX_BUCKETS_PER_NAMESPACE + 1);
        assert!(q.usage.contains_key("release/~other"));
        assert!(admitted <= (MAX_BUCKETS_PER_NAMESPACE + 1) * 2_000);
        assert!(q.admit_at("vendor-new/track", 64, CAP, t0));
        assert!(q.usage.contains_key("vendor-new"));
        assert_eq!(
            counter(
                &registry,
                "goat_audit_quota_dropped_total",
                &[("vendor", "vendor-new"), ("limit", "records")]
            ),
            Some(0),
            "a later producer's series is still created at 0"
        );
    }

    #[test]
    fn enforcing_refuses_and_counts_over_budget_samples() {
        let registry = Registry::new();
        let mut q = ProducerQuota::new(limits(3, 0), Some(&registry), "lab");
        let t0 = Instant::now();
        let admitted: Vec<bool> = (0..5)
            .map(|_| q.admit_at("vendor-a/x", 100, CAP, t0))
            .collect();
        assert_eq!(admitted, vec![true, true, true, false, false]);
        assert!(q.admit_at("vendor-b/x", 100, CAP, t0));
        assert_eq!(q.dropped("vendor-a"), 2);
        let charge = charge_bytes("vendor-a/x".len(), 100, CAP);
        for name in [
            "goat_audit_quota_dropped_total",
            "goat_audit_producer_over_rate_total",
        ] {
            assert_eq!(
                counter(
                    &registry,
                    name,
                    &[("vendor", "vendor-a"), ("limit", "records")]
                ),
                Some(2),
                "{name}"
            );
        }
        assert_eq!(
            counter(
                &registry,
                "goat_audit_quota_dropped_bytes_total",
                &[("vendor", "vendor-a")]
            ),
            Some(2 * charge)
        );
    }

    #[test]
    fn a_producer_series_exists_at_zero_before_any_excess() {
        let registry = Registry::new();
        let mut q = ProducerQuota::new(limits(100, 0), Some(&registry), "lab");
        assert!(q.admit_at("vendor-b/x", 10, CAP, Instant::now()));
        for (name, labels) in [
            (
                "goat_audit_producer_over_rate_total",
                vec![("vendor", "vendor-b"), ("limit", "records")],
            ),
            (
                "goat_audit_producer_over_rate_total",
                vec![("vendor", "vendor-b"), ("limit", "bytes")],
            ),
            (
                "goat_audit_quota_dropped_total",
                vec![("vendor", "vendor-b"), ("limit", "records")],
            ),
            (
                "goat_audit_quota_dropped_bytes_total",
                vec![("vendor", "vendor-b")],
            ),
            (
                "goat_audit_queue_dropped_total",
                vec![("vendor", "vendor-b")],
            ),
        ] {
            assert_eq!(
                counter(&registry, name, &labels),
                Some(0),
                "{name} {labels:?}"
            );
        }
    }

    #[test]
    fn bytes_charge_covers_key_base64_and_envelope() {
        assert_eq!(charge_bytes(10, 0, CAP), 60 + ENVELOPE_BYTES);
        assert_eq!(charge_bytes(10, 3, CAP), 60 + 4 + ENVELOPE_BYTES);
        assert_eq!(charge_bytes(10, 1 << 20, CAP), charge_bytes(10, CAP, CAP));
        let l = enforcing();
        let mut q = ProducerQuota::new(l.clone(), None, "lab");
        let key = format!("vendor-a/{}", "k".repeat(16 * 1024));
        let t0 = Instant::now();
        let mut charged = 0u64;
        for i in 0..l.records {
            if q.admit_at(&key, 0, CAP, t0 + Duration::from_millis(i * 2)) {
                charged += charge_bytes(key.len(), 0, CAP);
            }
        }
        let (_, burst_bytes) = l.capacities();
        assert!(charged as f64 <= l.bytes as f64 + burst_bytes, "{charged}");
    }

    #[test]
    fn bytes_limit_binds_on_charged_bytes() {
        let registry = Registry::new();
        let one = charge_bytes("vendor-a/x".len(), 600, CAP);
        let mut q = ProducerQuota::new(limits(0, one + one / 2), Some(&registry), "lab");
        let t0 = Instant::now();
        assert!(q.admit_at("vendor-a/x", 600, CAP, t0));
        assert!(!q.admit_at("vendor-a/x", 600, CAP, t0));
        assert_eq!(
            counter(
                &registry,
                "goat_audit_quota_dropped_total",
                &[("vendor", "vendor-a"), ("limit", "bytes")]
            ),
            Some(1)
        );
    }

    #[test]
    fn budget_refills_at_the_average_rate() {
        let mut l = limits(60, 0);
        l.burst = Duration::from_secs(10);
        let mut q = ProducerQuota::new(l, None, "lab");
        let t0 = Instant::now();
        let burst = (0..20)
            .filter(|_| q.admit_at("vendor-a/x", 1, CAP, t0))
            .count();
        assert_eq!(burst, 10);
        assert!(!q.admit_at("vendor-a/x", 1, CAP, t0 + Duration::from_millis(500)));
        assert!(q.admit_at("vendor-a/x", 1, CAP, t0 + Duration::from_secs(2)));
        let t1 = t0 + Duration::from_secs(61);
        let edge = (0..100)
            .filter(|_| q.admit_at("vendor-a/x", 1, CAP, t1))
            .count();
        assert!(edge <= 10, "{edge} admitted at the window edge");
    }

    #[test]
    fn producers_past_max_tracked_share_one_counted_bucket() {
        let registry = Registry::new();
        let mut q = ProducerQuota::new(limits(2, 0), Some(&registry), "lab");
        let t0 = Instant::now();
        for a in ["a", "b", "c"] {
            assert!(q.admit_at(a, 1, CAP, t0));
        }
        assert!(q.admit_at("d", 1, CAP, t0));
        assert!(q.admit_at("e", 1, CAP, t0));
        assert!(!q.admit_at("f", 1, CAP, t0));
        assert_eq!(q.usage.len(), 4);
        assert_eq!(
            counter(
                &registry,
                "goat_audit_quota_dropped_total",
                &[("vendor", OVERFLOW_ANCHOR), ("limit", "records")]
            ),
            Some(1)
        );
    }

    #[test]
    fn queue_drops_are_counted_by_producer() {
        let registry = Registry::new();
        let mut q = ProducerQuota::new(limits(100, 0), Some(&registry), "lab");
        let t0 = Instant::now();
        assert!(q.admit_at("vendor-a/x", 1, CAP, t0));
        assert!(q.admit_at("ops/audit/operator/alice", 1, CAP, t0));
        q.note_queue_drop("vendor-a/x");
        q.note_queue_drop("ops/audit/operator/alice");
        assert_eq!(
            counter(
                &registry,
                "goat_audit_queue_dropped_total",
                &[("vendor", "vendor-a")]
            ),
            Some(1)
        );
        assert_eq!(
            counter(
                &registry,
                "goat_audit_queue_dropped_total",
                &[("vendor", "ops/audit/operator")]
            ),
            Some(1)
        );
    }

    #[test]
    fn both_limits_zero_disables_metering() {
        let mut q = ProducerQuota::new(limits(0, 0), None, "lab");
        let t0 = Instant::now();
        assert!((0..100_000).all(|_| q.admit_at("vendor-a/x", 1_000_000, CAP, t0)));
    }

    #[test]
    fn settings_parse_and_a_bad_value_keeps_the_default_loudly() {
        let env: HashMap<&str, &str> = [
            (ENV_ENFORCE, "TRUE"),
            (ENV_RECORDS, "120000"),
            (ENV_BYTES, "0"),
            (ENV_WINDOW, "30"),
            (ENV_BURST, "2"),
            (ENV_MAX_TRACKED, "lots"),
        ]
        .into();
        let (l, problems) = Limits::from_lookup(|k| env.get(k).map(|v| v.to_string()));
        assert!(l.enforce);
        assert_eq!(l.records, 120_000);
        assert_eq!(l.bytes, 0);
        assert_eq!(l.window, Duration::from_secs(30));
        assert_eq!(l.burst, Duration::from_secs(2));
        assert_eq!(l.max_tracked, DEFAULT_MAX_TRACKED);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains(ENV_MAX_TRACKED));

        let (l, problems) =
            Limits::from_lookup(|k| (k == ENV_ENFORCE).then(|| "sometimes".to_string()));
        assert!(
            !l.enforce,
            "an unreadable enforce setting stays observe-only"
        );
        assert_eq!(problems.len(), 1);

        let (l, problems) = Limits::from_lookup(|k| (k == ENV_WINDOW).then(|| "0".to_string()));
        assert_eq!(l.window, DEFAULT_WINDOW);
        assert_eq!(problems.len(), 1);

        let (l, problems) = Limits::from_lookup(|k| (k == ENV_BURST).then(|| "600".to_string()));
        assert_eq!(l.burst, l.window);
        assert_eq!(problems.len(), 1);

        let (l, problems) = Limits::from_lookup(|_| None);
        assert_eq!(l, Limits::default());
        assert!(problems.is_empty());
    }

    #[test]
    fn limits_are_published_as_gauges() {
        let registry = Registry::new();
        let _q = ProducerQuota::new(Limits::default(), Some(&registry), "lab");
        let fams = registry.gather();
        let lim = fams
            .iter()
            .find(|f| f.get_name() == "goat_audit_quota_limit")
            .expect("limit gauge");
        let get = |which: &str| {
            lim.get_metric()
                .iter()
                .find(|m| {
                    m.get_label()
                        .iter()
                        .any(|l| l.get_name() == "limit" && l.get_value() == which)
                })
                .map(|m| m.get_gauge().get_value() as u64)
        };
        assert_eq!(get("records"), Some(DEFAULT_RECORDS_PER_WINDOW));
        assert_eq!(get("bytes"), Some(DEFAULT_BYTES_PER_WINDOW));
        assert_eq!(get("window_seconds"), Some(60));
        assert_eq!(get("burst_seconds"), Some(4));
        assert_eq!(get("enforce"), Some(0));
    }

    // Review r3 F1: a vendor named like a split namespace that spreads under
    // the per-bucket budget across many buckets still trips the namespace
    // aggregate, in observe mode (counted, kept) and under enforcement.
    #[test]
    fn a_namespace_spread_trips_the_aggregate() {
        for enforce in [false, true] {
            let registry = Registry::new();
            let l = Limits {
                enforce,
                ..Limits::default()
            };
            let mut q = ProducerQuota::new(l.clone(), Some(&registry), "lab");
            let t0 = Instant::now();
            let mut admitted = 0u64;
            // 64 buckets, each at 400/s (under its 500/s), for 2 s.
            for ms in 0..2_000u64 {
                for i in 0..64u64 {
                    if ms % 1000 < 400 {
                        let t = t0 + Duration::from_millis(ms);
                        if q.admit_at(&format!("release/r{i}/x"), 64, CAP, t) {
                            admitted += 1;
                        }
                    }
                }
            }
            let agg = counter(
                &registry,
                "goat_audit_producer_over_rate_total",
                &[("vendor", "release/*"), ("limit", "records")],
            );
            assert!(
                agg.unwrap() > 0,
                "enforce={enforce}: aggregate over-rate {agg:?}"
            );
            for i in [0u64, 30, 63] {
                assert_eq!(
                    counter(
                        &registry,
                        "goat_audit_producer_over_rate_total",
                        &[("vendor", &format!("release/r{i}")), ("limit", "records")]
                    ),
                    Some(0),
                    "no single bucket is over"
                );
            }
            let sent = 64 * 400 * 2;
            if enforce {
                let cap = (l.records * l.namespace_factor) as f64 / 60.0 * 2.0
                    + l.scaled_capacities(l.namespace_factor as f64).0;
                assert!(admitted as f64 <= cap + 1.0, "admitted {admitted} > {cap}");
            } else {
                assert_eq!(admitted, sent, "observe-only keeps everything");
            }
        }
    }

    #[test]
    fn the_namespace_aggregate_leaves_ordinary_use_alone() {
        let registry = Registry::new();
        let mut q = ProducerQuota::new(enforcing(), Some(&registry), "lab");
        let t0 = Instant::now();
        // 20 release sets at 50/s each (1,000/s total, half the aggregate).
        for ms in (0..60_000u64).step_by(20) {
            for i in 0..20 {
                assert!(q.admit_at(
                    &format!("release/s{i}/x"),
                    64,
                    CAP,
                    t0 + Duration::from_millis(ms)
                ));
            }
        }
        assert_eq!(
            counter(
                &registry,
                "goat_audit_producer_over_rate_total",
                &[("vendor", "release/*")]
            ),
            Some(0)
        );
    }

    // Review r3 F5: a writer-queue drop for an overflowed namespaced bucket is
    // labelled with the bucket it was metered to.
    #[test]
    fn queue_drop_labels_follow_the_resolved_bucket() {
        let registry = Registry::new();
        let mut l = enforcing();
        l.max_tracked = 1024;
        let mut q = ProducerQuota::new(l, Some(&registry), "lab");
        let t0 = Instant::now();
        for i in 0..(MAX_BUCKETS_PER_NAMESPACE + 5) {
            q.admit_at(&format!("release/r{i}/x"), 1, CAP, t0);
        }
        q.note_queue_drop("release/r9999/x");
        q.note_queue_drop("release/r3/x");
        assert_eq!(
            counter(
                &registry,
                "goat_audit_queue_dropped_total",
                &[("vendor", "release/~other")]
            ),
            Some(1)
        );
        assert_eq!(
            counter(
                &registry,
                "goat_audit_queue_dropped_total",
                &[("vendor", "release/r3")]
            ),
            Some(1)
        );
        assert_eq!(
            counter(
                &registry,
                "goat_audit_queue_dropped_total",
                &[("vendor", OVERFLOW_ANCHOR)]
            )
            .unwrap_or(0),
            0
        );
    }

    // Review r3 F4 (portable half; the stacked tree runs the same check on the
    // real records::record output): the charge is at least what a v2 audit
    // record plus its replay copy would store, for fields at realistic maximum
    // sizes.
    #[test]
    fn json_escaped_len_matches_serde_json() {
        for key in [
            "vendor-a/track/v1".to_string(),
            "a\"b\\c\nd\re\tf\u{8}g\u{c}h".to_string(),
            (0u8..0x20).map(char::from).collect::<String>(),
            "\u{7f}é漢字/x".to_string(),
        ] {
            let encoded = serde_json::to_string(&key).unwrap();
            assert_eq!(json_escaped_len(&key), encoded.len() - 2, "{key:?}");
        }
    }

    // Review r3 F7: a 16 KiB key of control characters (each stored as a
    // 6-byte escape) is still charged at least what is stored.
    #[test]
    fn a_control_character_key_is_charged_its_escaped_size() {
        for key in [
            "\u{1}".repeat(16 * 1024),
            format!("vendor-a/{}", "\u{1f}".repeat(4096)),
        ] {
            let encoded = serde_json::to_string(&key).unwrap().len() - 2;
            // key_expr, anchor and vendor in the audit record and its replay copy.
            let stored = 6 * encoded + 2 * 1024;
            let charged = charge_bytes(json_escaped_len(&key), 0, CAP);
            assert!(
                charged as usize >= stored,
                "charged {charged} < stored {stored}"
            );
        }
    }

    #[test]
    fn charge_bounds_a_v2_audit_and_replay_record() {
        use base64::Engine;
        for key_len in [1usize, 3, 28, 64, 256, 1024] {
            for payload_len in [0usize, 1, 700, 1200, CAP, CAP + 1] {
                for ctl in [false, true] {
                    let key = if ctl {
                        "\u{1}".repeat(key_len)
                    } else {
                        "k".repeat(key_len)
                    };
                    let anchor = key.clone();
                    let audit = serde_json::json!({
                        "schema_version": 2, "sample_kind": "delete", "receiving_site": "s".repeat(32),
                        "outcome": "observed", "namespace_slot": "a".repeat(32),
                        "namespace_attribution": "namespace_acl_inferred",
                        "policy_digest": format!("sha256:{}", "0".repeat(64)),
                        "policy_evidence": "rendered_configuration",
                        "event_id": format!("{}/2026/11/09/02/1794189653042-10000-v2-replay.ndjson#9999", "i".repeat(32)),
                        "anchor": anchor, "vendor": anchor, "identity_model": "per-uuid-slot-pool",
                        "key_expr": key, "ts": "2026-11-09T02:00:53.042Z", "src_zid": "f".repeat(32),
                        "source_provenance": "publisher_asserted",
                        "payload_hash": format!("sha256:{}", "0".repeat(64)), "payload_len": 18446744073709551615u64,
                        "payload_capture": "oversize", "onboarding_classification": "onboarding_hello",
                        "audit_subscriber_id": "i".repeat(32),
                    });
                    let mut replay = audit.clone();
                    if payload_len <= CAP {
                        replay["payload_bytes_b64"] = base64::engine::general_purpose::STANDARD
                            .encode(vec![0u8; payload_len])
                            .into();
                    }
                    let stored = serde_json::to_string(&audit).unwrap().len()
                        + serde_json::to_string(&replay).unwrap().len()
                        + 2;
                    let charged = charge_bytes(json_escaped_len(&key), payload_len, CAP);
                    assert!(
                    charged as usize >= stored,
                    "key {key_len} ctl {ctl} payload {payload_len}: charged {charged} < stored {stored}"
                );
                }
            }
        }
    }
}
