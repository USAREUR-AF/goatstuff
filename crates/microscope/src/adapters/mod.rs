//! Source adapters — one per actor-attributable stream (design §Sources).
//! Stethoscope plugin idiom: self-contained, reads the OWNING store, degrades
//! to `skipped` when unconfigured. Adding a stream = one file + one line in
//! [`all`].

use async_trait::async_trait;
use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::time::Duration;

use panoscope_core::audit_tail::{AuditTail, AuditTailConfig};

use crate::config::Config;
use crate::event::{SourceState, SourceStatus, TimelineEvent};
use crate::spine::EntitySpine;

pub mod console_audit;
pub mod fabric_audit;
pub mod hydra_consents;
mod kratos_sessions;
pub mod netbird_events;
pub mod netbird_peers;
pub mod operator_audit;
pub mod surface_visits;
pub mod portal_audit;

/// Shared MinIO audit-tail constructor for the two audit adapters.
/// `Ok(None)` ⇒ not configured on this deployment (adapters report
/// `skipped`): none of the three required fields are set, which is the
/// normal steady state for a site that has not turned audit tail on.
/// `Err` ⇒ configured but broken — only some of the three fields are set,
/// or the endpoint does not parse — and the caller must surface it rather
/// than fold it into a quiet skip.
pub(crate) fn tail_from(cfg: &Config) -> Result<Option<AuditTail>, String> {
    if cfg.audit_endpoint.is_empty()
        && cfg.audit_access_key.is_empty()
        && cfg.audit_secret_key.is_empty()
    {
        return Ok(None);
    }
    AuditTail::new(AuditTailConfig {
        enabled: true,
        endpoint: cfg.audit_endpoint.clone(),
        region: cfg.audit_region.clone(),
        bucket: cfg.audit_bucket.clone(),
        key_prefix: cfg.audit_key_prefix.clone(),
        access_key: cfg.audit_access_key.clone(),
        secret_key: cfg.audit_secret_key.clone(),
        request_timeout: Duration::from_secs(30),
        max_objects: AUDIT_MAX_OBJECTS,
    })
    .map(Some)
    .map_err(|e| e.to_string())
}

/// Default for [`Config::audit_max_rows`]: the most timeline rows one audit
/// adapter keeps from its window. A busy window holds millions of audit
/// events; a timeline page needs the newest few thousand at most.
pub const DEFAULT_AUDIT_MAX_ROWS: usize = 2_000;

/// The audit adapters' row ceiling for this config (0 means the default).
pub(crate) fn audit_max_rows(cfg: &Config) -> usize {
    if cfg.audit_max_rows == 0 {
        DEFAULT_AUDIT_MAX_ROWS
    } else {
        cfg.audit_max_rows
    }
}

/// The object ceiling handed to [`AuditTail`] by [`tail_from`]; reaching it
/// means older hours of the window were not read.
pub(crate) const AUDIT_MAX_OBJECTS: usize = 5_000;

/// Keeps the newest `max` timeline rows offered to it, by canonical
/// timestamp, and counts every row offered. The audit adapters feed it from
/// `AuditTail::stream_recent_events`, so memory is bounded by `max` rather
/// than by the window's event count.
///
/// When nothing is dropped, [`Self::finish`] returns the rows in the order
/// they were offered — the same rows, in the same order, the adapters produced
/// before the bound existed.
pub(crate) struct NewestRows {
    max: usize,
    seen: usize,
    seq: u64,
    heap: BinaryHeap<Reverse<Ranked>>,
}

/// A row ranked by (canonical timestamp, arrival order).
struct Ranked {
    ts: String,
    seq: u64,
    ev: TimelineEvent,
}

impl PartialEq for Ranked {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Ranked {}
impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        self.ts.cmp(&other.ts).then(self.seq.cmp(&other.seq))
    }
}

impl NewestRows {
    pub(crate) fn new(max: usize) -> Self {
        Self {
            max,
            seen: 0,
            seq: 0,
            heap: BinaryHeap::with_capacity(max.min(4_096) + 1),
        }
    }

    /// Offer one row. `ts` is the row's canonical timestamp; `build` receives
    /// it to put in `TimelineEvent::ts`, and runs only when the row is kept,
    /// so a dropped row costs no allocation beyond `ts`.
    pub(crate) fn offer(&mut self, ts: String, build: impl FnOnce(String) -> TimelineEvent) {
        self.seen += 1;
        let seq = self.seq;
        self.seq += 1;
        if self.max == 0 {
            return;
        }
        if self.heap.len() >= self.max {
            // Full: admit only a row newer than the oldest kept one.
            match self.heap.peek() {
                Some(Reverse(oldest)) if (ts.as_str(), seq) > (oldest.ts.as_str(), oldest.seq) => {
                    self.heap.pop();
                }
                _ => return,
            }
        }
        let ev = build(ts.clone());
        self.heap.push(Reverse(Ranked { ts, seq, ev }));
    }

    /// Rows currently kept.
    #[cfg(test)]
    pub(crate) fn kept(&self) -> usize {
        self.heap.len()
    }

    /// The kept rows in arrival order, and how many were offered in all.
    pub(crate) fn finish(self) -> (Vec<TimelineEvent>, usize) {
        let mut rows: Vec<Ranked> = self.heap.into_iter().map(|Reverse(r)| r).collect();
        rows.sort_by_key(|r| r.seq);
        (rows.into_iter().map(|r| r.ev).collect(), self.seen)
    }
}

/// The coverage clause the audit adapters put in their detail line when the
/// row ceiling dropped rows, or `None` when every matching row is shown.
pub(crate) fn rows_clause(kept: usize, seen: usize) -> Option<String> {
    (seen > kept).then(|| {
        format!(
            "ROWS CAPPED: showing the newest {kept} of {seen} matching events (MICROSCOPE_AUDIT_MAX_ROWS)"
        )
    })
}

pub struct AdapterOutput {
    pub events: Vec<TimelineEvent>,
    pub status: SourceStatus,
}

impl AdapterOutput {
    pub fn skipped(source: &'static str, why: &str) -> Self {
        Self {
            events: Vec::new(),
            status: SourceStatus {
                source,
                state: SourceState::Skipped,
                detail: why.to_string(),
                n: 0,
            },
        }
    }
    pub fn error(source: &'static str, err: impl std::fmt::Display) -> Self {
        Self {
            events: Vec::new(),
            status: SourceStatus {
                source,
                state: SourceState::Error,
                detail: err.to_string(),
                n: 0,
            },
        }
    }
    pub fn ok(source: &'static str, detail: &str, events: Vec<TimelineEvent>) -> Self {
        let n = events.len();
        Self {
            events,
            status: SourceStatus {
                source,
                state: SourceState::Ok,
                detail: detail.to_string(),
                n,
            },
        }
    }
}

#[async_trait]
pub trait SourceAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    async fn collect(&self, cfg: &Config, spine: &EntitySpine) -> AdapterOutput;
}

/// The adapter registry. Order is presentation order in per-source status.
pub fn all() -> Vec<Box<dyn SourceAdapter>> {
    vec![
        Box::new(console_audit::ConsoleAudit),
        Box::new(kratos_sessions::KratosSessions),
        Box::new(hydra_consents::HydraConsents),
        Box::new(operator_audit::OperatorAudit),
        Box::new(portal_audit::PortalAudit),
        Box::new(fabric_audit::FabricAudit),
        // Sits next to operator_audit and fabric_audit because it reads the
        // same MinIO tier, and after hydra_consents because it answers the
        // question that source raises and cannot close: the grant says a person
        // reached a surface, this says how often and when.
        Box::new(surface_visits::SurfaceVisits),
        Box::new(netbird_events::NetbirdEvents),
        Box::new(netbird_peers::NetbirdPeers),
    ]
}

/// A synthetic audit record for the adapters' tests: key `key`, timestamp
/// `i` milliseconds after midnight.
#[cfg(test)]
pub(crate) fn test_audit_event(key: &str, i: usize) -> panoscope_core::AuditEvent {
    panoscope_core::AuditEvent {
        vendor: key.split('/').next().unwrap_or_default().to_string(),
        key_expr: key.to_string(),
        ts: format!(
            "2026-01-01T{:02}:{:02}:{:02}.{:03}Z",
            (i / 3_600_000) % 24,
            (i / 60_000) % 60,
            (i / 1_000) % 60,
            i % 1_000
        ),
        src_zid: String::new(),
        payload_hash: String::new(),
        payload_len: 8,
        payload_bytes_b64: None,
        audit_subscriber_id: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing set at all (the normal, steady state for a site that has
    /// never turned audit tail on) must stay a quiet skip, not an error.
    /// AuditTail holds a reqwest::Client and does not derive Debug, so the
    /// branches are matched explicitly rather than via an `assert!` with a
    /// `{:?}`-formatted value.
    #[test]
    fn tail_from_unconfigured_is_ok_none() {
        let cfg = Config::default();
        match tail_from(&cfg) {
            Ok(None) => {}
            Ok(Some(_)) => panic!("expected Ok(None) for a fully unconfigured site, got Ok(Some(_))"),
            Err(e) => panic!("expected Ok(None) for a fully unconfigured site, got Err({e})"),
        }
    }

    /// A malformed endpoint (present but not a valid URL) must surface its
    /// real parse error through tail_from's Result, not get discarded by
    /// `.ok()` and reported as "unset" by the caller.
    #[test]
    fn tail_from_malformed_endpoint_surfaces_parse_error() {
        let cfg = Config {
            audit_endpoint: "not a url".to_string(),
            audit_access_key: "access-key".to_string(),
            audit_secret_key: "secret-key".to_string(),
            ..Default::default()
        };
        let err = match tail_from(&cfg) {
            Err(e) => e,
            Ok(None) => panic!("expected Err surfacing the parse failure, got Ok(None)"),
            Ok(Some(_)) => panic!("expected Err surfacing the parse failure, got Ok(Some(_))"),
        };
        assert!(
            err.contains("not a url"),
            "expected the real parse error to name the malformed endpoint, got: {err}"
        );
    }

    /// Partial configuration (var-name drift between the secret mint and
    /// the consumer leaves one field empty while the others are set) is a
    /// real misconfiguration, not an intentional skip, and must surface.
    #[test]
    fn tail_from_partial_config_is_an_error_not_a_skip() {
        let cfg = Config {
            audit_endpoint: "http://198.18.0.9:9000".to_string(),
            audit_access_key: "access-key".to_string(),
            audit_secret_key: String::new(),
            ..Default::default()
        };
        let err = match tail_from(&cfg) {
            Err(e) => e,
            Ok(None) => panic!("expected Err for a partially configured tail, got Ok(None)"),
            Ok(Some(_)) => panic!("expected Err for a partially configured tail, got Ok(Some(_))"),
        };
        assert!(
            err.contains("secret_key"),
            "expected the error to name the missing field, got: {err}"
        );
    }

    fn row(ts: &str, n: usize) -> TimelineEvent {
        TimelineEvent {
            ts: ts.to_string(),
            source: "test",
            action: "publish".into(),
            actor: "a".into(),
            target: Some(format!("k/{n}")),
            summary: String::new(),
            kind: crate::event::EventKind::classify("fabric.publish"),
            outcome: crate::event::Outcome::Unrecorded,
            direction: crate::event::Direction::BySubject,
            detail: None,
            attribution: crate::event::Attribution::Strong,
            raw: None,
        }
    }

    fn ts_of(i: usize) -> String {
        // Fixed-width, so byte order is chronological, as timefmt guarantees.
        format!("2026-01-01T00:00:00.{i:09}Z")
    }

    /// Under the ceiling nothing changes: every row, in offered order.
    #[test]
    fn newest_rows_under_the_ceiling_keeps_everything_in_order() {
        let mut b = NewestRows::new(10);
        for i in [5usize, 1, 9, 3] {
            b.offer(ts_of(i), |ts| row(&ts, i));
        }
        let (rows, seen) = b.finish();
        assert_eq!(seen, 4);
        let order: Vec<_> = rows.iter().map(|r| r.target.clone().unwrap()).collect();
        assert_eq!(order, vec!["k/5", "k/1", "k/9", "k/3"]);
        assert_eq!(rows_clause(rows.len(), seen), None);
    }

    /// A large window keeps exactly `max` rows, and they are the newest ones,
    /// whatever order the walk delivers them in.
    #[test]
    fn newest_rows_bounds_a_large_stream_to_the_newest() {
        const N: usize = 250_000;
        const K: usize = 1_000;
        let mut b = NewestRows::new(K);
        let mut built = 0usize;
        // Interleave: odd indices descending, then even ascending.
        let order = (0..N).filter(|i| i % 2 == 1).rev().chain((0..N).filter(|i| i % 2 == 0));
        for i in order {
            b.offer(ts_of(i), |ts| {
                built += 1;
                row(&ts, i)
            });
            assert!(b.kept() <= K);
        }
        assert!(built < N / 10, "dropped rows must not be built ({built})");
        let (rows, seen) = b.finish();
        assert_eq!(seen, N);
        assert_eq!(rows.len(), K);
        let mut got: Vec<String> = rows.into_iter().map(|r| r.ts).collect();
        got.sort();
        let want: Vec<String> = (N - K..N).map(ts_of).collect();
        assert_eq!(got, want);
        assert!(rows_clause(K, seen).unwrap().contains(&format!("newest {K} of {N}")));
    }

    #[test]
    fn audit_max_rows_zero_means_default() {
        assert_eq!(audit_max_rows(&Config::default()), DEFAULT_AUDIT_MAX_ROWS);
        let cfg = Config {
            audit_max_rows: 7,
            ..Default::default()
        };
        assert_eq!(audit_max_rows(&cfg), 7);
    }
}
