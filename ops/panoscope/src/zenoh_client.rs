// SPDX-License-Identifier: Apache-2.0
//
// Zenoh client wrapper. Owns the Session + drives two background tasks:
//
//   - data_tail: subscribes to `**`, samples through a per-key TokenBucket,
//     records publisher peer + topic into the FlowState (EMA decay) and
//     Topology (publish declaration). Drops payload bytes immediately.
//
//   - admin_tail: subscribes to `@/*/router/**`, treats each sample as a
//     peer-existence signal. v1 best-effort — exact admin keyspace shape
//     varies by Zenoh version; we use admin samples as supplementary peer
//     observation but the data-tail is the primary signal.
//
// Sampling + back-pressure per docs/design/panoscope.md §"Sampling +
// back-pressure". Defaults: 50 events/sec/topic, 100ms WS coalesce tick.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::sync::{broadcast, RwLock, Semaphore};
use tracing::{debug, info, warn};

use crate::config::TlsConfig;
use crate::model::{now_ms, FlowState, NodeKind, PeerId, TokenBucket, Topology, TopicKey};
use panoscope_core::census::{census_sources, parse_resource_key, ResourceRole};
use crate::schema::{FamilyMapResolver, SchemaResolver};

/// Broadcast capacity for the WS event channel. Lossy: slow WS clients
/// miss events when the buffer wraps; they can resync via /api/snapshot.
const EVENTS_CHANNEL_CAPACITY: usize = 1024;

/// Shared state across the HTTP layer and the zenoh-driven tail loops.
#[derive(Clone)]
pub struct SharedState {
    pub topology: Arc<RwLock<Topology>>,
    pub flow: Arc<RwLock<FlowState>>,
    pub stats: Arc<RwLock<Stats>>,
    pub samples: Arc<RwLock<crate::sample_buffer::SampleBuffer>>,
    pub events: broadcast::Sender<WsEvent>,
    pub schema_resolver: Arc<dyn SchemaResolver>,
    /// Slot-alias resolver (ADR 1039). Refreshed from the portal's
    /// `GET /api/aliases` on the same cadence as schemas (see
    /// `main.rs::spawn_alias_refresher`). Empty until the first successful
    /// poll, and empty forever when the portal is unconfigured/unreachable —
    /// in which case every node renders its truncated UUID, never an error.
    /// Read in `build_nodes_view` to populate `NodeView::slot_alias`. The raw
    /// UUID stays the canonical `peer_id`; this only adds a display field.
    pub aliases: Arc<RwLock<panoscope_core::AliasRegistry>>,
    /// Why `aliases` holds what it holds (ADR 1039 honesty arm). Maintained
    /// by the same refresher and surfaced on `/api/legend` so an operator
    /// staring at raw UUIDs can tell a deploy gap (no API key) from a portal
    /// outage from "this goatnet really has no aliases". Without it those
    /// three render identically and a UUID-display gap recurs unnoticed.
    pub alias_health: Arc<RwLock<panoscope_core::AliasRegistryHealth>>,
    /// The live Zenoh session, populated once `run` opens it. Held so the
    /// HTTP layer can issue the on-demand stale-recall `get` against the
    /// router's storage when a topic's in-RAM ring is empty
    /// (panoscope-stale-recall B1). `None` before the session opens and in
    /// tests (the recall path then degrades to today's empty-state).
    pub session: Arc<RwLock<Option<zenoh::Session>>>,
    /// Ceiling on client-triggered fabric queries in flight at once.
    ///
    /// Panoscope is the one inspection surface where an HTTP request from a
    /// browser turns into a `session.get()` against the router's storage: the
    /// hygiene retrievability probe (`storage_answers`, one `get` per fan-out
    /// template, 3s timeout each) and the stale-recall in `/api/samples`
    /// (`get_last_retained`, fired whenever a topic's in-RAM ring is empty).
    /// Every other surface — goat-map, flashlight, stethoscope, microscope,
    /// the portal — answers from memory and cannot push load onto the fabric
    /// no matter how hard it is clicked.
    ///
    /// Without a ceiling, N operators inspecting M topics put N×M queries at
    /// the router, and a topic that never publishes has a permanently-empty
    /// ring, so its recall fires on EVERY view. Routers can wedge under
    /// query pressure, which is why this is a
    /// hard cap rather than a comment asking callers to be careful.
    ///
    /// Deliberately a `try_acquire`, never a wait: saturation SKIPS the probe
    /// and the caller reports its existing honest "could not look" state —
    /// `Unconfirmed` for hygiene, the empty ring for samples. Queueing would
    /// convert fabric pressure into HTTP latency and eventually into held
    /// connections, which is the wedge one layer up.
    pub fabric_probes: Arc<Semaphore>,
    /// The snapshot build shared across viewers and WebSocket connects
    /// (`snapshot_view::shared_snapshot_build`).
    pub snapshot_build: crate::snapshot_view::SnapshotBuildCache,
    /// The shared fabric model the `/api/view*` endpoints serve from, kept
    /// current by `view::service::spawn_model_builder`.
    pub view_model: crate::view::model::ModelHandle,
    /// Samples the `**` subscriber dropped because the data tail's queue was
    /// full (`DropOnFull`). Lock-free so the Zenoh callback can bump it; read
    /// into `/api/stats` as `data_samples_dropped_backpressure`.
    pub ingest_backpressure_drops: Arc<std::sync::atomic::AtomicU64>,
}

/// Concurrent client-triggered fabric queries allowed across the whole
/// process. Small on purpose: these are diagnostics, and an operator waiting
/// one refresh for a retrievability verdict costs nothing next to putting
/// storage-query pressure on a router the whole site depends on.
pub const MAX_INFLIGHT_FABRIC_PROBES: usize = 4;

impl SharedState {
    pub fn new(schema_resolver: Arc<dyn SchemaResolver>) -> Self {
        let (events, _rx) = broadcast::channel(EVENTS_CHANNEL_CAPACITY);
        Self {
            topology: Arc::new(RwLock::new(Topology::default())),
            flow: Arc::new(RwLock::new(FlowState::default())),
            stats: Arc::new(RwLock::new(Stats::default())),
            samples: Arc::new(RwLock::new(crate::sample_buffer::SampleBuffer::default())),
            events,
            schema_resolver,
            aliases: Arc::new(RwLock::new(panoscope_core::AliasRegistry::empty())),
            alias_health: Arc::new(RwLock::new(
                panoscope_core::AliasRegistryHealth::default(),
            )),
            session: Arc::new(RwLock::new(None)),
            fabric_probes: Arc::new(Semaphore::new(MAX_INFLIGHT_FABRIC_PROBES)),
            snapshot_build: Arc::new(tokio::sync::Mutex::new(None)),
            view_model: Arc::new(tokio::sync::RwLock::new(Arc::new(
                crate::view::model::FabricModel::empty(),
            ))),
            ingest_backpressure_drops: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Stats {
    pub data_samples_seen: u64,
    pub data_samples_recorded: u64,
    pub data_samples_dropped: u64,
    /// Samples dropped before ingest because the data tail's bounded queue
    /// was full. Non-zero means panoscope fell behind and shed load rather
    /// than blocking the router. Filled from
    /// `SharedState::ingest_backpressure_drops` when `/api/stats` is served.
    pub data_samples_dropped_backpressure: u64,
    /// Samples received but with no zenoh SourceInfo and no key-expression
    /// prefix we could attribute to a peer. Loud counter for the silent-skip
    /// class (companion to F-133 detection-discipline).
    pub data_samples_orphaned: u64,
    /// Samples received without zenoh SourceInfo, attributed via the
    /// key-expression vendor/system prefix fallback. Normal in zenoh-1.x
    /// where publishers don't attach SourceInfo by default.
    pub data_samples_attributed_via_topic: u64,
    pub admin_samples_seen: u64,
    pub last_data_sample_ms: u64,
    pub last_admin_sample_ms: u64,
}

/// Event emitted to WS subscribers on each interesting state change.
/// Lossy channel — slow consumers miss events but the canonical state
/// is always available via /api/snapshot for full resync.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WsEvent {
    /// New peer ZID seen on data or admin tail.
    PeerSeen { peer_id: String, ts_ms: u64 },
    /// A publish was sampled into the FlowState (token bucket admitted).
    PublishSampled {
        peer_id: String,
        topic: String,
        ts_ms: u64,
        ema_eps: f64,
    },
    /// A publish was dropped by the sampling token bucket.
    PublishDropped {
        peer_id: String,
        topic: String,
        ts_ms: u64,
    },
    /// A sample landed in the per-topic ring buffer. Emitted ungated
    /// (every sample, not token-bucketed) to drive operator-opt-in
    /// LIVE-follow on the stream pane. Per-connection watch sets in
    /// `ws.rs` filter these so only watched topics reach a given
    /// browser. Payload bytes are NOT carried — the WS is a wake-up
    /// signal; bytes come via the existing /api/samples fetch.
    SampleArrived { topic: String, ts_ms: u64 },
}

impl Default for SharedState {
    fn default() -> Self {
        Self::new(Arc::new(FamilyMapResolver::new()))
    }
}

/// Bucketed sampling state per topic. Lives inside the data-tail task
/// (not in shared state) — no lock contention on the hot path.
#[derive(Default)]
struct TopicBuckets {
    buckets: HashMap<TopicKey, TokenBucket>,
    refill_per_sec: f64,
    capacity: f64,
}

impl TopicBuckets {
    fn new(refill_per_sec: f64) -> Self {
        Self {
            buckets: HashMap::new(),
            refill_per_sec,
            capacity: refill_per_sec, // 1-second-worth burst
        }
    }

    fn try_consume(&mut self, topic: &str, now: Instant) -> bool {
        let bucket = self
            .buckets
            .entry(topic.to_string())
            .or_insert_with(|| TokenBucket::new(self.capacity, self.refill_per_sec));
        bucket.try_consume(now)
    }
}

/// EMA time constant (seconds). 5.0 → events older than ~30s have
/// effectively-zero weight.
pub const EMA_TAU_SECS: f64 = 5.0;

/// Run the panoscope Zenoh client. Opens a session against the
/// configured endpoint and spawns the data + admin tail tasks. Returns
/// the shared state for the HTTP layer to consume.
///
/// This is a v1 best-effort wiring — exact API shape against
/// `zenoh = 1.9.0` is verified at compile + first-run smoke; if Zenoh
/// API surfaces shift in 2.x we adapt then.
pub async fn run(
    endpoint: &str,
    tls: &TlsConfig,
    sampling_per_topic_eps: u32,
    topic_blocklist: Vec<String>,
    schema_resolver: Arc<dyn SchemaResolver>,
) -> Result<SharedState> {
    let state = SharedState::new(schema_resolver);

    info!(
        endpoint,
        mtls = tls.enabled,
        sampling_per_topic_eps,
        "opening zenoh session"
    );

    let mut config = zenoh::Config::default();
    // mode=client — panoscope is a passive observer that only subscribes,
    // it does not need to participate in peer-to-peer routing. In the
    // default `peer` mode, subscriber declarations are kept local and
    // never propagate to the router for data forwarding; `zenoh::open`
    // returns Ok even when no transport actually connects (the session
    // is "open" locally and isolated). Client mode forces a real
    // connection to the configured endpoint and makes the router aware
    // of our subscriber.
    config
        .insert_json5("mode", "\"client\"")
        .map_err(|e| anyhow::anyhow!("invalid zenoh mode config: {e}"))?;
    config
        .insert_json5("connect/endpoints", &format!("[\"{endpoint}\"]"))
        .map_err(|e| anyhow::anyhow!("invalid zenoh endpoint config: {e}"))?;

    // Inject client-side mTLS material into the Zenoh config when
    // tls.enabled is true. No-op otherwise. Behavior unchanged from the
    // prior inline block — substrate factored out into panoscope-core so
    // flashlight reuses the same mTLS setup (docs/design/flashlight.md
    // Phase B).
    panoscope_core::apply_tls_to_zenoh_config(&mut config, tls)?;

    let session = zenoh::open(config)
        .await
        .map_err(|e| anyhow::anyhow!("zenoh::open failed: {e}"))
        .context("opening zenoh session")?;

    info!(zid = %session.zid(), "zenoh session open");

    // Retain the session for the HTTP layer's on-demand stale-recall `get`
    // (panoscope-stale-recall B1). Cheap clone — Session is Arc-backed.
    *state.session.write().await = Some(session.clone());

    let own_zid = session.zid().to_string();
    // Before the live subscribe loop opens, pull historical schema
    // declarations from the router's storage. Zenoh pub/sub gives us
    // ONLY samples that arrive after declare_subscriber; the storage
    // plugin holds attest/schema/** as a queryable and serves history
    // via session.get(). Without this, every panoscope restart waits
    // for synth-vendors to re-publish their one-shot self-registration,
    // leaving the COP showing topic-prefix ghost peers in the meantime.
    if let Err(e) = replay_schema_history(&session, &state).await {
        warn!(error = %e, "schema-history replay failed; continuing — live subs will populate as new decls arrive");
    }
    spawn_data_tail(
        session.clone(),
        state.clone(),
        sampling_per_topic_eps,
        topic_blocklist,
    )
    .await?;
    spawn_admin_tail(session.clone(), state.clone(), own_zid).await?;

    Ok(state)
}

/// Query the router's storage for every cached attest/schema/** sample
/// and apply each to the topology as if it had arrived live. Runs once,
/// at session-open, before subscriber declarations. Fail-soft — any
/// error here just means the COP starts cold and warms up as live
/// schema decls arrive.
async fn replay_schema_history(
    session: &zenoh::Session,
    state: &SharedState,
) -> Result<()> {
    use zenoh::query::{ConsolidationMode, QueryTarget};
    // Three attempts with 2s delay between. The first attempt
    // sometimes fires before the client session has finished wiring
    // up routes to remote queryables (the storage on the router).
    // target=All (not AllComplete) — storage queryables may not be
    // flagged "complete"; AllComplete excluded them and returned 0.
    let mut total_applied: u64 = 0;
    for attempt in 1..=3u32 {
        info!(attempt, "schema-history: querying attest/schema/** from storage");
        let replies = match session
            .get("attest/schema/**")
            .target(QueryTarget::All)
            .consolidation(ConsolidationMode::None)
            .timeout(std::time::Duration::from_secs(10))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "schema-history: get failed");
                continue;
            }
        };
        let now_millis = now_ms();
        let mut applied: u64 = 0;
        for (key, payload_bytes) in drain_replies(&replies).await {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&payload_bytes) {
                let mut topology = state.topology.write().await;
                apply_schema_declaration(&mut topology, &key, &json, now_millis);
                applied += 1;
            }
        }
        info!(attempt, applied, "schema-history: attempt complete");
        total_applied += applied;
        if applied > 0 {
            break; // got data; stop retrying
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    info!(total_applied, "schema-history: replay finished");

    // Periodic re-poll fallback — every 60s repeat the query so a
    // late-joining storage or recently-restarted peer's schema decl
    // gets caught even if the startup attempts missed it. Cheap (a
    // single get per minute) and resilient.
    let session_clone = session.clone();
    let state_clone = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        interval.tick().await; // skip first immediate tick
        loop {
            interval.tick().await;
            let Ok(replies) = session_clone
                .get("attest/schema/**")
                .target(QueryTarget::All)
                .consolidation(ConsolidationMode::None)
                .timeout(std::time::Duration::from_secs(5))
                .await
            else {
                continue;
            };
            let now_millis = now_ms();
            for (key, payload_bytes) in drain_replies(&replies).await {
                if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&payload_bytes) {
                    let mut topology = state_clone.topology.write().await;
                    apply_schema_declaration(&mut topology, &key, &json, now_millis);
                }
            }
        }
    });
    Ok(())
}

/// On-demand stale-recall (panoscope-stale-recall B1). When a topic's
/// in-RAM ring is empty, query the router's Zenoh storage for the single
/// most-recent retained sample on the concrete topic key and return it as
/// a `stale`-flagged `SampleEntry`. Reuses `replay_schema_history`'s proven
/// query shape — `QueryTarget::All` (storage queryables aren't flagged
/// "complete"; `AllComplete` excludes them and returns 0) +
/// `ConsolidationMode::None` — but targets one concrete key and keeps only
/// the newest reply.
///
/// Returns `None` when storage has nothing for the key: an unregistered
/// vendor prefix (no `<vendor>/**` storage on the router), a topic aged
/// past the storage `time_to_live` (typically 7d), or one that genuinely
/// never published. The caller then keeps today's `[no samples yet…]`
/// empty state. Fail-soft — any query error is logged and treated as
/// "nothing retained", never surfaced as an endpoint error.
/// Retrievability probe for the pub/sub-hygiene diagnostic: does ANY storage
/// answer a bounded `get` on this selector (which may carry `*` wildcards)?
/// Unlike `get_last_retained` we don't decode — a single reply object proves a
/// storage serves the key space. Confirm-only by contract: `false` means "no
/// storage answered within the window," NOT "no storage exists" (the caller
/// reports that as `Unconfirmed`, never absence). Fail-soft on query error.
pub async fn storage_answers(session: &zenoh::Session, selector: &str) -> bool {
    use zenoh::query::{ConsolidationMode, QueryTarget};
    let replies = match session
        .get(selector)
        .target(QueryTarget::All)
        .consolidation(ConsolidationMode::None)
        .timeout(std::time::Duration::from_secs(3))
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!(selector, error = %e, "hygiene probe: storage get failed");
            return false;
        }
    };
    // One reply (Ok or Err result) means a storage responded for this key.
    replies.recv_async().await.is_ok()
}

/// True when `topic` is a concrete key a storage `get` can be issued for.
///
/// The view's `publishes` list mixes topic keys with group labels such as
/// `<anchor>/aircraft/* (55 topics)`, and the side panel sends either back as
/// `?topic=`. A label is not a key expression (zenoh rejects it) and a
/// wildcard key is not a single topic, so neither reaches the fabric.
pub fn is_recallable_topic(topic: &str) -> bool {
    if topic.is_empty() || topic.contains(['*', '$', '?', '#', ' ']) {
        return false;
    }
    zenoh::key_expr::KeyExpr::try_from(topic).is_ok()
}

pub async fn get_last_retained(
    session: &zenoh::Session,
    topic: &str,
) -> Option<crate::sample_buffer::SampleEntry> {
    use zenoh::query::{ConsolidationMode, QueryTarget};

    if !is_recallable_topic(topic) {
        debug!(topic, "stale-recall: not a concrete key (group label or wildcard) — skipped");
        return None;
    }

    let replies = match session
        .get(topic)
        .target(QueryTarget::All)
        .consolidation(ConsolidationMode::None)
        .timeout(std::time::Duration::from_secs(3))
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!(topic, error = %e, "stale-recall: storage get failed");
            return None;
        }
    };

    // Keep only the newest reply (by stored publish timestamp). A concrete
    // key normally yields a single retained value, but be robust to more.
    let mut best: Option<crate::sample_buffer::SampleEntry> = None;
    while let Ok(reply) = replies.recv_async().await {
        let Ok(sample) = reply.result() else { continue };
        let payload = sample.payload().to_bytes();
        // Prefer the sample's stored publish timestamp so "last seen Ns ago"
        // reflects when the data was produced, not when we queried it. Fall
        // back to now() only if the stored sample carries no timestamp.
        let ts_ms = sample
            .timestamp()
            .map(|t| t.get_time().to_duration().as_millis() as u64)
            .unwrap_or_else(now_ms);
        let source_peer =
            source_peer_of(sample).or_else(|| peer_id_from_topic(sample.key_expr().as_str()));
        let key = sample.key_expr().as_str().to_string();
        // Storage replies carry the publisher's encoding too — keep it so a
        // stale-recalled protobuf sample selects its message off the wire.
        let encoding = wire_encoding(sample);
        let entry = crate::sample_buffer::stale_entry(key, ts_ms, source_peer, &payload, encoding);
        let newer = match &best {
            Some(b) => entry.ts_ms >= b.ts_ms,
            None => true,
        };
        if newer {
            best = Some(entry);
        }
    }
    if best.is_none() {
        debug!(topic, "stale-recall: storage returned no retained sample");
    }
    best
}

/// Samples the `**` subscriber callback may queue for the data tail before it
/// starts dropping. At the fleet's observed peak rates this is several seconds
/// of headroom, which covers an ordinary lock wait; anything longer is a stall
/// the router must not feel.
pub const DATA_INGEST_QUEUE_CAPACITY: usize = 16_384;

/// Minimum spacing between the "ingest queue full" warnings.
const BACKPRESSURE_LOG_INTERVAL_MS: u64 = 10_000;

/// Non-blocking hand-off from a Zenoh callback to an async consumer.
///
/// Zenoh runs subscriber callbacks on its receive threads. A callback that
/// blocks — or a bounded FIFO handler whose consumer has stopped reading —
/// holds that thread, the router's pushes to this session back up, and the
/// router eventually logs `Unable to push non droppable network message` and
/// closes the transport after blocking its own receive path for seconds. So
/// the callback never waits: `offer` is a `try_send`, and a full queue drops
/// the item and counts it.
pub struct DropOnFull<T> {
    tx: tokio::sync::mpsc::Sender<T>,
    dropped: Arc<std::sync::atomic::AtomicU64>,
    last_log_ms: std::sync::atomic::AtomicU64,
    what: &'static str,
}

impl<T> DropOnFull<T> {
    pub fn new(
        capacity: usize,
        dropped: Arc<std::sync::atomic::AtomicU64>,
        what: &'static str,
    ) -> (Self, tokio::sync::mpsc::Receiver<T>) {
        let (tx, rx) = tokio::sync::mpsc::channel(capacity);
        (
            Self {
                tx,
                dropped,
                last_log_ms: std::sync::atomic::AtomicU64::new(0),
                what,
            },
            rx,
        )
    }

    /// Queue `item` or drop it. Never blocks. Returns true when queued.
    pub fn offer(&self, item: T) -> bool {
        use std::sync::atomic::Ordering;
        match self.tx.try_send(item) {
            Ok(()) => true,
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                let total = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
                let now = now_ms();
                let last = self.last_log_ms.load(Ordering::Relaxed);
                if now.saturating_sub(last) >= BACKPRESSURE_LOG_INTERVAL_MS
                    && self
                        .last_log_ms
                        .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                        .is_ok()
                {
                    warn!(
                        queue = self.what,
                        dropped_total = total,
                        "ingest queue full — dropping samples so the router is never blocked"
                    );
                }
                false
            }
            // Consumer gone (shutdown). Nothing to count against.
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => false,
        }
    }
}

async fn spawn_data_tail(
    session: zenoh::Session,
    state: SharedState,
    sampling_per_topic_eps: u32,
    topic_blocklist: Vec<String>,
) -> Result<()> {
    // Callback subscriber feeding a bounded queue with drop-on-full. The
    // default handler is a bounded FIFO that BLOCKS Zenoh's receive thread
    // when full, so any stall in the consumer below (a lock held elsewhere)
    // reached the router and closed the transport. Filtering the blocklist in
    // the callback keeps a flood from using up the queue.
    let (queue, mut rx) = DropOnFull::<zenoh::sample::Sample>::new(
        DATA_INGEST_QUEUE_CAPACITY,
        state.ingest_backpressure_drops.clone(),
        "data_tail",
    );
    let blocklist_len = topic_blocklist.len();
    let subscriber = session
        .declare_subscriber("**")
        .callback(move |sample: zenoh::sample::Sample| {
            // Topic blocklist — drop noisy publishers before they touch the
            // topology / flow / sample ring. Matches a whole-key prefix OR a
            // path SEGMENT (so a mid-path marker like
            // `<uuid>/loadtest-wedge/<n>` is caught — load-test floods).
            // Publishers stay on Zenoh (router still routes their samples to
            // other subscribers; audit still records them); panoscope just
            // silences them in its view.
            if panoscope_core::types::topic_matches_blocklist(
                sample.key_expr().as_str(),
                &topic_blocklist,
            ) {
                return;
            }
            queue.offer(sample);
        })
        .await
        .map_err(|e| anyhow::anyhow!("declare_subscriber(**) failed: {e}"))?;
    info!(
        blocklist_len,
        queue_capacity = DATA_INGEST_QUEUE_CAPACITY,
        "data_tail: subscribed to ** (non-blocking callback, drop on full)"
    );

    tokio::spawn(async move {
        // Keep the Session and the subscriber alive for the consumer's
        // lifetime. Subscriber holds only Weak<SessionInner> in Zenoh 1.x, and
        // dropping a callback subscriber undeclares it.
        let _session = session;
        let _subscriber = subscriber;
        let mut buckets = TopicBuckets::new(sampling_per_topic_eps as f64);
        loop {
            match rx.recv().await {
                Some(sample) => {
                    let topic = sample.key_expr().as_str().to_string();
                    let now = Instant::now();
                    let now_millis = now_ms();
                    let payload_bytes = sample.payload().to_bytes();
                    // Self-describing wire encoding (zenoh-capability-adoption
                    // gap 1). Default/unset `zenoh/bytes` → None so it only
                    // carries a signal when the publisher actually set one;
                    // the decoder uses it to pick the protobuf message name
                    // when the portal row has no declared `format`.
                    let encoding = wire_encoding(&sample);

                    // Schema-registry samples (publisher self-registration
                    // under attest/schema/<vendor>/<system>/v1) are
                    // metadata, not data flow. Route into the registry
                    // store, don't count as data samples, don't fall
                    // through to the visualization path.
                    if topic.starts_with("attest/schema/") {
                        if let Ok(json) =
                            serde_json::from_slice::<serde_json::Value>(&payload_bytes)
                        {
                            let mut topology = state.topology.write().await;
                            apply_schema_declaration(&mut topology, &topic, &json, now_millis);
                        }
                        continue;
                    }
                    // Three-tier peer identity:
                    //   1. zenoh SourceInfo (rare — publishers don't attach
                    //      by default in zenoh-1.x)
                    //   2. Schema-declaration lookup — if any peer has
                    //      declared this topic as one of its publishes,
                    //      use that canonical peer_id. Eliminates the
                    //      ghost-duplicate peer that previously appeared
                    //      alongside each real one.
                    //   3. Topic-prefix fallback — last-resort identity
                    //      for publishers we have no schema decl for.
                    let (source_peer, attributed_via_topic) = match source_peer_of(&sample) {
                        Some(p) => (Some(p), false),
                        None => {
                            let from_registry = {
                                let topology = state.topology.read().await;
                                topology.topic_to_publisher.get(&topic).cloned()
                            };
                            match from_registry {
                                Some(p) => (Some(p), false),
                                None => (peer_id_from_topic(&topic), true),
                            }
                        }
                    };

                    // Always update stats + Topology peer-existence, even if
                    // we drop the sample for FlowState — we want to know the
                    // peer is there.
                    {
                        let mut stats = state.stats.write().await;
                        stats.data_samples_seen += 1;
                        stats.last_data_sample_ms = now_millis;
                        if source_peer.is_none() {
                            stats.data_samples_orphaned += 1;
                        } else if attributed_via_topic {
                            stats.data_samples_attributed_via_topic += 1;
                        }
                    }

                    // Push into per-topic ring buffer for click-to-stream UI.
                    // Always captures (not gated on the token bucket) so the
                    // inspector view sees the same samples the operator
                    // sees on the wire.
                    {
                        let mut buf = state.samples.write().await;
                        buf.push(
                            topic.clone(),
                            now_millis,
                            source_peer.clone(),
                            &payload_bytes,
                            encoding,
                        );
                    }
                    // Notify-only wake-up for the LIVE-follow opt-in. Every
                    // sample emits a SampleArrived event (ungated, distinct
                    // from the token-bucketed PublishSampled). Per-connection
                    // watch sets in ws.rs filter so only browsers actively
                    // watching this topic receive it.
                    let _ = state.events.send(WsEvent::SampleArrived {
                        topic: topic.clone(),
                        ts_ms: now_millis,
                    });

                    if let Some(ref peer) = source_peer {
                        let was_new = {
                            let mut topology = state.topology.write().await;
                            let was_new = !topology.peers.contains_key(peer);
                            topology.record_publish_declaration(
                                peer.clone(),
                                topic.clone(),
                                now_millis,
                            );
                            was_new
                        };
                        if was_new {
                            let _ = state.events.send(WsEvent::PeerSeen {
                                peer_id: peer.clone(),
                                ts_ms: now_millis,
                            });
                        }
                    }

                    if buckets.try_consume(&topic, now) {
                        if let Some(peer) = source_peer {
                            let schema_family = state.schema_resolver.resolve(&topic);
                            let ema_eps = {
                                let mut flow = state.flow.write().await;
                                flow.record_event(
                                    peer.clone(),
                                    topic.clone(),
                                    now_millis,
                                    EMA_TAU_SECS,
                                );
                                let edge_key = crate::model::EdgeKey::new(
                                    peer.clone(),
                                    topic.clone(),
                                );
                                if let Some(edge) = flow.edges.get_mut(&edge_key) {
                                    if edge.schema_family.is_none() {
                                        edge.schema_family = schema_family.clone();
                                    }
                                }
                                // The read-path rate: the EMA field itself only
                                // moves when a one-second bucket closes.
                                flow.edges
                                    .get(&edge_key)
                                    .map(|e| e.projected_eps(now_millis, EMA_TAU_SECS))
                                    .unwrap_or(0.0)
                            };
                            {
                                let mut stats = state.stats.write().await;
                                stats.data_samples_recorded += 1;
                            }
                            let _ = state.events.send(WsEvent::PublishSampled {
                                peer_id: peer,
                                topic,
                                ts_ms: now_millis,
                                ema_eps,
                            });
                        }
                    } else {
                        {
                            let mut flow = state.flow.write().await;
                            flow.record_drop(topic.clone());
                        }
                        {
                            let mut stats = state.stats.write().await;
                            stats.data_samples_dropped += 1;
                        }
                        if let Some(peer) = source_peer {
                            let _ = state.events.send(WsEvent::PublishDropped {
                                peer_id: peer,
                                topic,
                                ts_ms: now_millis,
                            });
                        }
                    }
                }
                None => {
                    warn!("data_tail: ingest queue closed — subscriber gone");
                    break;
                }
            }
        }
    });
    Ok(())
}

async fn spawn_admin_tail(
    session: zenoh::Session,
    state: SharedState,
    own_zid: String,
) -> Result<()> {
    // Zenoh admin keyspace is QUERY-only in 1.x (declare_subscriber("@/**")
    // returns Ok but receives zero samples).
    // We poll with session.get() every 5s and process the replies into
    // the topology.
    //
    // Two query targets per cycle:
    //   1. `@/**` — panoscope's own admin namespace. Returns its session-
    //      transport view of its TLS link to the router. Surfaces the
    //      router as a peer, but not the router's other connected peers
    //      (audit-subscriber, synth-vendors via the router-side path).
    //   2. `@/<router_zid>/**` — the router's own admin namespace, once
    //      the router_zid is known from (1). Returns the router's
    //      session-transport view of every connected peer AND — since the
    //      F-152 ACL grant (trial-peer-r-admin-census) — the router's pub/sub
    //      CENSUS: one reply per `@/<router_zid>/router/subscriber/<keyexpr>`
    //      (and `.../publisher/<keyexpr>`), naming the source peers in the
    //      payload. `process_admin_reply` routes those census keys to
    //      `panoscope_core::census::parse_resource_key` and records the SUBSCRIBE side of the
    //      fabric — that's where audit-subscriber + every pure sink finally
    //      surfaces. Skipped on first cycle (router ZID not yet known) and any
    //      cycle where the router peer was filtered out for some reason.
    info!("admin_tail: starting periodic session.get(\"@/**\") + router-admin poll");

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        // Skip the very first tick — interval fires immediately on its
        // first await, which would race with subscriber declaration on
        // the data tail.
        interval.tick().await;
        loop {
            interval.tick().await;
            let now_millis = now_ms();

            // Query 1: own admin namespace
            let own_count =
                run_admin_query(&session, "@/**", &state, &own_zid, now_millis).await;

            // Query 2: the router's pub/sub CENSUS, via the exact selectors the
            // least-privilege ACL grants (`trial-peer-r-admin-census`:
            // `@/*/router/{subscriber,publisher}/**`). The `*` matches the
            // router's ZID, so no router-ZID discovery is needed.
            //
            // This REPLACES the former broad `@/<router_zid>/**` query, which
            // the census ACL correctly DENIES: zenoh authorization allows a
            // query only when an allow rule *includes* it (authorization.rs
            // `policy_decision_point` → `nodes_including`), and the narrow
            // census grant does not include the broad `@/<zid>/**` (nor does
            // `**`, which cannot cross the verbatim `@` chunk). The broad query
            // therefore returns zero census;
            // the exact-selector queries below are self-included by
            // the grant and return the census. `process_admin_reply` routes the
            // replies through `panoscope_core::census::parse_resource_key`.
            // MIRROR-FIRST (the one-crawler contract,
            // zenoh-cardinality-budget-and-lazy-crawler.md §4): try the lazy
            // crawler's published census mirror first — one storage-backed
            // GET on ops/fabric/census/* whose {key, payload} entries are
            // byte-faithful adminspace replies, fed through the SAME
            // process_admin_reply path. Only when no fresh, populated mirror
            // exists (crawler down, or census-blind on its fallback leaf)
            // does the daemon walk the adminspace itself. max_age = 3x the
            // crawler's 60 s interval.
            let mut census_count =
                run_census_mirror(&session, &state, &own_zid, now_millis).await;
            if census_count == 0 {
                for selector in panoscope_core::census::CENSUS_SELECTORS {
                    census_count +=
                        run_admin_query(&session, selector, &state, &own_zid, now_millis).await;
                }
            }

            let sample_count = own_count + census_count;
            if sample_count > 0 {
                let mut stats = state.stats.write().await;
                stats.admin_samples_seen += sample_count;
                stats.last_admin_sample_ms = now_millis;
            }
        }
    });
    Ok(())
}

/// Read the lazy crawler's census mirror (ops/fabric/census/*) and feed each
/// byte-faithful {key, payload} entry through `process_admin_reply` — exactly
/// the path a direct adminspace reply takes. Returns the entry count from
/// FRESH (sampled_at_ms within 180 s) documents only; 0 = no authoritative
/// mirror (stale, absent, or the crawler's census-blind fallback identity
/// publishing empty) — the caller falls back to the direct walk. Never an
/// error: mirror trouble degrades to today's behavior, not to a gap.
async fn run_census_mirror(
    session: &zenoh::Session,
    state: &SharedState,
    own_zid: &str,
    now_millis: u64,
) -> u64 {
    const MIRROR_MAX_AGE_MS: i64 = 180_000;
    let Ok(replies) = session
        .get(panoscope_core::census::CENSUS_MIRROR_SELECTOR)
        .timeout(std::time::Duration::from_secs(3))
        .await
    else {
        return 0;
    };
    // Drain every reply before touching a lock (see `drain_replies`).
    let docs = drain_replies(&replies).await;
    let mut count: u64 = 0;
    for (_, bytes) in docs {
        let Ok(doc) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let sampled_at_ms = doc.get("sampled_at_ms").and_then(|v| v.as_i64()).unwrap_or(0);
        let age_ms = (now_millis as i64).saturating_sub(sampled_at_ms);
        if !(0..=MIRROR_MAX_AGE_MS).contains(&age_ms) {
            continue; // stale mirror is never trusted
        }
        let Some(entries) = doc.get("entries").and_then(|v| v.as_array()) else {
            continue;
        };
        for e in entries {
            let (Some(key), Some(payload)) =
                (e.get("key").and_then(|k| k.as_str()), e.get("payload"))
            else {
                continue;
            };
            let payload_bytes = serde_json::to_vec(payload).unwrap_or_default();
            process_admin_reply(state, key, &payload_bytes, own_zid, now_millis).await;
            count += 1;
        }
    }
    count
}

/// Single admin query → drain replies → upsert peers. Returns the
/// sample count so the caller can bump the stats counter once per
/// cycle across multiple queries.
async fn run_admin_query(
    session: &zenoh::Session,
    query: &str,
    state: &SharedState,
    own_zid: &str,
    now_millis: u64,
) -> u64 {
    let replies = match session.get(query).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, query = %query, "admin_tail: query failed");
            return 0;
        }
    };
    let mut sample_count: u64 = 0;
    for (key, payload_bytes) in drain_replies(&replies).await {
        sample_count += 1;
        process_admin_reply(state, &key, &payload_bytes, own_zid, now_millis).await;
    }
    sample_count
}

/// Read every reply of a `get` into memory before any of them is applied.
///
/// A `get`'s replies arrive through a bounded channel filled on Zenoh's
/// receive thread. Applying each reply as it is read means awaiting the
/// topology write lock between reads; while something else holds that lock
/// the channel fills and the receive thread waits on it, which is the same
/// router-blocking stall the data tail's drop-on-full queue prevents. Replies
/// to these queries are bounded (census, schema declarations), so holding
/// them briefly is cheap.
async fn drain_replies(
    replies: &zenoh::handlers::FifoChannelHandler<zenoh::query::Reply>,
) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    while let Ok(reply) = replies.recv_async().await {
        let Ok(sample) = reply.result() else { continue };
        out.push((
            sample.key_expr().as_str().to_string(),
            sample.payload().to_bytes().into_owned(),
        ));
    }
    out
}

/// Process a single admin-namespace reply: parse the key + payload,
/// upsert the peer (with NodeKind / locator), and walk any embedded
/// subscriber / publisher arrays in the payload.
async fn process_admin_reply(
    state: &SharedState,
    key: &str,
    payload_bytes: &[u8],
    own_zid: &str,
    now_millis: u64,
) {
    {
        let payload_preview = std::str::from_utf8(payload_bytes)
            .map(|s| s.chars().take(300).collect::<String>())
            .unwrap_or_else(|_| format!("<{} bytes non-utf8>", payload_bytes.len()));
        debug!(key = %key, payload = %payload_preview, "admin_tail: reply");
    }

    let json: Option<serde_json::Value> = serde_json::from_slice(payload_bytes).ok();

    // Router pub/sub CENSUS shape.
    //
    // The router's adminspace lists every subscription + publication routing
    // through it as ONE reply per resource, keyed with the subscribed/published
    // key-expression as the admin-key SUFFIX and carrying the source peer ZIDs
    // in the payload:
    //   key:     @/<router_zid>/router/subscriber/<subscribed_keyexpr>
    //   payload: {"routers":[zid…],"peers":[zid…],"clients":[zid…]}
    // (zenoh-1.9.0 net/runtime/adminspace.rs `subscribers_data` +
    // routing/hat/mod.rs `Sources`; the crate's tests/adminspace.rs pins the
    // key shape `@/<zid>/router/subscriber/some/key`). This is what surfaces
    // pure subscribers (audit-subscriber on `**`, non-publishing sinks) that
    // leave no data-tail or schema-decl trace — the SUBSCRIBE side of the
    // fabric, from the authoritative router view, no self-declaration.
    //
    // Handled here and returned early: the generic session-transport /
    // parse_admin_key path below would mis-read `.../router/subscriber/<ke>`
    // as merely "the router peer" (second segment == "router") and drop both
    // the keyexpr suffix and the subscriber identity.
    if let Some((role, owner_zid, keyexpr)) = parse_resource_key(key) {
        let mut seen_new: Vec<String> = Vec::new();
        let sources = json.as_ref().map(census_sources).unwrap_or_default();
        for src_zid in sources {
            // Skip our own subscriber declaration (panoscope subscribes `**`)
            // and the router's own in-process storage-manager subscriptions
            // (every `<uuid>/**` storage prefix surfaces with the router's ZID
            // in `clients`) — neither is an external data consumer.
            if src_zid == own_zid || src_zid == owner_zid {
                continue;
            }
            let mut topology = state.topology.write().await;
            let was_new = !topology.peers.contains_key(&src_zid);
            match role {
                ResourceRole::Subscriber => {
                    // A subscriber may also publish; upsert_admin_peer only sets
                    // NodeKind::Subscriber when nothing richer has classified it
                    // (schema-decl / Router both win), so this never downgrades a
                    // known peer — it just gives pure sinks a kind + a node.
                    topology.upsert_admin_peer(
                        src_zid.clone(),
                        Some(NodeKind::Subscriber),
                        None,
                        now_millis,
                    );
                    topology.record_subscribe_declaration(
                        src_zid.clone(),
                        keyexpr.clone(),
                        now_millis,
                    );
                }
                ResourceRole::Publisher => {
                    topology.record_publish_declaration(
                        src_zid.clone(),
                        keyexpr.clone(),
                        now_millis,
                    );
                }
            }
            if was_new {
                seen_new.push(src_zid);
            }
        }
        for zid in seen_new {
            let _ = state.events.send(WsEvent::PeerSeen {
                peer_id: zid,
                ts_ms: now_millis,
            });
        }
        return;
    }

    // Two recognized admin-key shapes:
    //
    //   1. Session-transport (the actual Zenoh-1.x shape):
    //         `@/<owner_zid>/session/transport/unicast/<peer_zid>`
    //         payload: `{"zid":"<peer_zid>","whatami":"router|peer",...}`
    //      The key's first segment is the QUERYABLE replier (panoscope
    //      itself when we query `@/**`, the ROUTER when we query
    //      `@/<router_zid>/**`); the peer at the other end of the
    //      session is the segment after `/unicast/`. NodeKind comes
    //      from `whatami` in the payload.
    //
    //   2. Legacy `@/<zid>/router/...` / `@/router/<zid>/...` shape
    //      (forward-compat with non-1.x deployments or future router-
    //      admin federation; no-op against the 1.x shape).
    //
    // For both `.../unicast/<peer_zid>` and `.../unicast/<peer_zid>/link/<id>`
    // shapes, take the segment immediately after `/unicast/` — `rsplit`
    // would pick up the numeric link-id on /link/ leaves and that
    // accidentally passes `is_plausible_zid` because digits are
    // hex-digits.
    let session_peer_zid = key
        .split("/session/transport/unicast/")
        .nth(1)
        .and_then(|after| after.split('/').next())
        .filter(|s| is_plausible_zid(s))
        .map(|s| s.to_string());

    // `kind` is None when the sample carries no whatami (the /link/
    // leaves of the session-transport tree, for instance) — let
    // upsert_admin_peer leave node_kind alone in that case so a later
    // sample with whatami can promote it. Same effect for legacy
    // router/peer parse when no class info is on the wire.
    let (zid, kind) = if let Some(peer_zid) = session_peer_zid {
        if peer_zid == own_zid {
            return;
        }
        let kind = json
            .as_ref()
            .and_then(|j| j.get("whatami"))
            .and_then(|v| v.as_str())
            .and_then(|w| match w {
                "router" => Some(NodeKind::Router),
                "peer" => Some(NodeKind::Peer),
                "client" => Some(NodeKind::Peer),
                _ => None,
            });
        (peer_zid, kind)
    } else {
        let parsed = match parse_admin_key(key) {
            Some(p) => p,
            None => return,
        };
        let (zid, kind) = match parsed {
            AdminKey::Local => return,
            AdminKey::Router { zid } => (zid, Some(NodeKind::Router)),
            AdminKey::Peer { zid } => (zid, Some(NodeKind::Peer)),
        };
        if zid == own_zid {
            return;
        }
        (zid, kind)
    };

    let locator = json
        .as_ref()
        .and_then(|j| {
            // Session-transport payloads carry a flat
            // `{"src":"tls/<ip>:<port>","dst":...}` on the /link/
            // leaves. Prefer src when set; fall back to the generic
            // extractor for other shapes.
            j.get("src").and_then(|v| v.as_str()).map(|s| s.to_string())
        })
        .or_else(|| json.as_ref().and_then(extract_locator));

    let was_new = {
        let mut topology = state.topology.write().await;
        let was_new = !topology.peers.contains_key(&zid);
        topology.upsert_admin_peer(zid.clone(), kind, locator.clone(), now_millis);
        was_new
    };
    if was_new {
        let _ = state.events.send(WsEvent::PeerSeen {
            peer_id: zid.clone(),
            ts_ms: now_millis,
        });
    }

    // Best-effort: parse declared subscribers + publishers. The
    // zenoh-1.x admin response shape isn't strictly documented and
    // varies by sub-path; we accept several common JSON layouts and
    // skip silently if none match.
    let Some(json) = json else { return };

    // Subscribers — admin payloads may carry the subscriber peer's own
    // ZID alongside its key_expr; when present, upsert that ZID as a
    // `Subscriber`-kind node so pure subscribers (audit-subscriber et
    // al.) surface as their own peer rather than being attributed to
    // the router.
    let sub_arrays = [
        json.get("subscribers"),
        json.pointer("/declarations/subscribers"),
        json.pointer("/local/subscribers"),
    ];
    for arr in sub_arrays.into_iter().flatten() {
        if let Some(items) = arr.as_array() {
            for item in items {
                let ke = item
                    .get("key_expr")
                    .or_else(|| item.get("keyexpr"))
                    .or_else(|| item.get("res"))
                    .and_then(|v| v.as_str());
                let sub_zid = item
                    .get("zid")
                    .or_else(|| item.get("peer_id"))
                    .and_then(|v| v.as_str())
                    .filter(|s| is_plausible_zid(s) && *s != own_zid)
                    .map(|s| s.to_string());
                let Some(ke) = ke else { continue };
                let target_zid = sub_zid.unwrap_or_else(|| zid.clone());
                let mut topology = state.topology.write().await;
                if target_zid != zid {
                    topology.upsert_admin_peer(
                        target_zid.clone(),
                        Some(NodeKind::Subscriber),
                        None,
                        now_millis,
                    );
                }
                topology.record_subscribe_declaration(
                    target_zid,
                    ke.to_string(),
                    now_millis,
                );
            }
        }
    }

    // Publishers — same shape, but no Subscriber upsert.
    let pub_arrays = [
        json.get("publishers"),
        json.pointer("/declarations/publishers"),
        json.pointer("/local/publishers"),
    ];
    for arr in pub_arrays.into_iter().flatten() {
        if let Some(items) = arr.as_array() {
            for item in items {
                let ke = item
                    .get("key_expr")
                    .or_else(|| item.get("keyexpr"))
                    .or_else(|| item.get("res"))
                    .and_then(|v| v.as_str());
                let pub_zid = item
                    .get("zid")
                    .or_else(|| item.get("peer_id"))
                    .and_then(|v| v.as_str())
                    .filter(|s| is_plausible_zid(s) && *s != own_zid)
                    .map(|s| s.to_string());
                let Some(ke) = ke else { continue };
                let target_zid = pub_zid.unwrap_or_else(|| zid.clone());
                let mut topology = state.topology.write().await;
                topology.record_publish_declaration(
                    target_zid,
                    ke.to_string(),
                    now_millis,
                );
            }
        }
    }
}

/// Extract the publisher ZID from a Zenoh Sample. Returns `None` if
/// the source info isn't populated (e.g. local-only test publishers).
/// Apply a synth-vendor schema-declaration payload into the Topology.
/// The peer's identity is derived from the publishes[] field (first
/// publish key's vendor/system prefix); declared subscribers turn
/// into Topology subscribe declarations so the renderer can draw
/// publisher→subscriber edges. Schemas land in the `attest/schema/`
/// keyspace per the design doc — Panoscope is a passive reader.
/// Schema-decl staleness ceiling. Synth-vendors re-publish their decl
/// every 30s; anything older than 3× that — without a refresh — is
/// almost certainly residue replayed from rocksdb storage after a
/// namespace migration (e.g. the caracal/community → notional collapse
///  left 22 stale decls hanging around the next morning).
/// Filtering at apply-time means panoscope self-cleans on restart
/// without operator-driven storage surgery.
pub const SCHEMA_DECL_STALE_MS: u64 = 90_000;

fn apply_schema_declaration(
    topology: &mut crate::model::Topology,
    key: &str,
    json: &serde_json::Value,
    now_millis: u64,
) {
    // Staleness gate: drop decls whose `published_at_ms` is older than
    // SCHEMA_DECL_STALE_MS. Live re-publishes (30s cadence) always pass;
    // storage-replayed residue from a defunct namespace gets dropped at
    // the boundary. Decls without `published_at_ms` (older synth-vendor
    // versions; partner peers that didn't include the field) pass
    // through — staleness is opt-in via the field, not enforced.
    if let Some(pub_at) = json.get("published_at_ms").and_then(|v| v.as_u64()) {
        if now_millis > pub_at && now_millis - pub_at > SCHEMA_DECL_STALE_MS {
            tracing::debug!(
                key = %key,
                age_ms = now_millis - pub_at,
                threshold_ms = SCHEMA_DECL_STALE_MS,
                "skipping stale schema declaration"
            );
            return;
        }
    }

    // Derive peer-id: prefer json.peer_id, else derive from key path
    // (attest/schema/<vendor>/<system>/v1 → "<vendor>/<system>").
    let peer_id = json
        .get("peer_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            let parts: Vec<&str> = key.strip_prefix("attest/schema/")?.split('/').collect();
            if parts.len() >= 2 {
                Some(format!("{}/{}", parts[0], parts[1]))
            } else {
                None
            }
        });
    let Some(peer_id) = peer_id else { return };

    // Always ensure the peer exists in topology — even if no publish/sub
    // data flows yet, we want to render the node.
    {
        let p = topology.upsert_peer(peer_id.clone(), now_millis);
        // Capture logical-vendor + site + role_class so panoscope can
        // cluster by friendly name (decoupled from cert-bound vendor).
        if let Some(v) = json.get("vendor").and_then(|v| v.as_str()) {
            if !v.is_empty() {
                p.vendor = Some(v.to_string());
            }
        }
        if let Some(dn) = json.get("display_name").and_then(|v| v.as_str()) {
            if !dn.is_empty() {
                p.display_name = Some(dn.to_string());
            }
        }
        if let Some(s) = json.get("site").and_then(|v| v.as_str()) {
            if !s.is_empty() {
                p.site = Some(s.to_string());
            }
        }
        if let Some(rc) = json.get("role_class").and_then(|v| v.as_str()) {
            if !rc.is_empty() {
                p.role_class = Some(rc.to_string());
            }
        }
        if let Some(fmt) = json.get("format").and_then(|v| v.as_str()) {
            if !fmt.is_empty() {
                p.format_hint = Some(fmt.to_string());
            }
        }
        if let Some(syn) = json.get("synthetic").and_then(|v| v.as_bool()) {
            p.synthetic = Some(syn);
        }
        // Explicit infrastructure self-declaration (highest-priority
        // signal in PeerInfo::node_class — lets a participant opt in/out
        // of the infra class). Optional; most peers omit it and get
        // classified by the derived heuristics instead.
        if let Some(inf) = json.get("infra").and_then(|v| v.as_bool()) {
            p.infra = Some(inf);
        }
        // Schema-decl always wins for attest_source: an admin
        // sample that previously tagged this peer "admin" gets upgraded
        // here. node_kind set by admin is preserved (it's substrate
        // info that schema-decl never carries).
        p.attest_source = Some("schema_decl".to_string());
    }

    if let Some(arr) = json.get("publishes").and_then(|v| v.as_array()) {
        for v in arr {
            if let Some(ke) = v.as_str() {
                topology.record_publish_declaration(
                    peer_id.clone(),
                    ke.to_string(),
                    now_millis,
                );
            }
        }
    }
    if let Some(arr) = json.get("subscribes").and_then(|v| v.as_array()) {
        for v in arr {
            if let Some(ke) = v.as_str() {
                topology.record_subscribe_declaration(
                    peer_id.clone(),
                    ke.to_string(),
                    now_millis,
                );
            }
        }
    }
}

/// Ingest a single observation from the audit log into the topology
/// AND the flow state.
///
/// Mirrors the data_tail's source-attribution + publish-declaration +
/// flow-event write path, but reads from a structured audit record
/// instead of a live `zenoh::sample::Sample`. Used by the MinIO
/// replay-on-boot path ([`crate::minio_replay`]) to re-hydrate the
/// in-RAM peer + flow registries from the audit-subscriber's NDJSON
/// history after a container restart wipes the prior session's state.
///
/// Both writes matter independently:
///   - `Topology.record_publish_declaration` populates the peer's
///     `publishes[]` list — visible in the side panel's "publishes"
///     drill-down.
///   - `FlowState.record_event` materializes a `(source_peer, topic)`
///     edge — drives the topic-virtual diamond rendering in the
///     topology view (`snapshot_view::shared_snapshot_build`). Without
///     this second call, replayed publishers appear as singleton
///     circles with no topic-diamond children, which was the
///     regression that survived the first replay
///     fix until this follow-up landed.
///
/// Identity preference (matches data_tail):
///   1. `src_zid` if non-empty (audit-subscriber preserves whatever
///      Zenoh's `SourceInfo` carried at the time the sample was seen)
///   2. topic-prefix fallback (`<vendor>/<system>`) when src_zid is empty
///
/// **`src_zid` IS ASSERTED BY THE PUBLISHER AND VERIFIED BY NOTHING.**
/// It is `ext_sinfo` off the wire; a forwarding hop rewrites only
/// `wire_expr` and `ext_nodeid`, so it crosses a federation boundary
/// verbatim, and no code in zenoh compares it to the transport peer. A
/// peered party can claim any ZID, including one of ours. That is tolerable
/// inside one trust domain, where every publisher is ours; it is NOT a basis
/// for attributing anything that arrived across a trust boundary. For that,
/// use the ingress key prefix and the peering certificate CN, which are the
/// only authenticated facts available.
/// See `docs/design/cross-sovereign-federation.md` §6 and ADR 1152.
///
/// Marks the peer with `attest_source = "audit_replay"` only if no
/// stronger source has tagged it already. A later live sample or
/// schema-decl can promote the attest_source value.
pub fn apply_audit_event(
    topology: &mut crate::model::Topology,
    flow: &mut crate::model::FlowState,
    vendor: &str,
    key_expr: &str,
    src_zid: &str,
    now_ms: u64,
) {
    let peer_id = if !src_zid.is_empty() {
        Some(src_zid.to_string())
    } else {
        peer_id_from_topic(key_expr)
    };
    let Some(peer_id) = peer_id else { return };

    {
        let p = topology.upsert_peer(peer_id.clone(), now_ms);
        if !vendor.is_empty() && p.vendor.is_none() {
            p.vendor = Some(vendor.to_string());
        }
        // Don't overwrite a stronger attest source (admin / schema_decl).
        if p.attest_source.is_none() {
            p.attest_source = Some("audit_replay".to_string());
        }
    }
    topology.record_publish_declaration(peer_id.clone(), key_expr.to_string(), now_ms);
    // Synthesize a FlowState edge so the renderer materializes the
    // topic-virtual diamond between publisher and (any) subscriber —
    // same shape the live data_tail's `flow.record_event` produces.
    // Uses the EMA decay constant from the runtime so a long-idle
    // replayed edge decays to ~0 eps in the snapshot view (the
    // history is read-only; we're not pretending it's live traffic).
    flow.record_event(peer_id, key_expr.to_string(), now_ms, EMA_TAU_SECS);
}

fn source_peer_of(sample: &zenoh::sample::Sample) -> Option<PeerId> {
    sample
        .source_info()
        .map(|si| si.source_id().zid().to_string())
}

/// The sample's stringified wire `Encoding`, or `None` for the default
/// unset `zenoh/bytes`. Keeping only meaningful encodings means a
/// publisher that sets nothing leaves no signal, and the decoder's
/// portal/first-message resolution is unchanged for it.
fn wire_encoding(sample: &zenoh::sample::Sample) -> Option<String> {
    let enc = sample.encoding().to_string();
    if enc.is_empty() || enc == "zenoh/bytes" {
        None
    } else {
        Some(enc)
    }
}

/// Derive a stable peer identity from the key-expression prefix when the
/// publisher didn't attach SourceInfo. The trial-fabric naming convention
/// is `<vendor>/<system>/<arbitrary>/v<N>`, so taking the first two
/// segments gives one node per publishing process
/// (e.g. `caracal/tracks-demo`). Single-segment topics or empty keys
/// degrade gracefully — single segment becomes the peer id, empty stays
/// orphaned and bumps the loud counter.
fn peer_id_from_topic(topic: &str) -> Option<PeerId> {
    let trimmed = topic.trim_start_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let mut segs = trimmed.split('/').filter(|s| !s.is_empty());
    let first = segs.next()?;
    // Under the ADR 1025 slot-pool model the leading segment is a 32-hex slot
    // anchor (`sha256(salt:slot_idx)[:32]`) that is the publisher's WHOLE
    // identity — cert CN, ACL slot, and (once pinned) the Zenoh ZID. Return it
    // alone so the topic-derived peer id converges with the ZID path instead of
    // splitting one publisher into two nodes (`<anchor>` vs `<anchor>/<vendor>`).
    // Legacy `<vendor>/<system>` topics (no uuid anchor) keep the pair.
    if panoscope_core::alias::is_anchor_shaped(first) {
        return Some(first.to_string());
    }
    match segs.next() {
        Some(second) => Some(format!("{first}/{second}")),
        None => Some(first.to_string()),
    }
}

/// Classification of an admin-namespace key shape.
///
/// Zenoh 1.x exposes admin paths under `@/router/<zid>/...` for routers
/// and `@/peer/<zid>/...` for peers. An older / pre-1.x shape put the
/// zid first as `@/<zid>/router/...`; we accept both so panoscope works
/// against either layout. The router/peer subscriber declarations sit
/// under `@/router/<rid>/subscriber/<sub_id>` (and analogous), which we
/// pick up downstream by inspecting payload contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminKey {
    /// Router self-admin (e.g. `@/router/<zid>` or `@/router/<zid>/sub`).
    Router { zid: PeerId },
    /// Peer self-admin (e.g. `@/peer/<zid>` or `@/peer/<zid>/sub`).
    Peer { zid: PeerId },
    /// Local-node admin (panoscope's own session — we skip these).
    Local,
}

/// Parse the substrate-role + ZID from an admin-namespace key.
///
/// Recognized shapes:
///   - `@/router/<zid>[/...]` — Router(zid)
///   - `@/peer/<zid>[/...]`   — Peer(zid)
///   - `@/<zid>/router[/...]` — Router(zid) (legacy / pre-1.x shape)
///   - `@/<zid>/peer[/...]`   — Peer(zid)   (legacy)
///   - `@/local/...`          — Local (skipped)
///
/// Returns `None` for shapes we don't recognize.
pub fn parse_admin_key(key: &str) -> Option<AdminKey> {
    let rest = key.strip_prefix("@/")?;
    let mut segs = rest.split('/');
    let first = segs.next()?;
    let second = segs.next();
    match first {
        "local" => Some(AdminKey::Local),
        "router" => {
            let zid = second?;
            if is_plausible_zid(zid) {
                Some(AdminKey::Router { zid: zid.to_string() })
            } else {
                None
            }
        }
        "peer" => {
            let zid = second?;
            if is_plausible_zid(zid) {
                Some(AdminKey::Peer { zid: zid.to_string() })
            } else {
                None
            }
        }
        _ if is_plausible_zid(first) => {
            // Zenoh-1.x self-admin shape: `@/<zid>/...`. The router (and
            // peers with admin enabled) self-publish under this prefix.
            // Inspect the second segment for an explicit role hint —
            // `router` / `peer` — but DEFAULT TO ROUTER when absent.
            //
            // Why default Router: a router emits dozens of self-admin
            // keys under `@/<router_zid>/` (`/version`, `/session/
            // transport/multicast`, etc.) plus exactly one `/router/`
            // sample. Whichever arrives first wins under PR #322's
            // "Router beats Peer on conflict" priority. With the prior
            // `_ => Peer` default, any non-`/router/` sample arriving
            // first would lock the classification to Peer, and the
            // single `/router/` sample couldn't promote it. Result: the
            // Zenoh broker rendered as a mystery "unknown peer" on the
            // Data-COP — the very ghost the F-147 work was supposed to
            // eliminate. Default Router is honest: anything self-
            // publishing admin under its own zid in Zenoh-1.x is
            // overwhelmingly a router; client-mode peers don't expose
            // admin this way. Authoritative classification still comes
            // from the payload's `whatami` field in admin_tail (which
            // overrides the parser's guess when present).
            //
            // Legacy shape: `@/<zid>/<role>/...`. Inspect the second
            // segment for "router" / "peer" — default to Peer if absent.
            match second {
                Some("router") => Some(AdminKey::Router { zid: first.to_string() }),
                Some("peer") => Some(AdminKey::Peer { zid: first.to_string() }),
                _ => Some(AdminKey::Router { zid: first.to_string() }),
            }
        }
        _ => None,
    }
}

// The pub/sub census parse + source-attribution logic now lives in the shared
// `panoscope_core::census` module (so `goat top`/`goat who`/panoscope share one
// primitive). This file consumes `parse_resource_key` + `census_sources` in the
// admin-tail census branch above.

/// Compat wrapper for the prior public API — returns just the ZID and
/// drops the role distinction. Retained so external callers (tests) don't
/// break; new code should use [`parse_admin_key`] instead.
#[allow(dead_code)]
pub fn parse_admin_peer_zid(key: &str) -> Option<PeerId> {
    match parse_admin_key(key)? {
        AdminKey::Router { zid } | AdminKey::Peer { zid } => Some(zid),
        AdminKey::Local => None,
    }
}

/// A "plausible" Zenoh ZID is a hex string of 8-32 chars. Conservative
/// matcher — admin keyspaces sometimes also include `version` etc. as
/// the second segment which we reject.
fn is_plausible_zid(s: &str) -> bool {
    let len = s.len();
    (8..=32).contains(&len) && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Pull a locator string (e.g. `tcp/198.51.100.7:7447`) out of an
/// admin payload. Tries a handful of common JSON locations; returns
/// `None` if none match.
fn extract_locator(json: &serde_json::Value) -> Option<String> {
    // Common spots: `locators[0]`, `link[0].dst`, `transport.locator`.
    if let Some(arr) = json.get("locators").and_then(|v| v.as_array()) {
        if let Some(s) = arr.first().and_then(|v| v.as_str()) {
            return Some(s.to_string());
        }
    }
    if let Some(arr) = json.get("links").and_then(|v| v.as_array()) {
        if let Some(s) = arr
            .first()
            .and_then(|item| item.get("dst").or_else(|| item.get("src")))
            .and_then(|v| v.as_str())
        {
            return Some(s.to_string());
        }
    }
    if let Some(s) = json
        .pointer("/transport/locator")
        .and_then(|v| v.as_str())
    {
        return Some(s.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── stale-recall key guard ──────────────────────────────────────────────

    #[test]
    fn group_labels_and_wildcards_are_not_recallable() {
        for label in [
            "aaaa/aircraft/* (55 topics)",
            "aaaa/imagery/v1/stream/*/…",
            "aaaa/** (3 topics)",
            "aaaa/**",
            "aaaa/*/v1",
            "",
            "a//b",
            "a/b/",
        ] {
            assert!(!is_recallable_topic(label), "{label:?} must not reach storage");
        }
        for key in ["aaaa/aircraft/0001/v1", "ops/fabric/health/router", "notional/tarsier/v1"] {
            assert!(is_recallable_topic(key), "{key:?} is a concrete key");
        }
    }

    // ── ingest backpressure (drop on full, never block) ─────────────────────

    /// A consumer that has stopped reading must never make `offer` wait: the
    /// queue takes what fits, the rest is dropped and counted.
    #[test]
    fn a_full_ingest_queue_drops_and_counts_instead_of_blocking() {
        let dropped = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (queue, mut rx) = DropOnFull::<u32>::new(2, dropped.clone(), "test");
        let started = std::time::Instant::now();
        let accepted: Vec<bool> = (0..5).map(|i| queue.offer(i)).collect();
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
        assert_eq!(accepted, vec![true, true, false, false, false]);
        assert_eq!(dropped.load(std::sync::atomic::Ordering::Relaxed), 3);
        // What was queued is delivered in order; the dropped items are gone.
        assert_eq!(rx.try_recv().ok(), Some(0));
        assert_eq!(rx.try_recv().ok(), Some(1));
        assert!(rx.try_recv().is_err());
        // Once drained, the queue accepts again.
        assert!(queue.offer(9));
        assert_eq!(dropped.load(std::sync::atomic::Ordering::Relaxed), 3);
    }

    /// Offers from a plain OS thread (the shape of a Zenoh receive thread)
    /// while the async consumer is stalled complete immediately.
    #[test]
    fn offers_from_a_foreign_thread_never_wait_on_a_stalled_consumer() {
        let dropped = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (queue, _rx) = DropOnFull::<u64>::new(8, dropped.clone(), "test");
        let handle = std::thread::spawn(move || {
            for i in 0..10_000 {
                queue.offer(i);
            }
        });
        handle.join().expect("offer thread finished");
        assert_eq!(dropped.load(std::sync::atomic::Ordering::Relaxed), 10_000 - 8);
    }

    /// A closed consumer (shutdown) is not backpressure and is not counted.
    #[test]
    fn a_closed_queue_is_not_counted_as_backpressure() {
        let dropped = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (queue, rx) = DropOnFull::<u8>::new(1, dropped.clone(), "test");
        drop(rx);
        assert!(!queue.offer(1));
        assert_eq!(dropped.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    // ── client-triggered fabric-probe budget ────────────────────────────────

    #[test]
    fn fabric_probe_budget_is_bounded_and_shared() {
        // The whole point: N concurrent viewers cannot put more than
        // MAX_INFLIGHT_FABRIC_PROBES queries at the router, no matter how many
        // handlers ask at once. One process-wide budget, not per-request.
        let state = SharedState::new(Arc::new(crate::schema::FamilyMapResolver::default()));
        let mut held = Vec::new();
        for _ in 0..MAX_INFLIGHT_FABRIC_PROBES {
            held.push(
                state
                    .fabric_probes
                    .clone()
                    .try_acquire_owned()
                    .expect("budget should admit up to the cap"),
            );
        }
        assert!(
            state.fabric_probes.clone().try_acquire_owned().is_err(),
            "the {MAX_INFLIGHT_FABRIC_PROBES}th+1 concurrent probe must be REFUSED — \
             if this admits, a browser can drive unbounded storage queries at the router"
        );
    }

    #[test]
    fn fabric_probe_budget_recovers_when_probes_finish() {
        // Saturation must be transient. A permit leak would silently and
        // permanently disable retrievability + stale-recall across the whole
        // surface, which reads to an operator as "storage has nothing" —
        // exactly the false-absence this endpoint is written to avoid.
        let state = SharedState::new(Arc::new(crate::schema::FamilyMapResolver::default()));
        {
            let mut held = Vec::new();
            for _ in 0..MAX_INFLIGHT_FABRIC_PROBES {
                held.push(state.fabric_probes.clone().try_acquire_owned().unwrap());
            }
            assert!(state.fabric_probes.clone().try_acquire_owned().is_err());
            // held drops here
        }
        assert!(
            state.fabric_probes.clone().try_acquire_owned().is_ok(),
            "permits must return to the budget when probes complete"
        );
    }

    #[test]
    fn parse_admin_peer_zid_handles_legacy_router_path() {
        // Legacy / pre-1.x shape: zid first.
        assert_eq!(
            parse_admin_peer_zid("@/aabbccddeeff0011/router/linkstate"),
            Some("aabbccddeeff0011".to_string())
        );
    }

    #[test]
    fn parse_admin_peer_zid_handles_1x_router_path() {
        // 1.x shape: `router` first, then zid.
        assert_eq!(
            parse_admin_peer_zid("@/router/aabbccddeeff0011"),
            Some("aabbccddeeff0011".to_string())
        );
        assert_eq!(
            parse_admin_peer_zid("@/router/aabbccddeeff0011/subscriber/x"),
            Some("aabbccddeeff0011".to_string())
        );
    }

    #[test]
    fn parse_admin_peer_zid_handles_1x_peer_path() {
        assert_eq!(
            parse_admin_peer_zid("@/peer/ccccddddeeeeffff"),
            Some("ccccddddeeeeffff".to_string())
        );
    }

    #[test]
    fn parse_admin_key_classifies_router_vs_peer() {
        assert_eq!(
            parse_admin_key("@/router/aabbccddeeff0011"),
            Some(AdminKey::Router { zid: "aabbccddeeff0011".to_string() })
        );
        assert_eq!(
            parse_admin_key("@/peer/aabbccddeeff0011"),
            Some(AdminKey::Peer { zid: "aabbccddeeff0011".to_string() })
        );
        assert_eq!(
            parse_admin_key("@/aabbccddeeff0011/router/linkstate"),
            Some(AdminKey::Router { zid: "aabbccddeeff0011".to_string() })
        );
        // Legacy shape with explicit `peer` second segment.
        assert_eq!(
            parse_admin_key("@/aabbccddeeff0011/peer/linkstate"),
            Some(AdminKey::Peer { zid: "aabbccddeeff0011".to_string() })
        );
        // Legacy shape with non-role second segment defaults to Router
        // (was Peer earlier; see comments in parse_admin_key for
        // the reason — the router self-publishes many keys
        // under `@/<zid>/<non-role-keyword>/...` and a Peer
        // default mis-classifies the broker as a peer,
        // producing a mystery unknown node on the Data-COP). The new
        // default Router is the honest guess for Zenoh-1.x self-admin;
        // payload `whatami` in admin_tail overrides authoritatively.
        assert_eq!(
            parse_admin_key("@/aabbccddeeff0011/version"),
            Some(AdminKey::Router { zid: "aabbccddeeff0011".to_string() })
        );
        assert_eq!(
            parse_admin_key("@/aabbccddeeff0011/session/transport/multicast"),
            Some(AdminKey::Router { zid: "aabbccddeeff0011".to_string() })
        );
        assert_eq!(parse_admin_key("@/local/something"), Some(AdminKey::Local));
    }

    #[test]
    fn parse_admin_peer_zid_rejects_non_hex_segment() {
        assert_eq!(parse_admin_peer_zid("@/version/info"), None);
        assert_eq!(parse_admin_peer_zid("@/short/info"), None); // too short
        assert_eq!(parse_admin_peer_zid("@/router/notazid"), None);
    }

    #[test]
    fn parse_admin_peer_zid_rejects_non_admin_keys() {
        assert_eq!(parse_admin_peer_zid("vendor-b/sensorgrid/raw"), None);
        assert_eq!(parse_admin_peer_zid(""), None);
    }

    #[test]
    fn extract_locator_finds_locators_array() {
        let j: serde_json::Value =
            serde_json::from_str(r#"{"locators":["tcp/198.51.100.7:7447"]}"#).unwrap();
        assert_eq!(
            extract_locator(&j),
            Some("tcp/198.51.100.7:7447".to_string())
        );
    }

    #[test]
    fn extract_locator_finds_link_dst() {
        let j: serde_json::Value =
            serde_json::from_str(r#"{"links":[{"dst":"tcp/10.0.0.1:7447"}]}"#).unwrap();
        assert_eq!(extract_locator(&j), Some("tcp/10.0.0.1:7447".to_string()));
    }

    #[test]
    fn extract_locator_returns_none_on_unknown_shape() {
        let j: serde_json::Value = serde_json::from_str(r#"{"foo":"bar"}"#).unwrap();
        assert_eq!(extract_locator(&j), None);
    }

    /// F-147 follow-up: the actual Zenoh-1.x admin shape
    /// is `@/<own_zid>/session/transport/unicast/<peer_zid>` (and
    /// `.../unicast/<peer_zid>/link/<id>` for per-link metadata). The
    /// peer zid is always the segment immediately following `/unicast/`.
    /// Mirrors the extraction logic in the admin_tail body.
    fn extract_session_peer_zid(key: &str) -> Option<String> {
        key.split("/session/transport/unicast/")
            .nth(1)
            .and_then(|after| after.split('/').next())
            .filter(|s| is_plausible_zid(s))
            .map(|s| s.to_string())
    }

    #[test]
    fn session_transport_key_extracts_peer_zid() {
        let key = "@/72a21b14f9f3a98f497f821ab12026e6/session/transport/unicast/36701131dff1e3a015f69f2440d88ba8";
        assert_eq!(
            extract_session_peer_zid(key),
            Some("36701131dff1e3a015f69f2440d88ba8".to_string())
        );
    }

    #[test]
    fn session_transport_key_with_link_suffix_still_extracts_peer_zid() {
        // /link/ leaves carry a numeric link-id; we want the segment
        // immediately after `/unicast/`, not rsplit's last segment.
        let key = "@/72a21b14f9f3a98f497f821ab12026e6/session/transport/unicast/36701131dff1e3a015f69f2440d88ba8/link/10542238000777157041";
        assert_eq!(
            extract_session_peer_zid(key),
            Some("36701131dff1e3a015f69f2440d88ba8".to_string())
        );
    }

    #[test]
    fn session_transport_key_rejects_non_zid_after_unicast() {
        let key = "@/72a21b14f9f3a98f497f821ab12026e6/session/transport/unicast/notazid";
        assert_eq!(extract_session_peer_zid(key), None);
    }

    #[test]
    fn session_transport_key_returns_none_for_non_session_keys() {
        assert_eq!(extract_session_peer_zid("@/router/aabbccddeeff0011"), None);
        assert_eq!(extract_session_peer_zid("vendor-b/sensorgrid/raw"), None);
    }


    // Census parse/filter logic is owned + unit-tested in
    // `panoscope_core::census`; this glue test proves the admin-tail branch
    // records the shared-parsed census into panoscope's Topology (a pure `**`
    // subscriber becomes a Subscriber node; the router's own storage self-sub
    // is filtered).
    #[test]
    fn census_branch_records_pure_subscriber_into_topology() {
        use panoscope_core::census::{census_sources, parse_resource_key, ResourceRole};
        let own = "ffffffffffffffff";
        let mut t = crate::model::Topology::default();
        let apply = |t: &mut crate::model::Topology, key: &str, payload: &serde_json::Value| {
            let (role, owner_zid, keyexpr) =
                parse_resource_key(key).expect("census key parses");
            for zid in census_sources(payload) {
                if zid == owner_zid || zid == own {
                    continue;
                }
                match role {
                    ResourceRole::Subscriber => {
                        t.upsert_admin_peer(zid.clone(), Some(NodeKind::Subscriber), None, 1000);
                        t.record_subscribe_declaration(zid, keyexpr.clone(), 1000);
                    }
                    ResourceRole::Publisher => {
                        t.record_publish_declaration(zid, keyexpr.clone(), 1000);
                    }
                }
            }
        };
        // Router storage self-sub — filtered (owner ZID).
        apply(
            &mut t,
            "@/b2ec0bfb5f92ed362a10cff8d181ae96/router/subscriber/c9122c354e6187cb836d97175e8237c1/**",
            &serde_json::json!({"routers":["b2ec0bfb5f92ed362a10cff8d181ae96"],"peers":[],"clients":["b2ec0bfb5f92ed362a10cff8d181ae96"]}),
        );
        assert!(t.peers.is_empty(), "storage self-sub records nothing");
        // Real pure `**` subscriber — recorded as a Subscriber node.
        apply(
            &mut t,
            "@/b2ec0bfb5f92ed362a10cff8d181ae96/router/subscriber/**",
            &serde_json::json!({"routers":["b2ec0bfb5f92ed362a10cff8d181ae96"],"peers":[],"clients":["36701131dff1e3a015f69f2440d88ba8"]}),
        );
        let p = t.peers.get("36701131dff1e3a015f69f2440d88ba8").expect("subscriber node");
        assert_eq!(p.node_kind, Some(NodeKind::Subscriber));
        assert!(p.subscribes.contains("**"));
        assert!(p.publishes.is_empty());
    }

    #[test]
    fn peer_id_from_topic_takes_vendor_system_pair() {
        assert_eq!(
            peer_id_from_topic("caracal/tracks-demo/entity/v1"),
            Some("caracal/tracks-demo".to_string())
        );
        assert_eq!(
            peer_id_from_topic("caracal/power-demo/site-nova/v1"),
            Some("caracal/power-demo".to_string())
        );
    }

    #[test]
    fn peer_id_from_topic_uses_anchor_alone_under_slot_pool() {
        // A 32-hex slot anchor is the publisher's whole identity (ADR 1025);
        // don't append the descriptive vendor segment, so this converges with
        // the pinned-ZID path instead of producing a second node.
        assert_eq!(
            peer_id_from_topic("0d06fc463154cb88502bc25fa9f0989a/mantis/alpha-fob/imagery/v1"),
            Some("0d06fc463154cb88502bc25fa9f0989a".to_string())
        );
        // Bare anchor (no taxonomy) also returns itself.
        assert_eq!(
            peer_id_from_topic("0d06fc463154cb88502bc25fa9f0989a"),
            Some("0d06fc463154cb88502bc25fa9f0989a".to_string())
        );
    }

    #[test]
    fn peer_id_from_topic_degrades_to_single_segment() {
        assert_eq!(
            peer_id_from_topic("caracal"),
            Some("caracal".to_string())
        );
    }

    #[test]
    fn peer_id_from_topic_tolerates_leading_or_doubled_slashes() {
        assert_eq!(
            peer_id_from_topic("/caracal/tracks-demo/x"),
            Some("caracal/tracks-demo".to_string())
        );
        assert_eq!(
            peer_id_from_topic("caracal//tracks-demo//x"),
            Some("caracal/tracks-demo".to_string())
        );
    }

    #[test]
    fn peer_id_from_topic_returns_none_for_empty() {
        assert_eq!(peer_id_from_topic(""), None);
        assert_eq!(peer_id_from_topic("/"), None);
        assert_eq!(peer_id_from_topic("///"), None);
    }

    #[test]
    fn is_plausible_zid_bounds() {
        assert!(is_plausible_zid("aabbccdd")); // 8 chars
        assert!(is_plausible_zid("aabbccddeeff0011")); // 16 chars (typical)
        assert!(is_plausible_zid(&"a".repeat(32))); // upper bound
        assert!(!is_plausible_zid(&"a".repeat(33))); // over
        assert!(!is_plausible_zid("aabbccd")); // 7 chars (under)
        assert!(!is_plausible_zid("xyz12345")); // non-hex
    }

    fn schema_decl_json(peer_id: &str, topic: &str, published_at_ms: Option<u64>) -> serde_json::Value {
        let mut v = serde_json::json!({
            "peer_id": peer_id,
            "vendor": "test-vendor",
            "site": "alpha-fob",
            "role_class": "sensor",
            "format": "json",
            "publishes": [topic],
            "subscribes": [],
        });
        if let Some(at) = published_at_ms {
            v["published_at_ms"] = serde_json::json!(at);
        }
        v
    }

    #[test]
    fn schema_decl_within_staleness_threshold_applies() {
        let mut t = crate::model::Topology::default();
        let now = 1_000_000_u64;
        let decl = schema_decl_json("p1", "caracal/aurochs/charlie/launched/v1", Some(now - 30_000));
        apply_schema_declaration(&mut t, "attest/schema/caracal/aurochs/charlie/v1", &decl, now);
        assert!(t.peers.contains_key("p1"), "fresh decl should land");
        assert_eq!(
            t.topic_to_publisher.get("caracal/aurochs/charlie/launched/v1"),
            Some(&"p1".to_string())
        );
    }

    #[test]
    fn schema_decl_past_staleness_threshold_is_skipped() {
        let mut t = crate::model::Topology::default();
        let now = 1_000_000_u64;
        // Decl is older than SCHEMA_DECL_STALE_MS — should be dropped on
        // the floor without mutating topology (the residue case post-
        // namespace-migration: caracal/community decls in rocksdb that
        // got replayed at panoscope startup but no live peer is
        // refreshing them).
        let decl = schema_decl_json(
            "stale-ghost",
            "caracal/aurochs/charlie/launched/v1",
            Some(now - SCHEMA_DECL_STALE_MS - 1),
        );
        apply_schema_declaration(&mut t, "attest/schema/caracal/aurochs/charlie/v1", &decl, now);
        assert!(!t.peers.contains_key("stale-ghost"));
        assert!(t.topic_to_publisher.get("caracal/aurochs/charlie/launched/v1").is_none());
    }

    #[test]
    fn schema_decl_without_published_at_ms_passes_through() {
        // Older synth-vendor payloads / partner peers that don't include
        // the field aren't subject to the gate — staleness is opt-in.
        let mut t = crate::model::Topology::default();
        let now = 1_000_000_u64;
        let decl = schema_decl_json("p2", "x/y/z/v1", None);
        apply_schema_declaration(&mut t, "attest/schema/x/y/v1", &decl, now);
        assert!(t.peers.contains_key("p2"));
    }
}
