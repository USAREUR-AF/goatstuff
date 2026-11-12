// SPDX-License-Identifier: Apache-2.0
//
// panoscope — live visualization of a Zenoh fabric.
//
// Design: docs/design/panoscope.md
// Tracking: docs/project/implementation-plan.md Block 82Q

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::{
    extract::{ws::WebSocketUpgrade, State},
    response::IntoResponse,
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

mod access_view;
mod config;
mod decoder;
mod embed;
mod health_wire;
mod minio_replay;
mod model;
mod sample_buffer;
mod schema;
mod snapshot_view;
mod state_snapshot;
mod top_view;
mod view;
mod viewer;
mod who_view;
mod ws;
mod zenoh_client;

use config::Config;
use decoder::{Decoder, TopicDecodeOutcome};
use model::now_ms;
use panoscope_core::PortalClient;
use schema::{FamilyMapResolver, ResolverStatus, SchemaResolver};
use snapshot_view::{build_expanded_edges_with, EdgeView, NodeView};
use viewer::{RbacConfig, ViewerScope};
use zenoh_client::{SharedState, EMA_TAU_SECS};

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let config = Config::from_env_or_default()?;
    info!(
        bind = %config.bind,
        zenoh_endpoint = %config.zenoh_endpoint,
        portal_schemas_url = %config.portal_schemas_url,
        sampling_per_topic_eps = config.sampling_per_topic_eps,
        ws_coalesce_ms = config.ws_coalesce_ms,
        "panoscope starting"
    );

    // Schema resolver = the compiled-in family-map floor. Per-peer edge
    // coloring rides schema-decl self-registration in zenoh_client (it
    // subscribes attest/schema/** and writes each peer's declared family
    // onto the topology); this floor only names the well-known prefix
    // families. The portal never served a bulk family endpoint, so the
    // old portal-fetch resolver was removed; see schema.rs.
    let resolver: Arc<dyn SchemaResolver> = Arc::new(FamilyMapResolver::new());

    // Start the zenoh client + tails BEFORE binding HTTP so /healthz only
    // returns ok once we have a live session.
    let state = zenoh_client::run(
        &config.zenoh_endpoint,
        &config.zenoh_tls,
        config.sampling_per_topic_eps,
        config.topic_blocklist.clone(),
        resolver,
    )
    .await?;

    // MinIO replay-on-boot handles. Re-hydrates the peer registry from the
    // audit-subscriber's NDJSON history so a container-recreate that wipes
    // in-RAM state doesn't drop every non-synth-vendor peer for hours-to-
    // days. Cloned here (Arc) so the replay can run in a BACKGROUND task
    // after the HTTP listener binds — replaying the 168h audit window takes
    // ~150s, and awaiting it before the bind used to leave the server
    // unbound for that whole window (Traefik 502 on every restart; F-168).
    let replay_cfg = config.minio_replay.clone();
    let replay_cfg_window_hours = replay_cfg.window_hours;
    let replay_topology = state.topology.clone();
    let replay_flow = state.flow.clone();

    // Deep on-demand last-seen scan (history-lens v1 follow-on). The boot
    // replay above is intentionally shallow (PANOSCOPE_MINIO_WINDOW_HOURS,
    // typically 6h) so the COP comes up fast. A SECOND, deeper pass then runs
    // lazily AFTER the fast seed completes — non-blocking for load-up — so
    // the operator's deeper lens windows (3d/7d) fill in soon after a cold
    // start instead of staying empty until live samples re-accumulate. Only
    // runs when the deep window is actually deeper than the boot window.
    let deep_window_hours: u32 = std::env::var("PANOSCOPE_MINIO_DEEP_WINDOW_HOURS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(168);
    let deep_cfg = {
        let mut c = replay_cfg.clone();
        c.window_hours = deep_window_hours;
        c
    };
    let deep_topology = state.topology.clone();
    let deep_flow = state.flow.clone();

    // Restore derived state from disk, so a restart does not re-derive from
    // 136,830 MinIO objects what it wrote down last time.
    //
    // Two effects, and the second is the one that needs care. The topology and
    // flow are installed straight away, so /api/snapshot is useful immediately
    // instead of after the walk. And the deep replay window SHRINKS to the age
    // of the snapshot plus a margin — which is only sound because the replay
    // is an upsert: replaying a shorter window on top of restored state
    // refreshes what changed and leaves everything else at its restored value.
    //
    // Any problem at all — no file, unreadable, malformed, a version this build
    // does not know, or too old to bound the window — falls back to the full
    // window. A snapshot may shrink work; it may never skip a period.
    let snapshot_path: std::path::PathBuf = std::env::var("PANOSCOPE_STATE_SNAPSHOT_PATH")
        .unwrap_or_else(|_| "/var/lib/panoscope/state-snapshot.json".to_string())
        .into();
    let snapshot_enabled = std::env::var("PANOSCOPE_STATE_SNAPSHOT_ENABLED")
        .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
        .unwrap_or(true);

    let mut deep_cfg = deep_cfg;
    if snapshot_enabled {
        if let Some(snap) = crate::state_snapshot::load(&snapshot_path) {
            let now = crate::model::now_ms();
            let peers = snap.topology.peers.len();
            match crate::state_snapshot::replay_window_hours(
                snap.written_at_ms,
                now,
                deep_window_hours,
            ) {
                Some(narrowed) => {
                    *state.topology.write().await = snap.topology;
                    *state.flow.write().await = snap.flow;
                    info!(
                        peers,
                        age_hours = (now.saturating_sub(snap.written_at_ms)) / 3_600_000,
                        narrowed_window_hours = narrowed,
                        full_window_hours = deep_window_hours,
                        "state snapshot: restored — deep replay narrowed to the delta"
                    );
                    deep_cfg.window_hours = narrowed;
                }
                None => {
                    // Restoring here would be harmless, but the full walk is
                    // about to overwrite it anyway; skipping keeps "what is in
                    // the registry" traceable to exactly one source.
                    info!(
                        peers,
                        "state snapshot: too old to narrow the window — full replay, snapshot ignored"
                    );
                }
            }
        }
    }

    let snapshot_topology = state.topology.clone();
    let snapshot_flow = state.flow.clone();
    let snapshot_write_path = snapshot_path.clone();

    // In-RAM peer evictor (history-lens v1 follow-on). `upsert_peer` never
    // removes, so the peer table grows unbounded over long uptimes. A periodic
    // task drops peers unseen for longer than the retention (7d default; set
    // PANOSCOPE_PEER_RETENTION_HOURS to change). Keep retention >= the deepest
    // selectable lens window so eviction never races the lens (topology.rs).
    let peer_retention_hours: u64 = std::env::var("PANOSCOPE_PEER_RETENTION_HOURS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(168);
    let evict_topology = state.topology.clone();

    // Slot-alias resolver refresh (ADR 1039). Polls the portal's
    // GET /api/aliases on the schema cadence and swaps the live registry
    // panoscope renders node labels through. No-op when the portal is
    // unconfigured (empty api key) — the registry stays empty and every
    // node renders its truncated UUID, never an error.
    spawn_alias_refresher(
        state.aliases.clone(),
        state.alias_health.clone(),
        PortalClient::new(config.portal_base_url(), config.portal_api_key.clone()),
        Duration::from_secs(config.portal_refresh_secs.max(1)),
    );

    // Build the descriptor cache for the decoded-stream view. Disabled
    // when PANOSCOPE_PORTAL_API_KEY isn't set — the frontend then renders
    // NO SCHEMA for every topic in the Decoded tab. See decoder.rs.
    let decoder = Decoder::new(PortalClient::new(
        config.portal_base_url(),
        config.portal_api_key.clone(),
    ));

    // Throughput-envelope registry (ADR 1045) for the /top BAND column,
    // refreshed on the alias cadence. Same degradation contract as the
    // alias refresher: portal disabled/down ⇒ the registry stays as-is
    // (empty ⇒ every band renders an honest `—`).
    let envelopes = Arc::new(tokio::sync::RwLock::new(
        panoscope_core::envelope::EnvelopeRegistry::empty(),
    ));
    spawn_envelope_refresher(
        envelopes.clone(),
        PortalClient::new(config.portal_base_url(), config.portal_api_key.clone()),
        Duration::from_secs(config.portal_refresh_secs.max(1)),
    );

    // Portal client for the /who roster + detail proxy.
    let portal = Arc::new(PortalClient::new(
        config.portal_base_url(),
        config.portal_api_key.clone(),
    ));

    // The shared view model behind /api/view* (docs/design/panoscope-view-model.md).
    view::service::spawn_model_builder(state.clone());

    let app = build_router(
        state,
        config.ws_coalesce_ms,
        config.portal_base_url(),
        config.resolved_siblings(),
        decoder,
        config.lexicon.clone(),
        Arc::new(config.rbac.clone()),
        envelopes,
        portal,
    );

    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    info!(bind = %config.bind, "listening");

    // Replay runs in the background so the server is reachable immediately;
    // /api/snapshot returns a progressively-populated topology (synth-
    // vendors also re-publish within 30s). Best-effort — a MinIO outage at
    // boot just means the registry rebuilds from live samples. F-168.
    tokio::spawn(async move {
        crate::minio_replay::replay_into_topology(
            &replay_cfg,
            &replay_topology,
            &replay_flow,
        )
        .await;
    });

    // Deep last-seen backfill: runs once after the fast boot seed, lazily.
    if deep_window_hours > replay_cfg_window_hours {
        tokio::spawn(async move {
            // Brief stagger so the fast boot pass and live samples populate
            // first — the deep pass is backfill, not the critical path.
            tokio::time::sleep(Duration::from_secs(30)).await;
            info!(
                deep_window_hours,
                "minio_replay: starting deep last-seen backfill pass"
            );
            crate::minio_replay::replay_into_topology(&deep_cfg, &deep_topology, &deep_flow).await;
            info!("minio_replay: deep last-seen backfill pass complete");
        });
    }

    // Persist the derived state: once the backfill has had time to land, then
    // hourly. Hourly is what bounds how much a restart has to re-derive — the
    // narrowed window is the snapshot's age, so an hour-old snapshot costs an
    // hour of replay instead of seven days of it.
    //
    // Best-effort by construction. A failed write is logged and the next tick
    // tries again; the only cost of never succeeding is the slow cold start
    // this exists to avoid.
    if snapshot_enabled {
        tokio::spawn(async move {
            // Long enough for the 30s-staggered deep pass to finish on a
            // healthy boot, so the first snapshot written is a complete one
            // rather than a half-filled registry.
            tokio::time::sleep(Duration::from_secs(600)).await;
            let mut tick = tokio::time::interval(Duration::from_secs(3600));
            loop {
                {
                    // Clone under each lock in turn, release, then serialize
                    // and write off the tokio workers. Holding both read locks
                    // across serde + disk I/O stalled the data tail.
                    let topo = snapshot_topology.read().await.clone();
                    let flow = snapshot_flow.read().await.clone();
                    let now = crate::model::now_ms();
                    let path = snapshot_write_path.clone();
                    let peers = topo.peers.len();
                    // Admin path, not the relay hot path the workspace lint is written for.
                    #[allow(clippy::disallowed_methods)]
                    let result = tokio::task::spawn_blocking(move || {
                        crate::state_snapshot::save(&path, &topo, &flow, now)
                    })
                    .await
                    .unwrap_or_else(|e| Err(std::io::Error::other(e.to_string())));
                    match result {
                        Ok(()) => info!(
                            path = %snapshot_write_path.display(),
                            peers,
                            "state snapshot: written"
                        ),
                        Err(e) => warn!(
                            path = %snapshot_write_path.display(),
                            error = %e,
                            "state snapshot: write failed — a restart will do the full replay"
                        ),
                    }
                }
                tick.tick().await;
            }
        });
    }

    // Periodic peer evictor.
    tokio::spawn(async move {
        let retention_ms = peer_retention_hours.saturating_mul(3_600_000);
        let mut tick = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tick.tick().await;
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let cutoff = now_ms.saturating_sub(retention_ms);
            let removed = evict_topology.write().await.evict_stale(cutoff);
            if removed > 0 {
                info!(
                    removed,
                    peer_retention_hours, "peer_evictor: dropped peers unseen past retention"
                );
            }
        }
    });

    axum::serve(listener, app).await?;
    Ok(())
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("panoscope=info,tower_http=info,axum=info"));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(true)
        .init();
}

/// Background task: refresh the slot-alias registry (ADR 1039) from the
/// portal's `GET /api/aliases` on `interval`, swapping the live
/// `AliasRegistry` panoscope renders `NodeView::slot_alias` through. The
/// first poll runs immediately so labels resolve without waiting a full
/// interval. A failed/unconfigured poll LEAVES THE EXISTING registry in
/// place (a transient portal blip never blanks names back to UUIDs); only a
/// successful poll swaps it. When the portal is disabled the very first
/// poll returns `Disabled` and the registry simply stays empty (every node
/// renders its truncated UUID — degraded, not broken).
///
/// Every outcome is also recorded in `health`, which `/api/legend` serves.
/// Rendering UUIDs is a legitimate degraded state; rendering them with no
/// way to tell *why* lets a deployment run on an unset
/// `PANOSCOPE_PORTAL_API_KEY` while its portal holds every alias, unnoticed.
/// The disabled branch therefore WARNs on the first poll —
/// it is a deploy gap that will never self-heal — and drops to debug after,
/// so a misconfigured deploy is loud once rather than either silent forever
/// or noisy every interval.
fn spawn_alias_refresher(
    aliases: Arc<tokio::sync::RwLock<panoscope_core::AliasRegistry>>,
    health: Arc<tokio::sync::RwLock<panoscope_core::AliasRegistryHealth>>,
    portal: PortalClient,
    interval: Duration,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        let mut warned_disabled = false;
        loop {
            ticker.tick().await;
            match portal.list_aliases().await {
                Ok(rows) => {
                    let n = rows.len();
                    let reg = panoscope_core::AliasRegistry::from_rows(rows);
                    let known = reg.len();
                    *aliases.write().await = reg;
                    health.write().await.record_ok(known, now_ms());
                    warned_disabled = false;
                    info!(rows = n, aliases = known, "slot-alias registry refreshed");
                }
                Err(panoscope_core::PortalError::Disabled) => {
                    health.write().await.record_disabled();
                    if !warned_disabled {
                        warned_disabled = true;
                        tracing::warn!(
                            "slot-alias resolution DISABLED: PANOSCOPE_PORTAL_API_KEY is unset, \
                             so GET /api/aliases is never called and every peer will render its \
                             raw UUID. This is a deploy gap, not a runtime fault — set \
                             panoscope_portal_api_key in group_vars/<env>_hosts/panoscope.yml \
                             and re-apply the panoscope role. Reported as \
                             alias_registry.state=disabled on /api/legend."
                        );
                    } else {
                        tracing::debug!("alias refresh skipped: portal disabled");
                    }
                }
                Err(e) => {
                    health.write().await.record_error(e.to_string());
                    tracing::warn!(error = %e, "slot-alias refresh failed; keeping prior registry");
                }
            }
        }
    });
}

/// Server-side roster cache for the /who page: one bulk portal fetch
/// shared across viewers, refreshed at most every TTL. Last-good is kept
/// so a portal blip serves an honestly-aged copy instead of a blank.
#[derive(Default)]
struct RosterCache {
    fetched_at_ms: u64,
    roster: Option<panoscope_core::portal::AccessRoster>,
}

const ROSTER_CACHE_TTL_MS: u64 = 15_000;

/// Background task: refresh the envelope registry (ADR 1045) from the
/// portal's `GET /api/envelopes` on `interval`. Mirrors the alias
/// refresher's degradation contract: only a successful poll swaps the
/// registry; disabled/down leaves the prior one in place.
fn spawn_envelope_refresher(
    envelopes: Arc<tokio::sync::RwLock<panoscope_core::envelope::EnvelopeRegistry>>,
    portal: PortalClient,
    interval: Duration,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            match portal.list_envelopes().await {
                Ok(rows) => {
                    let reg = panoscope_core::envelope::EnvelopeRegistry::from_rows(rows);
                    let known = reg.len();
                    *envelopes.write().await = reg;
                    tracing::debug!(envelopes = known, "envelope registry refreshed");
                }
                Err(panoscope_core::PortalError::Disabled) => {
                    tracing::debug!("envelope refresh skipped: portal disabled");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "envelope refresh failed; keeping prior registry");
                }
            }
        }
    });
}

#[derive(Clone)]
struct AppState {
    shared: SharedState,
    ws_coalesce_ms: u64,
    portal_base_url: String,
    siblings: crate::config::SiblingUrls,
    decoder: Arc<Decoder>,
    /// Display-only lexicon — surfaced on `/api/legend` for runtime JS
    /// relexing + applied to `index.html` tokens at serve time.
    lexicon: crate::config::Lexicon,
    /// Per-viewer RBAC scoping config (ADR 1073). Mode `off` ⇒ every
    /// request resolves to Full scope (pre-RBAC behavior).
    rbac: Arc<RbacConfig>,
    /// Throughput envelopes (ADR 1045) refreshed from the portal on the
    /// alias cadence; empty registry ⇒ every band is an honest `—`.
    envelopes: Arc<tokio::sync::RwLock<panoscope_core::envelope::EnvelopeRegistry>>,
    /// Portal client for the /who roster + detail proxy (same service
    /// key as the alias/decoder paths; empty key ⇒ Disabled).
    portal: Arc<PortalClient>,
    roster_cache: Arc<tokio::sync::RwLock<RosterCache>>,
    /// Org-list cache for the /access catalog (60 s TTL — org rows churn
    /// slowly; per-org topics are fetched lazily per request instead).
    orgs_cache: Arc<tokio::sync::RwLock<OrgsCache>>,
    /// Per-viewer data-roles cache backing the ADR 1074 B1 participant
    /// resolver enrichment (email → derived `data.view.*` roles).
    data_roles_cache: Arc<tokio::sync::RwLock<DataRolesCache>>,
}

impl AppState {
    /// Resolve the ViewerScope for one request. Base scope is the
    /// claim-derived ADR 1073 lattice ([`ViewerScope::from_headers`]).
    /// When the B1 participant resolver is enabled AND the base is the
    /// Scoped tier AND the request carries a trusted login email, fold in
    /// the registry-derived data-roles from the portal (cached, short
    /// TTL). Fail-closed at every step: RBAC off, a non-Scoped tier, no
    /// email, a disabled/erroring portal, or an unknown principal all
    /// leave the base scope untouched — enrichment only ever widens the
    /// participant tier, never narrows any tier.
    async fn viewer_scope(&self, headers: &axum::http::HeaderMap) -> ViewerScope {
        let base = ViewerScope::from_headers(headers, &self.rbac);
        if !self.rbac.participant_resolver {
            return base;
        }
        // Only the Scoped participant tier benefits; Full/DataAll are
        // already maximal and must never round-trip the resolver.
        if !matches!(base, ViewerScope::Scoped { .. }) {
            return base;
        }
        let Some(email) = crate::viewer::viewer_email(headers) else {
            return base;
        };
        let roles = self.data_roles_for(&email).await;
        base.enriched_with(&roles)
    }

    /// Cached fetch of a viewer's registry-derived data-roles. Returns an
    /// empty vec on any miss/error (the caller then leaves the base scope
    /// — fail-closed). Positive results cache for `DATA_ROLES_CACHE_TTL_MS`;
    /// misses/errors are not cached.
    async fn data_roles_for(&self, email: &str) -> Vec<String> {
        let now = now_ms();
        {
            let c = self.data_roles_cache.read().await;
            if let Some((roles, at)) = c.entries.get(email) {
                if now.saturating_sub(*at) < DATA_ROLES_CACHE_TTL_MS {
                    return roles.clone();
                }
            }
        }
        match self.portal.fetch_data_roles(email).await {
            Ok(roles) => {
                let mut c = self.data_roles_cache.write().await;
                c.entries.insert(email.to_string(), (roles.clone(), now));
                roles
            }
            Err(e) => {
                // Fail-closed: base scope stands this request. Logged so a
                // misconfigured lookup token (401) or portal outage is
                // visible rather than a silent under-grant.
                tracing::warn!(error = %e, "data-roles resolver enrichment failed; base scope stands");
                Vec::new()
            }
        }
    }
}

#[derive(Default)]
struct OrgsCache {
    fetched_at_ms: u64,
    orgs: Option<Vec<panoscope_core::portal::Vendor>>,
}

const ORGS_CACHE_TTL_MS: u64 = 60_000;

/// Per-viewer cache of the registry-derived data-roles (ADR 1074 B1),
/// keyed by trusted login email. The resolver does a Kratos identity scan
/// plus a roster join per call, so a short TTL keeps the hot request path
/// off that work while staying fresh enough that a just-granted participant
/// sees their org within a minute. Negative results (miss/error) are NOT
/// cached — a transient portal blip must not pin a participant to the bare
/// floor for the whole TTL; it just falls back to the base scope this
/// request and retries next time.
#[derive(Default)]
struct DataRolesCache {
    entries: HashMap<String, (Vec<String>, u64)>,
}

const DATA_ROLES_CACHE_TTL_MS: u64 = 60_000;

#[allow(clippy::too_many_arguments)]
fn build_router(
    shared: SharedState,
    ws_coalesce_ms: u64,
    portal_base_url: String,
    siblings: crate::config::SiblingUrls,
    decoder: Arc<Decoder>,
    lexicon: crate::config::Lexicon,
    rbac: Arc<RbacConfig>,
    envelopes: Arc<tokio::sync::RwLock<panoscope_core::envelope::EnvelopeRegistry>>,
    portal: Arc<PortalClient>,
) -> Router {
    let app_state = AppState {
        shared,
        ws_coalesce_ms,
        portal_base_url,
        siblings,
        decoder,
        lexicon,
        rbac,
        envelopes,
        portal,
        roster_cache: Arc::new(tokio::sync::RwLock::new(RosterCache::default())),
        orgs_cache: Arc::new(tokio::sync::RwLock::new(OrgsCache::default())),
        data_roles_cache: Arc::new(tokio::sync::RwLock::new(DataRolesCache::default())),
    };
    Router::new()
        .route("/healthz", get(healthz))
        .route("/version", get(version))
        .route("/api/stats", get(stats))
        .route("/api/snapshot", get(snapshot))
        .route("/api/snapshot/expand", get(snapshot_expand))
        .route("/api/view", axum::routing::post(view_handler))
        .route("/api/view/group", axum::routing::post(view_group_handler))
        .route("/api/view/search", axum::routing::post(view_search_handler))
        .route("/api/view/reveal", axum::routing::post(view_reveal_handler))
        .route("/api/view/build", get(view_build_handler))
        .route("/api/topics", get(topics_handler))
        .route("/api/publisher/hygiene", get(publisher_hygiene))
        .route("/api/legend", get(legend))
        .route("/api/samples", get(samples))
        .route("/api/samples/decoded", get(samples_decoded))
        .route("/api/top", get(top))
        .route("/api/access/catalog", get(access_catalog))
        .route("/api/access/catalog/topics", get(access_catalog_topics))
        .route("/api/access/query/why", get(access_query_why))
        .route("/api/access/query/topic", get(access_query_topic))
        .route("/api/who/roster", get(who_roster))
        .route("/api/who/detail", get(who_detail))
        .route("/ws", get(ws_upgrade))
        // Fallback registered BEFORE with_state so it can pull AppState
        // (the lexicon) for serve-time HTML token substitution.
        .fallback(serve_frontend)
        .with_state(app_state)
}

/// Stateful fallback — serves the embedded SPA, applying the display
/// lexicon to HTML at serve time (`embed::serve_embedded`).
async fn serve_frontend(
    State(app): State<AppState>,
    uri: axum::http::Uri,
) -> axum::response::Response {
    embed::serve_embedded(uri, &app.lexicon).await
}

/// Notional second-chance (ADR 1074): a key/peer denied by the scope's
/// text rules is still allowed when the viewer holds the notional grant
/// and the topic's publisher (or the peer itself) self-declares
/// `synthetic: true` in its schema-decl. Key text alone can't prove
/// synthetic-ness, so this resolves through the live topology.
async fn topic_is_synthetic(shared: &SharedState, topic: &str) -> bool {
    let topology = shared.topology.read().await;
    topology
        .topic_to_publisher
        .get(topic)
        .and_then(|pid| topology.peers.get(pid))
        .is_some_and(|p| p.synthetic == Some(true))
}

async fn peer_is_synthetic(shared: &SharedState, peer_id: &str) -> bool {
    shared
        .topology
        .read()
        .await
        .peers
        .get(peer_id)
        .is_some_and(|p| p.synthetic == Some(true))
}

#[derive(Serialize, Deserialize)]
struct Healthz {
    status: &'static str,
}

async fn healthz() -> axum::Json<Healthz> {
    axum::Json(Healthz { status: "ok" })
}

#[derive(Serialize, Deserialize)]
struct Version {
    name: &'static str,
    version: &'static str,
}

async fn version() -> axum::Json<Version> {
    axum::Json(Version {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn stats(State(app): State<AppState>) -> axum::Json<zenoh_client::Stats> {
    let mut s = app.shared.stats.read().await.clone();
    s.data_samples_dropped_backpressure = app
        .shared
        .ingest_backpressure_drops
        .load(std::sync::atomic::Ordering::Relaxed);
    axum::Json(s)
}

/// Snapshot view — peers (with role + projected EPS rollup) + edges
/// (with projected EPS using the same decay as the model). Designed to
/// be cheap to compute on every poll.
#[derive(Serialize)]
struct Snapshot {
    nodes: Vec<NodeView>,
    edges: Vec<EdgeView>,
    sampling_drops: HashMap<String, u64>,
    generated_at_ms: u64,
}

/// History-lens window selector (docs/design/panoscope-history-lens.md). When
/// set, the snapshot is filtered to peers whose `last_seen_ms` is within the
/// requested window — the operator's chosen time horizon. Omitted ⇒ no filter
/// (every accumulated peer), preserving pre-lens behavior for old clients.
#[derive(Deserialize)]
struct SnapshotQuery {
    window_hours: Option<u64>,
    /// Aggressive simplified view: roll every publisher's fanout topics
    /// (namespace-folds AND distinct one-offs) into one bundle per publisher.
    /// Operator-toggled; absent/false ⇒ the normal per-namespace fold.
    aggressive: Option<bool>,
}

async fn snapshot(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<SnapshotQuery>,
) -> axum::Json<Snapshot> {
    let scope = app.viewer_scope(&headers).await;
    let now = now_ms();
    // Drop the three RwLock read guards BEFORE serializing JSON. Earlier
    // version held them across the axum::Json(..) construction, which on
    // a busy fabric (admin_tail + data_tail upserting on every sample)
    // created a long critical section that contended with the write
    // path. The visible symptom on the operator side was a pile-up of
    // pending /api/snapshot fetches in DevTools (browsers cap concurrent
    // connections at 6 per origin); once latency exceeded the poll
    // interval, requests stacked, the connection pool saturated, new
    // fetches sat 'pending' indefinitely, and the fleet-stats HUD's
    // 'STALE' tag tripped on healthy internet because lastSuccessAt
    // fell arbitrarily behind. Restructuring the handler so the locks
    // drop the moment the owned views are built bounds the contention
    // to the cheap clone/copy work, not the JSON serialization.
    // One build shared by every viewer; scope, window and the aggressive
    // rollup below stay per request (snapshot_view::shared_snapshot_build).
    let build = snapshot_view::shared_snapshot_build(&app.shared).await;
    let nodes = (*build.nodes).clone();
    let edges = (*build.edges).clone();
    let sampling_drops = scope.filter_sampling_drops(&build.sampling_drops);
    // History-lens window filter (docs/design/panoscope-history-lens.md):
    // keep peers seen within the operator's chosen horizon. Pure + unit-tested
    // in snapshot_view; 0/absent ⇒ no filter (pre-lens behavior).
    let (nodes, edges) =
        snapshot_view::filter_snapshot_window(nodes, edges, now, q.window_hours.unwrap_or(0));
    // Per-viewer RBAC scope filter (ADR 1073) — no-op at Full scope.
    let (nodes, edges) = scope.filter_snapshot(nodes, edges);
    // Aggressive simplified view (operator toggle): collapse each publisher's
    // whole fanout into one bundle edge. Drill-in stays available via the
    // existing expand endpoint (the bundle's `**` folded_namespace). Applied
    // after the window filter so it only bundles what's actually on-screen.
    let edges = if q.aggressive.unwrap_or(false) {
        snapshot_view::aggressive_rollup(edges)
    } else {
        edges
    };
    axum::Json(Snapshot {
        nodes,
        edges,
        sampling_drops,
        generated_at_ms: build.built_at_ms,
    })
}

/// The viewer's view: shared fabric model → scope, window, filters, grouping,
/// element cap (docs/design/panoscope-view-model.md). `ETag` is the model
/// generation + scope + parameters; a matching `If-None-Match` answers 304.
async fn view_handler(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::Json(params): axum::Json<view::assemble::ViewParams>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let scope = app.viewer_scope(&headers).await;
    let model = app.shared.view_model.read().await.clone();
    let now = now_ms();
    let etag = view::assemble::etag(model.generation, &scope, &params, now);
    if headers
        .get(axum::http::header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == etag)
    {
        return (
            axum::http::StatusCode::NOT_MODIFIED,
            [(axum::http::header::ETAG, etag)],
        )
            .into_response();
    }
    // One assembly per generation serves every viewer asking the same thing.
    // Keyed on the scope and parameters THEMSELVES, never on the tag: the tag
    // is a 64-bit hash of client-supplied strings, and this cache decides whose
    // scope-cut body a request is served (view::cache).
    let built = view::assemble::view_key(&scope, &params, now).and_then(|key| {
        view::cache::body_for(model.generation, key, || {
            serde_json::to_string(&view::assemble::assemble(&model, &scope, &params, now))
        })
    });
    let body = match built {
        Ok(body) => body,
        Err(e) => {
            // Never an empty 200: the page would draw it as a fabric with
            // nothing on it, and the cache would hand it to every viewer.
            warn!(error = %e, "view: serialisation failed; answering 500, nothing cached");
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "view serialisation failed",
            )
                .into_response();
        }
    };
    (
        [
            (axum::http::header::ETAG, etag),
            (
                axum::http::header::CONTENT_TYPE,
                "application/json".to_string(),
            ),
        ],
        body.to_string(),
    )
        .into_response()
}

async fn view_group_handler(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::Json(q): axum::Json<view::service::GroupQuery>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let scope = app.viewer_scope(&headers).await;
    let model = app.shared.view_model.read().await.clone();
    match view::service::group_members(&model, &scope, &q, now_ms()) {
        Some(m) => axum::Json(m).into_response(),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            "no such group in this view (opened, re-grouped, or out of scope)",
        )
            .into_response(),
    }
}

async fn view_search_handler(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::Json(q): axum::Json<view::service::SearchQuery>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let scope = app.viewer_scope(&headers).await;
    let model = app.shared.view_model.read().await.clone();
    axum::Json(view::service::search(&model, &scope, &q, now_ms())).into_response()
}

async fn view_reveal_handler(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::Json(q): axum::Json<view::service::RevealQuery>,
) -> axum::Json<view::service::Reveal> {
    let scope = app.viewer_scope(&headers).await;
    let model = app.shared.view_model.read().await.clone();
    axum::Json(view::service::reveal(&model, &scope, &q, now_ms()))
}

/// Every topic the viewer can see, ungrouped (`goat flow`). Served from the
/// shared model, so it costs a walk of the topic list and takes no ingest lock.
async fn topics_handler(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<view::service::TopicsQuery>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let scope = app.viewer_scope(&headers).await;
    let model = app.shared.view_model.read().await.clone();
    axum::Json(view::service::topics(&model, &scope, &q, now_ms())).into_response()
}

/// Build statistics of the current model, for measurement. Names no topic.
async fn view_build_handler(State(app): State<AppState>) -> axum::Json<serde_json::Value> {
    let model = app.shared.view_model.read().await.clone();
    axum::Json(serde_json::json!({
        "generation": model.generation,
        "built_at_ms": model.built_at_ms,
        "build": model.build,
    }))
}

/// Companion to /api/snapshot: un-fold a single (publisher, namespace)
/// singleton-super group on operator demand. Restores the PR #573
/// interaction model (peek / explode / Alt+click drill-in) on top of
/// the wire-bounded server fold — without this endpoint the snapshot
/// fold would discard the per-topic detail and the operator couldn't
/// drill into "which 19,500 lattice entities are in this group?"
///
/// Query params: source=<peer_id>&namespace=<topic_namespace>.
/// Both fields appear verbatim on every server-folded edge (`source`,
/// `folded_namespace`), so the frontend builds the URL without parsing.
/// Returns the per-topic edges that would have appeared in the snapshot
/// if the fold hadn't run for this group; the frontend merges them
/// into local state and renders as real topic-pins.
///
/// Larger payload than the bounded snapshot (~1 MB for lattice's 19.5k
/// edges) — by design, only fired on explicit operator action.
#[derive(Deserialize)]
struct ExpandQuery {
    source: String,
    namespace: String,
}

#[derive(Serialize)]
struct ExpandedFold {
    source: String,
    namespace: String,
    edges: Vec<EdgeView>,
    generated_at_ms: u64,
}

async fn snapshot_expand(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(params): axum::extract::Query<ExpandQuery>,
) -> Result<axum::Json<ExpandedFold>, (axum::http::StatusCode, String)> {
    let scope = app.viewer_scope(&headers).await;
    // Allowed when the whole (source, namespace) pair is in scope, or the
    // source is a notional producer and the viewer holds the notional
    // grant (the namespace belongs to the source, so synthetic-source
    // covers it).
    let synthetic_ok =
        scope.allows_synthetic_key(peer_is_synthetic(&app.shared, &params.source).await);
    if !(synthetic_ok
        || (scope.allows_peer_inspect(&params.source) && scope.allows_key(&params.namespace)))
    {
        // Explicit denial, never a silent empty payload (the F-177 lesson).
        return Err((
            axum::http::StatusCode::FORBIDDEN,
            "viewer scope does not cover this publisher/namespace (ADR 1073/1074)".to_string(),
        ));
    }
    let now = now_ms();
    // Copy the ingest state one lock at a time, then build off the tokio
    // workers with no lock held. The raw edge build is the expensive walk
    // (every flow edge against every peer's subscriptions); holding the read
    // locks across it stopped the data tail, which needs write locks for
    // every sample.
    let (topology, flow, formats) = snapshot_view::copy_ingest_state(&app.shared).await;
    let (source, namespace) = (params.source.clone(), params.namespace.clone());
    // Admin path, not the relay hot path the workspace lint is written for.
    #[allow(clippy::disallowed_methods)]
    let edges = tokio::task::spawn_blocking(move || {
        build_expanded_edges_with(
            &flow,
            &topology,
            &|t| formats.get(t).copied(),
            now,
            &source,
            &namespace,
        )
    })
    .await
    .map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("expand build failed: {e}"),
        )
    })?;

    Ok(axum::Json(ExpandedFold {
        source: params.source,
        namespace: params.namespace,
        edges,
        generated_at_ms: now,
    }))
}

/// Per-publisher pub/sub-hygiene report (the lazy, full path). Runs the
/// key-shape classifier AND a live storage-`get` probe over the publisher's
/// fan-out templates to confirm retrievability. Fired on demand when the
/// operator opens a publisher's side panel — not per snapshot. See
/// docs/design/pubsub-hygiene-diagnostic.md.
#[derive(Deserialize)]
struct HygieneQuery {
    source: String,
}

async fn publisher_hygiene(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<HygieneQuery>,
) -> Result<axum::Json<panoscope_core::PublisherHygiene>, (axum::http::StatusCode, String)> {
    let scope = app.viewer_scope(&headers).await;
    if !scope.allows_peer_inspect(&q.source)
        && !scope.allows_synthetic_key(peer_is_synthetic(&app.shared, &q.source).await)
    {
        return Err((
            axum::http::StatusCode::FORBIDDEN,
            "viewer scope does not cover this publisher (ADR 1073/1074)".to_string(),
        ));
    }
    // Served from the shared view model, which already holds every
    // publisher's canonical topic records. This used to rebuild the whole
    // raw edge graph (every flow edge against every peer's subscriptions)
    // while holding the topology, flow and sample-buffer read locks, which on a
    // large fabric starves a tokio worker, with the data tail unable to take
    // its write locks for the whole time. Now it takes no ingest lock.
    let model = app.shared.view_model.read().await.clone();
    let mut report = view::service::publisher_hygiene(&model, &q.source);
    // Live retrievability probe: does any storage answer a bounded `get` on
    // the publisher's fan-out templates? Confirm-only (a reply ⇒ Confirmed;
    // silence ⇒ Unconfirmed, never "no storage" — we can't prove absence).
    // The probe budget (SharedState::fabric_probes) caps how many of these
    // client-triggered `get`s can be at the router at once. `try_acquire`, not
    // `acquire`: when the budget is spent we SKIP the probe and leave
    // `retrievability` at its pre-probe value — which is `Unconfirmed`, the
    // state this endpoint already uses for "no storage answered in the
    // window". Waiting instead would turn fabric pressure into held HTTP
    // connections, which is the same wedge one layer up.
    if !report.probe_selectors.is_empty() {
        let session = app.shared.session.read().await.clone();
        if let Some(session) = session {
            match app.shared.fabric_probes.clone().try_acquire_owned() {
                Ok(_permit) => {
                    let mut confirmed = false;
                    for sel in &report.probe_selectors {
                        if zenoh_client::storage_answers(&session, sel).await {
                            confirmed = true;
                            break;
                        }
                    }
                    report.retrievability = if confirmed {
                        panoscope_core::Retrievability::Confirmed
                    } else {
                        panoscope_core::Retrievability::Unconfirmed
                    };
                }
                Err(_) => {
                    // Budget spent. Honest degradation, not an error: the
                    // viewer sees the same "we could not confirm" they would
                    // see if storage stayed silent, and the router is spared.
                    debug!(
                        source = %q.source,
                        "hygiene retrievability probe skipped — fabric probe budget saturated"
                    );
                }
            }
        }
    }
    Ok(axum::Json(report))
}

/// Legend payload — what the frontend's bottom-strip legend panel uses
/// to render decoded color / sample-rate / resolver-status info. Stable
/// shape; new fields land in later sub-tasks without breaking existing keys.
#[derive(Serialize)]
struct Legend {
    /// Schema resolver status — always `family-map` (the compiled-in
    /// family-map floor; per-peer coloring rides schema-decls). See
    /// schema.rs for why this is no longer a portal-reachability verdict.
    schema_resolver_status: ResolverStatus,
    /// Number of distinct schema families currently recognized.
    schema_resolver_known_families: usize,
    /// Slot-alias registry health (ADR 1039 honesty arm). The legend strip
    /// renders this next to the schema resolver so "every peer shows a raw
    /// UUID" always comes with its reason attached — a missing API key
    /// (`disabled`), a portal that has never answered (`unreached`), a
    /// last-good registry after a blip (`stale`), or a goatnet that genuinely
    /// has no aliases assigned (`ok` with `aliases: 0`).
    alias_registry: panoscope_core::AliasRegistryHealth,
    /// Per-topic sampling drop counts (cumulative since start).
    sampling_drops: HashMap<String, u64>,
    /// EMA time constant in seconds — used by the frontend to label
    /// the legend's edge-thickness scale.
    ema_tau_secs: f64,
    /// WS coalescing tick (ms).
    ws_coalesce_ms: u64,
    /// Server-side wallclock (for legend "as of <time>" footer).
    generated_at_ms: u64,
    /// Origin of the goat-zenoh-portal (e.g.
    /// `https://portal.nova.netbird.nova.example`). Empty when the
    /// portal URL isn't configured. The frontend uses this to link out
    /// to `/schema.html?key=<topic>` on the portal.
    ///
    /// **Backwards-compat:** preserved alongside the newer `siblings`
    /// field below. The frontend prefers `siblings.portal` when present
    /// and falls back to this field — see Block 94 cross-surface
    /// deep-link contract (`docs/design/zenoh-surface-deep-links.md`).
    portal_base_url: String,
    /// Sibling-surface base URLs (Block 94 cross-surface deep-link
    /// contract). Each is `null` when no sibling is configured at this
    /// deployment; the frontend hides the corresponding outbound button
    /// rather than emitting a dead href. See
    /// `docs/design/zenoh-surface-deep-links.md`.
    siblings: crate::config::SiblingUrls,
    /// True when the decoded-stream view's portal client is configured
    /// (URL + API key both set). Frontend reads this on legend bootstrap
    /// to decide whether to render the Decoded toggle at all — when
    /// false, every topic would surface NO SCHEMA, so we save the round-
    /// trips and surface a single deploy-gap hint instead.
    decoder_enabled: bool,
    /// Display-only lexicon — the participant noun + fabric label in the
    /// goatnet-appropriate wording. The frontend reads this on legend
    /// bootstrap to relex JS-generated labels (side-panel rows, fold
    /// buttons, super-node badge). Static HTML is already substituted at
    /// serve time. See docs/design/portal-lexicon.md §"Panoscope".
    lexicon: crate::config::Lexicon,
}

async fn legend(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
) -> axum::Json<Legend> {
    let scope = app.viewer_scope(&headers).await;
    let flow = app.shared.flow.read().await;
    let resolver = &app.shared.schema_resolver;
    axum::Json(Legend {
        schema_resolver_status: resolver.status(),
        schema_resolver_known_families: resolver.known_family_count(),
        alias_registry: app.shared.alias_health.read().await.clone(),
        sampling_drops: scope.filter_sampling_drops(&flow.sampling_drops),
        ema_tau_secs: EMA_TAU_SECS,
        ws_coalesce_ms: app.ws_coalesce_ms,
        generated_at_ms: now_ms(),
        portal_base_url: app.portal_base_url.clone(),
        siblings: app.siblings.clone(),
        decoder_enabled: app.decoder.portal_enabled(),
        lexicon: app.lexicon.clone(),
    })
}

/// Web-native `goat top` (ADR 1046): the per-producer table, class loads,
/// and envelope BANDs, assembled server-side from the firehose buffer's
/// health/footprint beats and SCOPE-FILTERED per producer row (ADR 1073 —
/// a footprint names an app producer, the same class of information the
/// snapshot's node filter protects). Identity-free aggregates render for
/// every tier; `restricted: true` tells the frontend to show the
/// scoped-view note. See src/top_view.rs.
async fn top(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
) -> axum::Json<top_view::TopView> {
    let scope = app.viewer_scope(&headers).await;
    let now = now_ms();
    let view = {
        let samples = app.shared.samples.read().await;
        let topology = app.shared.topology.read().await;
        let aliases = app.shared.aliases.read().await;
        let envelopes = app.envelopes.read().await;
        let src = top_view::collect_sources(&samples);
        top_view::build_top_view(&scope, &src, &topology, &aliases, &envelopes, now)
        // Guards drop here, before serialization (same lock discipline
        // as /api/snapshot).
    };
    axum::Json(view)
}

/// Shared gate for the /who endpoints: identity-administration data,
/// admitted by platform group only (`PANOSCOPE_RBAC_WHO_GROUPS`, default
/// ADMINS,IAM-ADMIN). Explicit 403 — never a silent empty.
fn require_who(
    app: &AppState,
    headers: &axum::http::HeaderMap,
) -> Result<(), (axum::http::StatusCode, String)> {
    if app.rbac.allows_who(headers) {
        Ok(())
    } else {
        Err((
            axum::http::StatusCode::FORBIDDEN,
            "the identity roster requires an identity-administration role (ADR 1073)".to_string(),
        ))
    }
}

/// Web-native `goat who` roster (ADR 1047): one cached bulk portal
/// roster + the live presence/footprint/band overlay. Group-gated.
async fn who_roster(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<axum::Json<who_view::WhoView>, (axum::http::StatusCode, String)> {
    require_who(&app, &headers)?;
    let now = now_ms();

    // Refresh the shared cache when past TTL; keep last-good on failure.
    let (roster, portal_state, roster_age) = {
        let cache = app.roster_cache.read().await;
        if cache.roster.is_some() && now.saturating_sub(cache.fetched_at_ms) < ROSTER_CACHE_TTL_MS {
            (cache.roster.clone().unwrap(), "ok", None)
        } else {
            drop(cache);
            match app.portal.access_roster().await {
                Ok(r) => {
                    let mut cache = app.roster_cache.write().await;
                    cache.roster = Some(r.clone());
                    cache.fetched_at_ms = now;
                    (r, "ok", None)
                }
                Err(panoscope_core::PortalError::Disabled) => (
                    panoscope_core::portal::AccessRoster::default(),
                    "disabled",
                    None,
                ),
                Err(e) => {
                    let cache = app.roster_cache.read().await;
                    match &cache.roster {
                        // Serve last-good, honestly aged — never blank on a blip.
                        Some(r) => {
                            let age = now.saturating_sub(cache.fetched_at_ms) as i64;
                            (r.clone(), "stale", Some(health_wire::fmt_age(age)))
                        }
                        None => {
                            return Err((
                                axum::http::StatusCode::BAD_GATEWAY,
                                format!("portal roster unavailable: {e}"),
                            ))
                        }
                    }
                }
            }
        }
    };

    let view = {
        let samples = app.shared.samples.read().await;
        let aliases = app.shared.aliases.read().await;
        let envelopes = app.envelopes.read().await;
        let src = top_view::collect_sources(&samples);
        who_view::build_who_view(&roster, portal_state, roster_age, &src, &aliases, &envelopes, now)
    };
    Ok(axum::Json(view))
}

#[derive(Deserialize)]
struct WhoDetailQuery {
    id: String,
}

/// Per-identity drill-down (`GET /api/access/who` proxy) for the /who
/// detail pane. Same group gate as the roster.
async fn who_detail(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<WhoDetailQuery>,
) -> Result<axum::Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    require_who(&app, &headers)?;
    // COP-to-lens deep link (ADR 1090, Block 94): when a microscope sibling is
    // configured, hand the roster its `/entity?id=<anchor>` link so the /who
    // detail can offer a `microscope ↗` jump. `None` (no sibling) → no button.
    let microscope_link = app.siblings.microscope.as_ref().map(|base| {
        let id_enc: String = q
            .id
            .bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect();
        format!("{}/entity?id={}", base.trim_end_matches('/'), id_enc)
    });
    match app.portal.access_who(&q.id).await {
        Ok(w) => Ok(axum::Json(serde_json::json!({
            "resolved": w.resolved,
            "kind": w.kind,
            "state": w.state,
            "affiliation": w.affiliation,
            "nation": w.nation,
            "recon_reason": w.recon_reason,
            "on_mesh": w.on_mesh,
            "microscope_link": microscope_link,
            "writes": w.writes.iter().map(|e| serde_json::json!({
                "topic": e.topic, "layer": e.layer, "reason": e.reason,
            })).collect::<Vec<_>>(),
            "reads": w.reads.iter().map(|e| serde_json::json!({
                "topic": e.topic, "layer": e.layer, "reason": e.reason,
            })).collect::<Vec<_>>(),
            "notes": w.notes,
        }))),
        Err(panoscope_core::PortalError::Disabled) => Err((
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "portal not configured at this deployment".to_string(),
        )),
        Err(e) => Err((
            axum::http::StatusCode::BAD_GATEWAY,
            format!("portal access query failed: {e}"),
        )),
    }
}

/// Access COP catalog: the org list with alias join + a viewer-scoped
/// live-rate rollup (docs/design/access-cop-web.md §3-4). Catalog
/// METADATA answers for every authenticated viewer (ADR 1028 — topic
/// visibility is advisory); only the live overlay is scope-gated.
async fn access_catalog(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<axum::Json<access_view::CatalogView>, (axum::http::StatusCode, String)> {
    let scope = app.viewer_scope(&headers).await;
    let now = now_ms();

    let orgs = {
        let cache = app.orgs_cache.read().await;
        if cache.orgs.is_some() && now.saturating_sub(cache.fetched_at_ms) < ORGS_CACHE_TTL_MS {
            cache.orgs.clone().unwrap()
        } else {
            drop(cache);
            match app.portal.list_vendors().await {
                Ok(rows) => {
                    let mut cache = app.orgs_cache.write().await;
                    cache.orgs = Some(rows.clone());
                    cache.fetched_at_ms = now;
                    rows
                }
                Err(panoscope_core::PortalError::Disabled) => {
                    // Honest deploy-gap state: the catalog needs the portal.
                    return Ok(axum::Json(access_view::CatalogView {
                        portal_state: "disabled",
                        orgs: vec![],
                        labels_available: false,
                        generated_at_ms: now,
                    }));
                }
                Err(e) => {
                    let cache = app.orgs_cache.read().await;
                    match &cache.orgs {
                        Some(rows) => rows.clone(), // last-good on a blip
                        None => {
                            return Err((
                                axum::http::StatusCode::BAD_GATEWAY,
                                format!("portal org listing unavailable: {e}"),
                            ))
                        }
                    }
                }
            }
        }
    };

    let view = {
        let flow = app.shared.flow.read().await;
        let aliases = app.shared.aliases.read().await;
        let rates = access_view::live_rates(&flow, now);
        access_view::build_catalog(&scope, &orgs, &aliases, &rates, now)
    };
    Ok(axum::Json(view))
}

#[derive(Deserialize)]
struct CatalogTopicsQuery {
    org: String,
}

/// One org's declared topic rows + viewer-scoped live rates. Lazy per
/// expanded org (no N-orgs fan-out); metadata advisory, rates scoped.
async fn access_catalog_topics(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<CatalogTopicsQuery>,
) -> Result<axum::Json<access_view::CatalogTopicsView>, (axum::http::StatusCode, String)> {
    let scope = app.viewer_scope(&headers).await;
    let now = now_ms();
    let topics = match app.portal.list_topics(&q.org).await {
        Ok(t) => t,
        Err(panoscope_core::PortalError::Disabled) => {
            return Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "portal not configured at this deployment".to_string(),
            ))
        }
        Err(e) => {
            return Err((
                axum::http::StatusCode::BAD_GATEWAY,
                format!("portal topic listing failed: {e}"),
            ))
        }
    };
    let view = {
        let flow = app.shared.flow.read().await;
        let rates = access_view::live_rates(&flow, now);
        access_view::build_catalog_topics(&scope, &q.org, &topics, &rates, now)
    };
    Ok(axum::Json(view))
}

#[derive(Deserialize)]
struct WhyQuery {
    id: String,
    key: String,
}

/// Why-trace proxy (`GET /api/access/why` at the portal) — the
/// troubleshooting workhorse. Cross-identity read ⇒ who_groups gate.
async fn access_query_why(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<WhyQuery>,
) -> Result<axum::Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    require_who(&app, &headers)?;
    match app.portal.access_why(&q.id, &q.key).await {
        Ok(w) => Ok(axum::Json(serde_json::json!({
            "query": w.query, "topic": w.topic, "resolved": w.resolved,
            "write_verdict": w.write_verdict, "read_verdict": w.read_verdict,
            "steps": w.steps.iter().map(|s| serde_json::json!({
                "layer": s.layer, "verdict": s.verdict, "reason": s.reason,
            })).collect::<Vec<_>>(),
            "notes": w.notes,
        }))),
        Err(panoscope_core::PortalError::Disabled) => Err((
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "portal not configured at this deployment".to_string(),
        )),
        Err(e) => Err((
            axum::http::StatusCode::BAD_GATEWAY,
            format!("portal why query failed: {e}"),
        )),
    }
}

#[derive(Deserialize)]
struct TopicQuery {
    key: String,
}

/// Reverse topic query proxy (who can write/read this key) — operator-
/// tier at the portal, who_groups-gated here.
async fn access_query_topic(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<TopicQuery>,
) -> Result<axum::Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    require_who(&app, &headers)?;
    match app.portal.access_topic(&q.key).await {
        Ok(t) => Ok(axum::Json(serde_json::json!({
            "topic": t.topic, "found": t.found,
            "writers": t.writers.iter().map(|w| serde_json::json!({
                "identity": w.identity, "kind": w.kind, "layer": w.layer, "reason": w.reason,
            })).collect::<Vec<_>>(),
            "readers": t.readers.iter().map(|r| serde_json::json!({
                "identity": r.identity, "layer": r.layer, "reason": r.reason,
            })).collect::<Vec<_>>(),
            "receivers": t.receivers,
            "notes": t.notes,
        }))),
        Err(panoscope_core::PortalError::Disabled) => Err((
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "portal not configured at this deployment".to_string(),
        )),
        Err(e) => Err((
            axum::http::StatusCode::BAD_GATEWAY,
            format!("portal topic query failed: {e}"),
        )),
    }
}

#[derive(Deserialize, Default)]
struct WsQuery {
    /// `snapshot=0`: skip the opening full snapshot. Pages that draw from
    /// /api/view send it; the snapshot build is the expensive part of a
    /// connect, and they would discard it.
    #[serde(default)]
    snapshot: Option<u8>,
}

async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<WsQuery>,
) -> impl IntoResponse {
    // Scope is fixed at upgrade time from the sidecar-injected headers —
    // the socket carries no further identity, so the whole stream lives
    // at the scope the viewer authenticated with (ADR 1073).
    let scope = app.viewer_scope(&headers).await;
    let send_snapshot = q.snapshot != Some(0);
    ws.on_upgrade(move |socket| ws::handle_socket(socket, app.shared, app.ws_coalesce_ms, scope, send_snapshot))
}

#[derive(Serialize)]
struct SamplesResponse {
    topic: String,
    count: usize,
    entries: Vec<sample_buffer::SampleEntry>,
}

#[derive(Deserialize)]
struct SamplesQuery {
    topic: String,
    #[serde(default = "default_sample_limit")]
    limit: usize,
}

fn default_sample_limit() -> usize {
    25
}

async fn samples(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<SamplesQuery>,
) -> Result<axum::Json<SamplesResponse>, (axum::http::StatusCode, String)> {
    let scope = app.viewer_scope(&headers).await;
    if !scope.allows_key(&q.topic)
        && !scope.allows_synthetic_key(topic_is_synthetic(&app.shared, &q.topic).await)
    {
        return Err((
            axum::http::StatusCode::FORBIDDEN,
            "viewer scope does not cover this topic (ADR 1073/1074)".to_string(),
        ));
    }
    let mut entries = {
        let buf = app.shared.samples.read().await;
        buf.recent(&q.topic, q.limit)
    };
    // Empty live ring → on-demand stale-recall from the router's Zenoh
    // storage (panoscope-stale-recall B1). A bounded one-shot `get`; yields
    // at most one entry, flagged `stale`. None when storage has nothing
    // (unregistered prefix / aged past the 7-day TTL / never published),
    // in which case the frontend keeps today's `[no samples yet…]`.
    //
    // Budget-gated (SharedState::fabric_probes). This is the recall most worth
    // capping: a topic that never publishes has a permanently-empty ring, so
    // EVERY view of it falls through to storage. When the budget is spent we
    // skip and keep the empty state the comment above already describes.
    // A group label or wildcard is not a key a storage can answer; skip the
    // recall without spending probe budget (see `is_recallable_topic`).
    if entries.is_empty() && zenoh_client::is_recallable_topic(&q.topic) {
        let session = app.shared.session.read().await.clone();
        if let Some(session) = session {
            match app.shared.fabric_probes.clone().try_acquire_owned() {
                Ok(_permit) => {
                    if let Some(stale) =
                        zenoh_client::get_last_retained(&session, &q.topic).await
                    {
                        entries.push(stale);
                    }
                }
                Err(_) => {
                    debug!(
                        topic = %q.topic,
                        "stale-recall skipped — fabric probe budget saturated"
                    );
                }
            }
        }
    }
    Ok(axum::Json(SamplesResponse {
        topic: q.topic,
        count: entries.len(),
        entries,
    }))
}

/// Decoded-stream view companion to `/api/samples`. Same ring buffer (with
/// the same empty-ring stale-recall fallback), but each sample's raw bytes
/// get replayed through the protobuf descriptor fetched from the portal
/// (cached after first hit). When no schema is available for the topic,
/// returns status=`no_schema` with an empty samples list — the frontend
/// renders the NO SCHEMA badge. See decoder.rs for the cache discipline.
async fn samples_decoded(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<SamplesQuery>,
) -> Result<axum::Json<TopicDecodeOutcome>, (axum::http::StatusCode, String)> {
    let scope = app.viewer_scope(&headers).await;
    if !scope.allows_key(&q.topic)
        && !scope.allows_synthetic_key(topic_is_synthetic(&app.shared, &q.topic).await)
    {
        return Err((
            axum::http::StatusCode::FORBIDDEN,
            "viewer scope does not cover this topic (ADR 1073/1074)".to_string(),
        ));
    }
    // Take a cheap snapshot of the recent samples then release the
    // ring-buffer lock before doing the (potentially network-bound) decode
    // resolution — the buffer write path on data_tail should never wait
    // on a portal fetch.
    let snapshot = {
        let buf = app.shared.samples.read().await;
        buf.recent(&q.topic, q.limit)
    };
    // Empty live ring → recall the single most-recent retained sample from
    // Zenoh storage and decode that (flagged `stale`) so a quiet topic's
    // last decoded message is still inspectable across panoscope restarts
    // (panoscope-stale-recall B1). Falls through to today's empty decode
    // outcome when storage has nothing.
    //
    // Budget-gated exactly as `/api/samples` is. Gating one recall and not the
    // other would be no gate at all: the Decoded toggle hits the same storage
    // over the same topics, so an operator with it enabled would simply route
    // around the cap.
    let snapshot = if snapshot.is_empty() && zenoh_client::is_recallable_topic(&q.topic) {
        let session = app.shared.session.read().await.clone();
        match session {
            Some(session) => match app.shared.fabric_probes.clone().try_acquire_owned() {
                Ok(_permit) => zenoh_client::get_last_retained(&session, &q.topic)
                    .await
                    .map(|e| vec![e])
                    .unwrap_or_default(),
                Err(_) => {
                    debug!(
                        topic = %q.topic,
                        "decoded stale-recall skipped — fabric probe budget saturated"
                    );
                    snapshot
                }
            },
            None => snapshot,
        }
    } else {
        snapshot
    };
    let outcome = app.decoder.decode_topic_samples(&q.topic, &snapshot).await;
    Ok(axum::Json(outcome))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_decoder() -> Arc<Decoder> {
        Decoder::new(PortalClient::new(String::new(), String::new()))
    }

    fn test_envelopes() -> Arc<tokio::sync::RwLock<panoscope_core::envelope::EnvelopeRegistry>> {
        Arc::new(tokio::sync::RwLock::new(
            panoscope_core::envelope::EnvelopeRegistry::empty(),
        ))
    }

    /// Disabled portal client (empty url + key) — /who then reports the
    /// deploy-gap "disabled" state.
    fn test_portal() -> Arc<PortalClient> {
        Arc::new(PortalClient::new(String::new(), String::new()))
    }

    fn test_state() -> AppState {
        AppState {
            shared: SharedState::default(),
            ws_coalesce_ms: crate::config::DEFAULT_WS_COALESCE_MS,
            portal_base_url: String::new(),
            siblings: crate::config::SiblingUrls::default(),
            decoder: test_decoder(),
            lexicon: crate::config::Lexicon::default(),
            rbac: Arc::new(RbacConfig::default()),
            envelopes: test_envelopes(),
            portal: test_portal(),
            roster_cache: Arc::new(tokio::sync::RwLock::new(RosterCache::default())),
            orgs_cache: Arc::new(tokio::sync::RwLock::new(OrgsCache::default())),
            data_roles_cache: Arc::new(tokio::sync::RwLock::new(DataRolesCache::default())),
        }
    }

    fn test_router() -> Router {
        build_router(
            SharedState::default(),
            crate::config::DEFAULT_WS_COALESCE_MS,
            String::new(),
            crate::config::SiblingUrls::default(),
            test_decoder(),
            crate::config::Lexicon::default(),
            Arc::new(RbacConfig::default()),
            test_envelopes(),
            test_portal(),
        )
    }

    /// A router over a SharedState holding one chunk-streaming publisher, with
    /// the view model built once.
    async fn view_router() -> (Router, SharedState) {
        let shared = SharedState::default();
        {
            let mut t = shared.topology.write().await;
            let mut f = shared.flow.write().await;
            let now = now_ms();
            for c in 0..40 {
                for p in 0..3 {
                    let key = format!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/imagery/v1/stream/0001/chunk/{c:04}/{p:06}");
                    t.record_publish_declaration("chunker".into(), key.clone(), now);
                    f.record_event("chunker".into(), key, now, 5.0);
                }
            }
            t.record_subscribe_declaration("viewer".into(), "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/**".into(), now);
        }
        let index = Arc::new(std::sync::Mutex::new(crate::view::subs::SubscriptionIndex::default()));
        crate::view::service::rebuild(&shared, &index).await.unwrap();
        let router = build_router(
            shared.clone(),
            crate::config::DEFAULT_WS_COALESCE_MS,
            String::new(),
            crate::config::SiblingUrls::default(),
            test_decoder(),
            crate::config::Lexicon::default(),
            Arc::new(RbacConfig::default()),
            test_envelopes(),
            test_portal(),
        );
        (router, shared)
    }

    fn post_json(uri: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn json_of(r: axum::response::Response) -> serde_json::Value {
        serde_json::from_slice(&axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn view_answers_with_an_etag_and_304_when_unchanged() {
        let (app, _) = view_router().await;
        // An unwindowed view, so the tag cannot roll over a minute between
        // the two requests.
        let r = app.clone().oneshot(post_json("/api/view", serde_json::json!({"window_hours": 0}))).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let etag = r.headers().get("etag").unwrap().to_str().unwrap().to_string();
        let v = json_of(r).await;
        assert_eq!(v["totals"]["topics"], 120);
        assert!(v["groups"].as_array().unwrap().len() >= 1, "the chunk stream is grouped");
        let again = Request::builder()
            .method("POST")
            .uri("/api/view")
            .header("content-type", "application/json")
            .header("if-none-match", etag)
            .body(Body::from(r#"{"window_hours": 0}"#))
            .unwrap();
        assert_eq!(app.oneshot(again).await.unwrap().status(), StatusCode::NOT_MODIFIED);
    }

    #[tokio::test]
    async fn group_members_search_and_reveal_agree_with_the_view() {
        let (app, _) = view_router().await;
        let v = json_of(app.clone().oneshot(post_json("/api/view", serde_json::json!({}))).await.unwrap()).await;
        let gid = v["groups"][0]["id"].as_str().unwrap().to_string();
        let count = v["groups"][0]["topic_count"].as_u64().unwrap();

        let m = json_of(app.clone().oneshot(post_json("/api/view/group", serde_json::json!({"id": gid, "q": "chunk/0007"}))).await.unwrap()).await;
        assert_eq!(m["total"].as_u64().unwrap(), count);
        assert_eq!(m["matched"], 3);

        let s = json_of(app.clone().oneshot(post_json("/api/view/search", serde_json::json!({"q": "chunk/0007/000001"}))).await.unwrap()).await;
        assert_eq!(s["total"], 1);
        let key = s["hits"][0]["key"].as_str().unwrap().to_string();
        assert_eq!(s["hits"][0]["drawn_as"], gid);

        let rv = json_of(app.clone().oneshot(post_json("/api/view/reveal", serde_json::json!({"key": key, "publisher": "chunker"}))).await.unwrap()).await;
        let expand = rv["expand"].as_array().unwrap().clone();
        assert!(!expand.is_empty());
        assert_eq!(rv["topic_id"], format!("topic:{key}"));
        let opened = json_of(app.oneshot(post_json("/api/view", serde_json::json!({"expand": expand}))).await.unwrap()).await;
        assert!(
            opened["topics"].as_array().unwrap().iter().any(|t| t["key"] == key),
            "after opening the reveal chain the topic is its own pin"
        );

    }

    #[tokio::test]
    async fn topics_lists_every_key_ungrouped_and_filters_by_key_expression() {
        let (app, _) = view_router().await;
        let get = |uri: &str| Request::builder().uri(uri).body(Body::empty()).unwrap();
        let all = json_of(app.clone().oneshot(get("/api/topics")).await.unwrap()).await;
        assert_eq!(all["total"], 120, "every chunk part is its own row, never a group");
        assert_eq!(all["truncated"], false);
        let row = &all["topics"][0];
        assert_eq!(row["publishers"], serde_json::json!(["chunker"]));
        assert_eq!(row["subscriber_count"], 1);

        let one = json_of(
            app.clone()
                .oneshot(get("/api/topics?key=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/imagery/v1/stream/0001/chunk/0007/*"))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(one["total"], 3);

        let capped = json_of(app.oneshot(get("/api/topics?limit=5")).await.unwrap()).await;
        assert_eq!(capped["topics"].as_array().unwrap().len(), 5);
        assert_eq!(capped["truncated"], true);
        assert_eq!(capped["total"], 120);
    }

    #[tokio::test]
    async fn unknown_group_is_404_not_an_empty_list() {
        let (app, _) = view_router().await;
        let r = app.oneshot(post_json("/api/view/group", serde_json::json!({"id": "grp:nobody:pub"}))).await.unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn healthz_returns_ok() {
        let app = test_router();
        let response = app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn version_returns_crate_name() {
        let app = test_router();
        let response = app
            .oneshot(Request::builder().uri("/version").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(v["name"], "panoscope");
        assert!(v["version"].as_str().unwrap().starts_with("0.0.1"));
    }

    #[tokio::test]
    async fn stats_endpoint_returns_initialized_state() {
        let app = test_router();
        let response = app
            .oneshot(Request::builder().uri("/api/stats").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let s: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(s["data_samples_seen"], 0);
        assert_eq!(s["admin_samples_seen"], 0);
        assert_eq!(s["data_samples_dropped_backpressure"], 0);
    }

    #[tokio::test]
    async fn snapshot_returns_empty_when_no_zenoh_traffic() {
        let app = test_router();
        let response = app
            .oneshot(Request::builder().uri("/api/snapshot").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let s: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert!(s["nodes"].as_array().unwrap().is_empty());
        assert!(s["edges"].as_array().unwrap().is_empty());
        assert!(s["generated_at_ms"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn snapshot_reflects_recorded_events() {
        let state = test_state();
        {
            let mut topology = state.shared.topology.write().await;
            topology.record_publish_declaration(
                "peer-aaaaaaaa".into(),
                "vendor-b/sensorgrid/raw".into(),
                1000,
            );
        }
        {
            let mut flow = state.shared.flow.write().await;
            flow.record_event(
                "peer-aaaaaaaa".into(),
                "vendor-b/sensorgrid/raw".into(),
                1000,
                EMA_TAU_SECS,
            );
            flow.record_event(
                "peer-aaaaaaaa".into(),
                "vendor-b/sensorgrid/raw".into(),
                1100,
                EMA_TAU_SECS,
            );
        }

        let app = Router::new()
            .route("/api/snapshot", get(snapshot))
            .with_state(state);
        let response = app
            .oneshot(Request::builder().uri("/api/snapshot").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let s: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let nodes = s["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0]["peer_id"], "peer-aaaaaaaa");
        assert_eq!(nodes[0]["role"], "source");
        assert_eq!(nodes[0]["publishes"][0], "vendor-b/sensorgrid/raw");
        let edges = s["edges"].as_array().unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0]["total_events"], 2);
    }

    #[tokio::test]
    async fn legend_returns_known_field_set() {
        let app = test_router();
        let response = app
            .oneshot(Request::builder().uri("/api/legend").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let l: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        // SharedState::default() ⇒ FamilyMapResolver
        assert_eq!(l["schema_resolver_status"], "family-map");
        // The legend reports what the resolver knows: the compiled-in floor
        // (product namespaces only: cot, community, caracal, notional) plus
        // whatever the deployment adds in PANOSCOPE_SCHEMA_FAMILIES.
        let floor: std::collections::HashSet<String> = crate::schema::embedded_mappings()
            .into_iter()
            .map(|m| m.family)
            .collect();
        assert_eq!(floor.len(), 4);
        let known = crate::schema::FamilyMapResolver::new().known_family_count();
        assert!(known >= floor.len());
        assert_eq!(l["schema_resolver_known_families"], known);
        // Slot-alias honesty arm (ADR 1039). A fresh SharedState has never
        // polled, so the legend must say `unreached` — NOT `ok` with zero
        // aliases, which would read as "this goatnet assigns no names" and
        // is precisely the conflation the arm exists to break.
        assert_eq!(l["alias_registry"]["state"], "unreached");
        assert_eq!(l["alias_registry"]["aliases"], 0);
        assert_eq!(l["ema_tau_secs"], EMA_TAU_SECS);
        assert_eq!(l["ws_coalesce_ms"], crate::config::DEFAULT_WS_COALESCE_MS);
        assert!(l["generated_at_ms"].as_u64().unwrap() > 0);
        assert!(l["sampling_drops"].is_object());
        // Block 94 cross-surface deep-link contract: `siblings` is
        // always present (object with optional inner fields), distinct
        // from the legacy `portal_base_url` flat field.
        assert!(l["siblings"].is_object());
        // Display lexicon — default wording surfaces for the
        // frontend to relex JS-generated labels.
        assert_eq!(l["lexicon"]["org"], "vendor");
        assert_eq!(l["lexicon"]["orgs"], "vendors");
        assert_eq!(l["lexicon"]["fabric_label"], "Zenoh fabric");
    }

    #[tokio::test]
    async fn legend_carries_explicit_sibling_urls() {
        // Sibling URLs configured at boot surface verbatim on /api/legend
        // so the frontend can render outbound diagnose/tail buttons.
        let app = build_router(
            SharedState::default(),
            crate::config::DEFAULT_WS_COALESCE_MS,
            "https://portal.example".to_string(),
            crate::config::SiblingUrls {
                stethoscope: Some("https://stethoscope.example".to_string()),
                flashlight: Some("https://flashlight.example".to_string()),
                microscope: None,
                portal: Some("https://portal.example".to_string()),
            },
            test_decoder(),
            crate::config::Lexicon::default(),
            Arc::new(RbacConfig::default()),
            test_envelopes(),
            test_portal(),
        );
        let response = app
            .oneshot(Request::builder().uri("/api/legend").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let l: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(l["siblings"]["stethoscope"], "https://stethoscope.example");
        assert_eq!(l["siblings"]["flashlight"], "https://flashlight.example");
        assert_eq!(l["siblings"]["portal"], "https://portal.example");
        // Legacy field stays populated for backwards-compat with any
        // frontend that hasn't switched to reading `siblings.portal` yet.
        assert_eq!(l["portal_base_url"], "https://portal.example");
    }

    // panoscope-stale-recall B1: the empty-ring storage fallback must
    // degrade safely when no Zenoh session is wired (the test harness, and
    // the window before `run` opens the session). With session=None the
    // handler skips the `get` and returns a genuine empty — no hang, no
    // panic. The live storage-`get` path is exercised in deployment soak,
    // not unit tests (it needs a router with rocksdb storage).
    #[tokio::test]
    async fn samples_empty_ring_no_session_returns_genuine_empty() {
        let app = test_router(); // SharedState::default() → session None
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/samples?topic=drs/r/RADAR/tracks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(v["count"], 0);
        assert_eq!(v["entries"].as_array().unwrap().len(), 0);
    }

    fn enforce_router() -> Router {
        let rbac = RbacConfig {
            mode: viewer::RbacMode::Enforce,
            ..RbacConfig::default()
        };
        build_router(
            SharedState::default(),
            crate::config::DEFAULT_WS_COALESCE_MS,
            String::new(),
            crate::config::SiblingUrls::default(),
            test_decoder(),
            crate::config::Lexicon::default(),
            Arc::new(rbac),
            test_envelopes(),
            test_portal(),
        )
    }

    // ADR 1073: enforce mode + no identity headers fails closed — an
    // out-of-scope topic read is an explicit 403, never a silent empty.
    #[tokio::test]
    async fn enforce_mode_samples_forbidden_without_groups() {
        let app = enforce_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/samples?topic=someorg/telemetry/v1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    // Access COP gates (docs/design/access-cop-web.md §3): catalog
    // metadata answers every authenticated viewer even at the floor
    // (ADR 1028 advisory visibility; disabled test portal reports the
    // honest deploy-gap state), while the cross-identity query proxies
    // are who_groups-gated exactly like /who.
    #[tokio::test]
    async fn access_catalog_answers_floor_but_queries_gate() {
        let app = enforce_router();
        // Floor viewer (no groups): catalog answers with the disabled
        // portal state — metadata tier, not identity tier.
        let response = app
            .clone()
            .oneshot(Request::builder().uri("/api/access/catalog").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["portal_state"], "disabled");
        // Cross-identity query proxies: 403 without an identity-admin
        // group — data.view.all does not qualify.
        for (uri, groups) in [
            ("/api/access/query/why?id=x&key=y", None),
            ("/api/access/query/why?id=x&key=y", Some("data.view.all")),
            ("/api/access/query/topic?key=y", None),
            ("/api/access/query/topic?key=y", Some("data.view.all")),
        ] {
            let mut req = Request::builder().uri(uri);
            if let Some(g) = groups {
                req = req.header("X-Auth-Request-Groups", g);
            }
            let response = app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "uri={uri} groups={groups:?}");
        }
        // An identity-admin group passes the gate; the disabled test
        // portal then reports 503 (deploy gap), not 403.
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/access/query/topic?key=y")
                    .header("X-Forwarded-Groups", "IAM-ADMIN")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    // ADR 1073: the /who roster is identity-administration data. Enforce
    // mode + no identity ⇒ explicit 403 (fail-closed, F-177). Note the
    // data-access apex grant deliberately does NOT admit it — mission-data
    // visibility and identity administration are orthogonal axes.
    #[tokio::test]
    async fn enforce_mode_who_roster_forbidden_without_admin_group() {
        let app = enforce_router();
        for (uri, groups) in [
            ("/api/who/roster", None),
            ("/api/who/roster", Some("data.view.all")),
            ("/api/who/roster", Some("OBS-READONLY")),
            ("/api/who/detail?id=abc123", None),
            ("/api/who/detail?id=abc123", Some("data.view.all")),
        ] {
            let mut req = Request::builder().uri(uri);
            if let Some(g) = groups {
                req = req.header("X-Auth-Request-Groups", g);
            }
            let response = app
                .clone()
                .oneshot(req.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "uri={uri} groups={groups:?}"
            );
        }
    }

    // The identity-administration groups (default ADMINS + IAM-ADMIN) pass
    // the /who gate. With the test portal disabled the handler reports the
    // honest deploy-gap state rather than a fake empty roster.
    #[tokio::test]
    async fn enforce_mode_who_roster_admits_identity_admin_groups() {
        let app = enforce_router();
        for g in ["ADMINS", "IAM-ADMIN"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/who/roster")
                        .header("X-Forwarded-Groups", g)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "group={g}");
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(v["portal_state"], "disabled");
            assert_eq!(v["rows"].as_array().unwrap().len(), 0);
        }
    }

    // ADR 1073 on /api/top: the endpoint answers every authenticated tier
    // (the weather/class aggregates are identity-free), but a floor viewer
    // gets zero producer rows and an explicit restricted marker — producer
    // identities are scoped information even though the carrier key is
    // floor-visible ops/fabric/footprint/**.
    #[tokio::test]
    async fn enforce_mode_top_floor_gets_aggregates_not_producer_rows() {
        let app = enforce_router();
        let response = app
            .clone()
            .oneshot(Request::builder().uri("/api/top").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["scope_tier"], "floor");
        assert_eq!(v["restricted"], true);
        assert_eq!(v["producers"].as_array().unwrap().len(), 0);
        // Admin keeps the unrestricted view.
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/top")
                    .header("X-Forwarded-Groups", "ADMINS")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["scope_tier"], "full");
        assert_eq!(v["restricted"], false);
    }

    // ADR 1073: an admin-group viewer keeps the full view in enforce mode,
    // and observer-tier viewers still read the common ops/ namespaces.
    #[tokio::test]
    async fn enforce_mode_admin_group_reads_any_topic_observer_reads_ops() {
        let app = enforce_router();
        // F-244: authorization reads the groups claim from X-Forwarded-Groups
        // ONLY — the header oauth2-proxy strips-and-sets. X-Auth-Request-* is
        // client-forgeable in reverse-proxy mode and must never grant scope
        // (regression-locked in enforce_mode_forged_x_auth_request_is_floored).
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/samples?topic=someorg/telemetry/v1")
                    .header("X-Forwarded-Groups", "ADMINS")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/samples?topic=ops/fabric/health/site-a")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "observer floor keeps ops/ health readable");
    }

    // F-244 regression: a logged-in FLOOR viewer forging the client-controllable
    // X-Auth-Request-Groups header must NOT escalate — the router ignores it and
    // the request is scoped to the floor (a real producer namespace is 403).
    #[tokio::test]
    async fn enforce_mode_forged_x_auth_request_is_floored() {
        let app = enforce_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/samples?topic=someorg/telemetry/v1")
                    .header("X-Auth-Request-Groups", "ADMINS")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "forged X-Auth-Request-Groups must not grant scope"
        );
    }

    #[tokio::test]
    async fn samples_decoded_empty_ring_no_session_returns_empty_outcome() {
        let app = test_router(); // portal disabled + session None
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/samples/decoded?topic=drs/r/RADAR/tracks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        // Portal disabled in the harness → PortalDisabled, empty samples.
        assert_eq!(v["status"], "portal_disabled");
        assert_eq!(v["samples"].as_array().unwrap().len(), 0);
    }
}
