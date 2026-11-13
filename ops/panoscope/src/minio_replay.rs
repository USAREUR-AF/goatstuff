// SPDX-License-Identifier: Apache-2.0
//
// MinIO replay-on-boot — re-hydrates the panoscope in-RAM peer registry
// from the audit-subscriber's NDJSON archive in MinIO `audit-integrity`.
//
// Why this exists:
//   panoscope holds all observation state in RAM. A `docker compose up -d`
//   that recreates the container loses the prior session's accumulated
//   view — every peer that wasn't actively re-publishing in the seconds
//   following restart silently disappears from the COP. Synth-vendors
//   re-publish their schema-decl every 30s so they come back; real
//   portal-minted vendors do NOT publish schema-decls (only the synth
//   binary does), so without this replay they only re-appear when
//   they next publish a live sample. The audit-subscriber writes every
//   sample it saw (vendor, key_expr, src_zid, ts, payload_hash, ...) to
//   `audit-integrity/<site>/<YYYY>/<MM>/<DD>/<HH>/...ndjson` already;
//   this module reads that history at boot and feeds it into the same
//   topology + flow write-path that data_tail uses for live samples.
//
// The replay is current-state-only: it dedupes by (src_zid, key_expr)
// during a single replay pass so the topology gets one peer-upsert +
// one record_publish_declaration + one flow.record_event per unique
// tuple regardless of how many samples the original peer published.
// Cost scales with unique tuples (typically hundreds), not raw sample
// count (millions over 7d).
//
// Substrate: S3 walker mechanics + event parsing live in
// `panoscope_core::AuditTail` + `panoscope_core::AuditEvent` per the
// docs/design/stethoscope.md Phase B factor-out (Wave 1) plus the
// panoscope-core consolidation that landed via docs/design/flashlight.md
// Phase B (Wave 2). This module owns the apply-to-topology logic
// specific to panoscope's data model.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use panoscope_core::audit_tail::DEFAULT_AUDIT_FETCH_CONCURRENCY;
use panoscope_core::{AuditEvent, AuditTail, AuditTailConfig};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::model::{now_ms, FlowState, Topology};
use crate::zenoh_client::apply_audit_event;

/// Operator-tunable replay config. All fields are env-var driven from
/// `Config::from_env_or_default()`; defaults disable the path so a
/// missing MinIO substrate doesn't crash startup.
#[derive(Debug, Clone)]
pub struct MinioReplayConfig {
    pub enabled: bool,
    /// S3 endpoint URL (e.g. `http://198.18.0.X:9000` on-mesh).
    pub endpoint: String,
    /// AWS region label — MinIO ignores the value but the signer requires one.
    pub region: String,
    /// Bucket name (default `audit-integrity` — the Block 82S substrate).
    pub bucket: String,
    /// Object-key prefix (e.g. `site-a/` to scope to a single site). The
    /// replay walks `<prefix>YYYY/MM/DD/HH/...` so the day-level prefix
    /// alone is enough to bound the scan.
    pub key_prefix: String,
    pub access_key: String,
    pub secret_key: String,
    /// How far back to replay. The audit retention is typically 7d, so 168h
    /// is the
    /// natural ceiling; defaults to 24h to keep boot time short.
    pub window_hours: u32,
    /// Hard cap on the number of objects fetched per replay. Defends
    /// against runaway boot time if the bucket grows unexpectedly large.
    /// audit-subscriber writes ~12 objects/min ≈ 17K/day; 20K covers
    /// just over a day at the live cadence.
    pub max_objects: usize,
    /// Audit objects fetched at once by the walker; peak memory is about
    /// this many object bodies. `PANOSCOPE_MINIO_FETCH_CONCURRENCY`.
    pub fetch_concurrency: usize,
    /// Vendor prefixes to skip during replay. Audit retention is
    /// Object-Lock COMPLIANCE-protected (ADR 0811) so events cannot be
    /// deleted from MinIO, but a vendor that was retired from the portal
    /// still has events rolling in the 7d audit window — without this
    /// filter they resurface as ghost peers on the COP after every
    /// panoscope restart. Strict equality match against the audit
    /// event `vendor` field; not a wildcard.
    pub vendor_blocklist: HashSet<String>,
    /// Topic-prefix blocklist mirror of `Config::topic_blocklist`. Set
    /// from the same `PANOSCOPE_TOPIC_BLOCKLIST` env var so the replay
    /// path filters out the same noisy publishers the live `data_tail`
    /// drops. Prefix-match against `audit_event.key_expr`.
    pub topic_blocklist: Vec<String>,
}

impl Default for MinioReplayConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: String::new(),
            region: "us-east-1".into(),
            bucket: "audit-integrity".into(),
            key_prefix: String::new(),
            access_key: String::new(),
            secret_key: String::new(),
            window_hours: 24,
            max_objects: 20_000,
            fetch_concurrency: DEFAULT_AUDIT_FETCH_CONCURRENCY,
            vendor_blocklist: HashSet::new(),
            topic_blocklist: Vec::new(),
        }
    }
}

impl MinioReplayConfig {
    /// Build the panoscope-core walker config from panoscope's config.
    /// The walker doesn't know about `vendor_blocklist` (that's applied
    /// post-read on the panoscope side) — everything else maps 1:1.
    fn to_audit_tail_config(&self) -> AuditTailConfig {
        AuditTailConfig {
            enabled: self.enabled,
            endpoint: self.endpoint.clone(),
            region: self.region.clone(),
            bucket: self.bucket.clone(),
            key_prefix: self.key_prefix.clone(),
            access_key: self.access_key.clone(),
            secret_key: self.secret_key.clone(),
            request_timeout: std::time::Duration::from_secs(30),
            max_objects: self.max_objects,
            fetch_concurrency: self.fetch_concurrency,
        }
    }
}

/// Top-level entry: stream the audit log, dedupe by (src_zid, key_expr),
/// and feed each unique tuple into the topology + flow.
///
/// Lock discipline: the expensive part — fetching + parsing the
/// 168h MinIO audit window (~150s for ~60k objects / ~8M events) — runs
/// with NO `Topology`/`FlowState` locks held; it only builds an in-memory
/// `Vec`. The write locks are taken only for the fast in-memory apply
/// loop, in short batches, so a concurrent `/api/snapshot` read is never
/// starved. Invoked from a background task AFTER the HTTP listener binds
/// (see `main.rs`), so the replay never delays the server coming up — the
/// pre-F-168 inline-before-bind ordering left panoscope unbound (Traefik
/// 502) for the whole replay window on every restart.
///
/// Errors are converted to warnings — replay is best-effort. A MinIO
/// outage at boot must not prevent panoscope from running; it just
/// means the peer view will rebuild from live samples as before.
pub async fn replay_into_topology(
    cfg: &MinioReplayConfig,
    topology: &Arc<RwLock<Topology>>,
    flow: &Arc<RwLock<FlowState>>,
) {
    if !cfg.enabled {
        info!("minio_replay: disabled (PANOSCOPE_MINIO_REPLAY_ENABLED!=true)");
        return;
    }
    if cfg.access_key.is_empty() || cfg.secret_key.is_empty() {
        warn!("minio_replay: enabled but no credentials — skipping (set PANOSCOPE_MINIO_ACCESS_KEY + PANOSCOPE_MINIO_SECRET_KEY)");
        return;
    }

    let start = std::time::Instant::now();
    match do_replay(cfg, topology, flow).await {
        Ok(stats) => {
            info!(
                elapsed_ms = start.elapsed().as_millis() as u64,
                objects_listed = stats.objects_listed,
                objects_fetched = stats.objects_fetched,
                events_parsed = stats.events_parsed,
                events_blocked = stats.events_blocked,
                unique_tuples = stats.unique_tuples,
                unique_peers = stats.unique_peers,
                "minio_replay: complete"
            );
        }
        Err(e) => {
            warn!(error = %e, "minio_replay: failed — continuing with empty peer registry");
        }
    }
}

#[derive(Debug, Default)]
struct ReplayStats {
    objects_listed: usize,
    objects_fetched: usize,
    events_parsed: usize,
    events_blocked: usize,
    unique_tuples: usize,
    unique_peers: usize,
}

async fn do_replay(
    cfg: &MinioReplayConfig,
    topology: &Arc<RwLock<Topology>>,
    flow: &Arc<RwLock<FlowState>>,
) -> Result<ReplayStats> {
    let tail = AuditTail::new(cfg.to_audit_tail_config())?;
    info!(
        endpoint = %cfg.endpoint,
        bucket = %cfg.bucket,
        prefix_root = %cfg.key_prefix,
        window_hours = cfg.window_hours,
        "minio_replay: starting"
    );

    // panoscope-side filtering keeps the panoscope-core walker
    // application-agnostic — no vendor_filter passed in.
    //
    // STREAMED, NOT COLLECTED. This used to be
    //     let (events, walker_stats) = tail.recent_events(...).await?;
    // which materializes the ENTIRE window before applying any of it. On a busy
    // fabric that means tens of millions of events resident at once and tens of
    // GiB of peak memory, to derive a few hundred unique peers, and the
    // allocator never returns the arena, so panoscope would sit at that RSS
    // indefinitely after every cold start.
    //
    // The fix rests on a property of the fold: everything that decides whether
    // an event MATTERS is local. The blocklists and the (peer, key_expr) dedup
    // touch neither topology nor flow, and the dedup returns early on a repeat
    // — so 99.9995% of those events did one hash probe and were dropped. Only
    // the FIRST event per tuple reaches `apply_audit_event`, and there are
    // `unique_tuples` of those, not `events_parsed`.
    //
    // So: admit() runs inside the walker callback with no locks held and keeps
    // only survivors; the survivors are then applied under one lock
    // acquisition. Peak memory becomes O(unique tuples) rather
    // than O(window).
    //
    // ORDERING IS UNCHANGED, which matters because first-occurrence-wins is
    // what stamps a peer's last_seen. The walker yields events in exactly the
    // same order as before; we simply stop buffering them.
    let mut stats = ReplayStats::default();
    let mut seen_tuples: HashSet<(String, String)> = HashSet::new();
    let mut seen_peers: HashSet<String> = HashSet::new();
    let now = now_ms();

    // One entry per admitted tuple. Bounded by the fabric's distinct
    // (peer, topic) count, NOT by the audit volume behind it.
    let mut admitted: Vec<AdmittedEvent> = Vec::new();

    let walker_stats = tail
        .stream_recent_events(cfg.window_hours, None, |ev| {
            if let Some(a) = admit(
                &ev,
                &mut seen_tuples,
                &mut seen_peers,
                &cfg.vendor_blocklist,
                &cfg.topic_blocklist,
                now,
                &mut stats,
            ) {
                admitted.push(a);
            }
        })
        .await?;

    stats.objects_listed = walker_stats.objects_listed;
    stats.objects_fetched = walker_stats.objects_fetched;
    stats.events_parsed = walker_stats.events_parsed;

    // The write locks are held once, over `admitted.len()` upserts. The old
    // code re-acquired every 1000 events so a concurrent /api/snapshot could
    // interleave during a 191-second walk; that concern is gone now, because
    // the walk no longer happens under any lock and this apply pass is three
    // orders of magnitude shorter.
    {
        let mut topology = topology.write().await;
        let mut flow = flow.write().await;
        for a in &admitted {
            apply_audit_event(
                &mut topology,
                &mut flow,
                &a.vendor,
                &a.key_expr,
                &a.src_zid,
                a.event_ms,
            );
        }
    }

    stats.unique_tuples = seen_tuples.len();
    stats.unique_peers = seen_peers.len();
    Ok(stats)
}

/// One admitted (peer, topic) tuple, carrying only what the apply pass needs.
/// Deliberately owns its strings: it outlives the borrowed `AuditEvent`, which
/// the streaming walker drops as soon as the callback returns.
struct AdmittedEvent {
    vendor: String,
    key_expr: String,
    src_zid: String,
    event_ms: u64,
}

/// Decide whether an event contributes anything, WITHOUT touching topology or
/// flow — so this can run inside the walker callback with no locks held.
///
/// Returns `Some` only for the FIRST event of each (peer, key_expr) tuple.
/// Everything after that is a repeat and contributes nothing, which is why the
/// replay can stream: almost no events reach the apply pass.
#[allow(clippy::too_many_arguments)]
fn admit(
    ev: &AuditEvent,
    seen_tuples: &mut HashSet<(String, String)>,
    seen_peers: &mut HashSet<String>,
    vendor_blocklist: &HashSet<String>,
    topic_blocklist: &[String],
    now_ms: u64,
    stats: &mut ReplayStats,
) -> Option<AdmittedEvent> {
    if vendor_blocklist.contains(&ev.vendor) {
        stats.events_blocked += 1;
        return None;
    }
    if panoscope_core::types::topic_matches_blocklist(&ev.key_expr, topic_blocklist) {
        stats.events_blocked += 1;
        return None;
    }
    let peer_key = if ev.src_zid.is_empty() {
        ev.key_expr.clone()
    } else {
        ev.src_zid.clone()
    };
    if !seen_tuples.insert((peer_key.clone(), ev.key_expr.clone())) {
        return None;
    }
    seen_peers.insert(peer_key);
    // Stamp the peer's last_seen with the event's REAL publish time, not the
    // replay wall-clock — otherwise every replayed peer looks "just seen" and
    // the history-lens window filter can't tell live from historical (see
    // docs/design/panoscope-history-lens.md). Fall back to `now_ms` only when
    // the audit ts is missing/unparseable. Clamp to <= now so a skewed future
    // ts can't park a peer permanently "fresh".
    let event_ms = ev
        .ts_unix_ms()
        .filter(|t| *t > 0)
        .map(|t| (t as u64).min(now_ms))
        .unwrap_or(now_ms);
    Some(AdmittedEvent {
        vendor: ev.vendor.clone(),
        key_expr: ev.key_expr.clone(),
        src_zid: ev.src_zid.clone(),
        event_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(vendor: &str, key_expr: &str, src_zid: &str, ts: &str) -> AuditEvent {
        AuditEvent {
            vendor: vendor.to_string(),
            key_expr: key_expr.to_string(),
            ts: ts.to_string(),
            src_zid: src_zid.to_string(),
            payload_hash: "sha256:00".to_string(),
            payload_len: 0,
            payload_bytes_b64: None,
            audit_subscriber_id: "test".to_string(),
        }
    }

    fn admit_all(events: &[AuditEvent]) -> (Vec<AdmittedEvent>, ReplayStats) {
        let mut seen_tuples = HashSet::new();
        let mut seen_peers = HashSet::new();
        let mut stats = ReplayStats::default();
        let blocked: HashSet<String> = HashSet::new();
        let now = 4_000_000_000_000u64; // far future, so nothing clamps
        let out = events
            .iter()
            .filter_map(|e| {
                admit(
                    e,
                    &mut seen_tuples,
                    &mut seen_peers,
                    &blocked,
                    &[],
                    now,
                    &mut stats,
                )
            })
            .collect();
        (out, stats)
    }

    /// THE PROPERTY THE STREAMING FIX RESTS ON: a repeat of an
    /// already-seen (peer, key_expr) contributes nothing, so the replay never
    /// has to hold more than one admitted entry per distinct tuple. If this
    /// ever stops being true, streaming silently starts losing data and the
    /// memory win is not worth having.
    #[test]
    fn only_the_first_event_per_tuple_is_admitted() {
        let events = vec![
            ev("acme", "acme/telemetry", "zidA", "2026-08-19T10:00:00.000Z"),
            ev("acme", "acme/telemetry", "zidA", "2026-08-19T11:00:00.000Z"),
            ev("acme", "acme/telemetry", "zidA", "2026-08-19T12:00:00.000Z"),
        ];
        let (admitted, _) = admit_all(&events);
        assert_eq!(admitted.len(), 1, "a repeated tuple must admit exactly once");
    }

    /// FIRST OCCURRENCE WINS, and that is what stamps a peer's last_seen — so
    /// the walker's ordering is load-bearing. Streaming preserves that order;
    /// this pins the semantic so a future "optimisation" that reorders the walk
    /// fails here rather than silently ageing every peer wrongly.
    #[test]
    fn admitted_timestamp_comes_from_the_first_event_not_the_last() {
        let events = vec![
            ev("acme", "acme/telemetry", "zidA", "2026-08-19T10:00:00.000Z"),
            ev("acme", "acme/telemetry", "zidA", "2026-08-19T12:00:00.000Z"),
        ];
        let (admitted, _) = admit_all(&events);
        let first_ms = events[0].ts_unix_ms().unwrap() as u64;
        assert_eq!(admitted[0].event_ms, first_ms);
    }

    #[test]
    fn distinct_tuples_each_admit_once() {
        let events = vec![
            ev("acme", "acme/a", "zidA", "2026-08-19T10:00:00.000Z"),
            ev("acme", "acme/b", "zidA", "2026-08-19T10:00:01.000Z"),
            ev("beta", "beta/a", "zidB", "2026-08-19T10:00:02.000Z"),
            ev("acme", "acme/a", "zidA", "2026-08-19T10:00:03.000Z"), // repeat
        ];
        let (admitted, _) = admit_all(&events);
        assert_eq!(admitted.len(), 3);
    }

    #[test]
    fn vendor_blocklist_rejects_and_counts() {
        let mut seen_tuples = HashSet::new();
        let mut seen_peers = HashSet::new();
        let mut stats = ReplayStats::default();
        let mut blocked = HashSet::new();
        blocked.insert("acme".to_string());
        let e = ev("acme", "acme/a", "zidA", "2026-08-19T10:00:00.000Z");
        let got = admit(
            &e,
            &mut seen_tuples,
            &mut seen_peers,
            &blocked,
            &[],
            4_000_000_000_000,
            &mut stats,
        );
        assert!(got.is_none());
        assert_eq!(stats.events_blocked, 1);
    }

    /// A blocked event must not consume the tuple — otherwise un-blocking a
    /// vendor would need a restart to take effect on an already-walked tuple.
    #[test]
    fn a_blocked_event_does_not_claim_the_tuple() {
        let mut seen_tuples = HashSet::new();
        let mut seen_peers = HashSet::new();
        let mut stats = ReplayStats::default();
        let mut blocked = HashSet::new();
        blocked.insert("acme".to_string());
        let e = ev("acme", "acme/a", "zidA", "2026-08-19T10:00:00.000Z");
        admit(&e, &mut seen_tuples, &mut seen_peers, &blocked, &[], 4_000_000_000_000, &mut stats);
        assert!(seen_tuples.is_empty(), "a blocked event must not mark the tuple seen");
    }

    /// An empty src_zid falls back to the key_expr as the peer key — pinned
    /// because it decides what counts as a distinct peer.
    #[test]
    fn empty_src_zid_falls_back_to_key_expr_as_peer_key() {
        let events = vec![
            ev("acme", "acme/a", "", "2026-08-19T10:00:00.000Z"),
            ev("acme", "acme/a", "", "2026-08-19T10:00:01.000Z"),
        ];
        let (admitted, _) = admit_all(&events);
        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0].src_zid, "");
    }

    /// A future timestamp is clamped to now, so a skewed publisher cannot park
    /// a peer permanently "fresh" in the history lens.
    #[test]
    fn future_timestamp_is_clamped_to_now() {
        let mut seen_tuples = HashSet::new();
        let mut seen_peers = HashSet::new();
        let mut stats = ReplayStats::default();
        let blocked = HashSet::new();
        let now = 1_000_000_000_000u64;
        let e = ev("acme", "acme/a", "zidA", "2099-01-01T00:00:00.000Z");
        let got = admit(&e, &mut seen_tuples, &mut seen_peers, &blocked, &[], now, &mut stats)
            .expect("should admit");
        assert_eq!(got.event_ms, now, "a future ts must clamp to now");
    }

    /// An unparseable timestamp falls back to now rather than dropping the
    /// event — the tuple still matters even if its clock does not parse.
    #[test]
    fn unparseable_timestamp_falls_back_to_now() {
        let mut seen_tuples = HashSet::new();
        let mut seen_peers = HashSet::new();
        let mut stats = ReplayStats::default();
        let blocked = HashSet::new();
        let now = 1_000_000_000_000u64;
        let e = ev("acme", "acme/a", "zidA", "not-a-timestamp");
        let got = admit(&e, &mut seen_tuples, &mut seen_peers, &blocked, &[], now, &mut stats)
            .expect("should still admit");
        assert_eq!(got.event_ms, now);
    }
}
