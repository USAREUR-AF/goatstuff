// SPDX-License-Identifier: Apache-2.0
//
// flashlight — focus-oriented inspection surface for the Zenoh
// fabric. URL declares the focused slice (?key=/?vendor=/?role=/?site=/
// ?since=/?until=/?format=/?compute=); server narrows its Zenoh
// subscription accordingly; Tier 1 HTML+SSE renders the live tail.
//
// Block 89 Phase C per docs/design/flashlight.md.
//
// This binary brings up the HTTP service with
// /healthz, /api/version, /api/parse, /api/plan, /api/stats, AND
// /events (SSE live tail, per-request narrowed). When
// FLASHLIGHT_ZENOH_ENDPOINT is set, a broad Zenoh subscription
// feeds a broadcast channel each SSE client re-narrows in-process.
// Brotli compression is wrapped at the router level.
//

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::Serialize;
use tokio::sync::broadcast;
use tower_http::compression::CompressionLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

mod compute;
mod directory;
mod filter;
mod keymatch;
mod render;
mod sample;
mod schema_decl;
mod sse;
mod store;
mod subscription;
mod supervisor;
mod wasm_assets;
mod zenoh_client;

use std::sync::RwLock;

use panoscope_core::portal::PortalClient;
use panoscope_core::topology::Topology;

use filter::{parse_filter, Filter, FilterError};
use store::SampleStore;
use subscription::{plan_subscription, SubscriptionPlan};
use zenoh_client::{SampleEvent, ZenohConfig};

const DEFAULT_BIND: &str = "0.0.0.0:8080";

/// Shared state surfaced to handlers. `events` is an `Arc<broadcast::Sender>`
/// so /events handlers can call `subscribe()` to get their own Receiver
/// for per-request filtering.
#[derive(Clone)]
pub(crate) struct AppState {
    /// Reconnectable live-subscription handle. `None` whenever there is no
    /// live subscription — before the first connect, and again for as long as
    /// a lost one is being rebuilt. The supervisor owns both transitions, so
    /// a boot-race and a mid-life router restart each
    /// self-heal without a manual container restart.
    ///
    /// Everything that reports flashlight's health reads this slot, so it
    /// emptying on loss is what keeps `/healthz` and `/api/stats` honest.
    pub events: Arc<RwLock<Option<Arc<broadcast::Sender<SampleEvent>>>>>,
    /// True when `FLASHLIGHT_ZENOH_ENDPOINT` is configured — i.e. flashlight
    /// is *supposed* to have a live subscription. Lets `/healthz` report
    /// degraded (503) when configured-but-not-subscribed (so isitup + the
    /// docker healthcheck stop false-greening an HTTP-only flashlight), while
    /// a deliberately endpoint-less deploy still reads healthy.
    pub zenoh_configured: bool,
    /// Bounded ring of recent samples' payload bytes, fed by the broad
    /// Zenoh subscriber and read by `GET /api/sample/{seq}`. Always
    /// present; empty (and so every lookup 404s) when no live
    /// subscription is running.
    pub store: Arc<SampleStore>,
    /// Portal registry client backing `GET /api/directory` (the
    /// navigate-anywhere catalog). Disabled (every call short-circuits)
    /// when `FLASHLIGHT_PORTAL_URL` / `FLASHLIGHT_PORTAL_API_KEY` are
    /// unset.
    pub portal: PortalClient,
    /// Peer topology hydrated from schema-declarations seen on the broad
    /// subscription. The SSE handler reads it to apply the
    /// `role`/`site`/`format` post-filters; empty until the first decl
    /// lands (so those filters drop unattributed events — see
    /// subscription::post_filters_pass).
    pub topology: Arc<RwLock<Topology>>,
    /// Per-key decode cache (declared format + FDS descriptor) for the
    /// payload decode view, so auto-follow refreshes don't re-hit the
    /// portal. See sample.rs `decode_ctx_for_key`.
    pub descriptor_cache:
        Arc<std::sync::Mutex<std::collections::HashMap<String, sample::DecodeCtx>>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .json()
        .init();

    let bind: SocketAddr = std::env::var("FLASHLIGHT_BIND")
        .unwrap_or_else(|_| DEFAULT_BIND.to_string())
        .parse()
        .context("FLASHLIGHT_BIND parse")?;

    // Optional Zenoh wiring: when FLASHLIGHT_ZENOH_ENDPOINT is set,
    // open a session and subscribe on `**` so the broadcast channel
    // has live samples for the SSE handler. The narrow-from-Filter
    // path stays available via /api/plan; each SSE client narrows per request.
    // Bounded sample ring shared between the broad Zenoh subscriber
    // (writer) and the `/api/sample/{seq}` handler (reader). Created
    // unconditionally so the route is always wired; stays empty when
    // no live subscription runs.
    let store = Arc::new(SampleStore::from_env());

    // Peer topology hydrated from schema-decls on the broad subscription;
    // read by the SSE handler for role/site/format post-filtering.
    let topology = Arc::new(RwLock::new(Topology::default()));

    // Reconnectable subscription slot — filled and emptied by the supervisor
    // task below, which retries the zenoh connect with backoff until it
    // establishes and then keeps doing so for every later loss. A boot-race
    // against the router and a router restart under a running
    // flashlight both self-heal instead of stranding it HTTP-only
    // until a manual restart. The HTTP surface comes up immediately
    // regardless; the slot fills in once connected.
    let events: Arc<RwLock<Option<Arc<broadcast::Sender<SampleEvent>>>>> =
        Arc::new(RwLock::new(None));
    let zenoh_configured = match ZenohConfig::from_env()? {
        Some(cfg) => {
            let slot = events.clone();
            let store_bg = store.clone();
            let topology_bg = topology.clone();
            // Subscribe broadly so the SSE handler can filter at sample
            // time; per-request narrowing is the SSE handler's job.
            let mut catchall = SubscriptionPlan::default();
            catchall.key_exprs.insert("**".to_string());
            // Supervised for the life of the process, not just until the
            // first connect succeeds: the supervisor rebuilds the
            // subscription whenever it stops delivering, and empties the
            // slot while it is gone so /healthz and /api/stats report the
            // real state. See supervisor.rs for the failure this answers.
            // clippy.toml forbids `tokio::spawn` for goat-relay's
            // shard-pinned hot path and names the exception this is: a
            // supervisor task that genuinely needs to run across the
            // multi-threaded runtime, allowed explicitly so the
            // deviation shows up in review. flashlight is not on that
            // hot path and has no LocalSet.
            #[allow(clippy::disallowed_methods)]
            tokio::spawn(supervisor::supervise(
                cfg,
                catchall,
                store_bg,
                topology_bg,
                slot,
            ));
            true
        }
        None => {
            info!("FLASHLIGHT_ZENOH_ENDPOINT unset — running HTTP-only (no live tail)");
            false
        }
    };

    // Portal client for the navigate-anywhere directory. Empty creds →
    // disabled client; `/api/directory` then reports `enabled: false`.
    let portal = PortalClient::new(
        std::env::var("FLASHLIGHT_PORTAL_URL").unwrap_or_default(),
        std::env::var("FLASHLIGHT_PORTAL_API_KEY").unwrap_or_default(),
    );
    if !portal.is_enabled() {
        info!("FLASHLIGHT_PORTAL_URL unset — directory disabled (jump box + browse will be empty)");
    }

    let state = AppState {
        events,
        zenoh_configured,
        store,
        portal,
        topology,
        descriptor_cache: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };

    let panoscope_origin = std::env::var("FLASHLIGHT_PANOSCOPE_ORIGIN").unwrap_or_default();
    // Block 94 cross-surface deep-link contract — sibling origin for
    // the `is this healthy? ↗` header link to stethoscope. Empty =
    // no button rendered (URL-only contract, no runtime coupling).
    let stethoscope_origin =
        std::env::var("FLASHLIGHT_SIBLING_STETHOSCOPE_URL").unwrap_or_default();
    let siblings = render::SiblingOrigins {
        panoscope: panoscope_origin,
        stethoscope: stethoscope_origin,
    };

    // Display-only lexicon (docs/design/portal-lexicon.md). Default
    // "vendor"; a goatnet overrides via FLASHLIGHT_LEX_ORG from the role.
    let lexicon = render::Lexicon::from_env();

    let app = Router::new()
        .route(
            "/",
            get(move |q| index_handler(q, siblings.clone(), lexicon.clone())),
        )
        .route("/healthz", get(healthz))
        .route("/api/version", get(version))
        .route("/api/parse", get(parse_endpoint))
        .route("/api/plan", get(plan_endpoint))
        .route("/api/stats", get(stats_endpoint))
        // Phase D Tier 2 server-side analytics — operator selects via
        // `?compute=server` when their device can't run WASM. Same
        // flashlight-analytics Engine the browser-side WASM module
        // wraps; native↔WASM byte-equal output is gated by the
        // wasm-bindgen-test runner in CI (#586).
        .route("/api/compute", axum::routing::post(compute::compute_endpoint))
        // Phase E Tier 2 — serves the wasm-bindgen output (`.wasm` +
        // `.js` glue) embedded by `rust-embed` at compile time. The
        // Dockerfile's `wasm-builder` stage produces the artifacts;
        // local non-Docker builds 404 here, which the Tier 1 page
        // tolerates (Tier 2 is opt-in via lazy-load).
        .route("/api/wasm/:filename", get(wasm_assets::wasm_asset_handler))
        // On-demand raw payload retrieval — the Tier 1 row-click viewer
        // fetches the bytes here so the live tail stays byte-free.
        .route("/api/sample/:seq", get(sample::sample_handler))
        // Navigate-anywhere catalog — vendors + topics from the portal,
        // backs the directory landing + header jump box.
        .route("/api/directory", get(directory::directory_handler))
        .route("/events", get(sse::events_handler))
        .with_state(state)
        // Brotli compression per docs/design/minimalism-under-network-constraints.md
        // Decision 1 (SSE = Brotli streaming). tower-http chooses the
        // best supported encoding per Accept-Encoding header.
        .layer(CompressionLayer::new().br(true).gzip(true));

    info!(addr = %bind, "flashlight listening");
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .context("bind tcp listener")?;
    axum::serve(listener, app).await.context("axum serve")?;
    Ok(())
}

async fn index_handler(
    Query(raw): Query<serde_json::Value>,
    siblings: render::SiblingOrigins,
    lexicon: render::Lexicon,
) -> axum::response::Response {
    let query = collapse_query(raw);
    let body = render::render_page(&query, &siblings, &lexicon);
    (
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}

/// Liveness + functional health. Returns 503 when a zenoh endpoint is
/// configured but no live subscription is established — so isitup + the
/// docker healthcheck reflect an HTTP-only flashlight as degraded instead of
/// false-greening it. 200 when subscribed, or when no endpoint is
/// configured (a deliberately HTTP-only deploy is healthy).
async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let subscribed = state
        .events
        .read()
        .ok()
        .map(|g| g.is_some())
        .unwrap_or(false);
    if state.zenoh_configured && !subscribed {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "degraded: zenoh endpoint configured but live subscription not established",
        );
    }
    (StatusCode::OK, "ok")
}

async fn version() -> String {
    format!("{} v{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
}

/// Echoes the parsed `Filter` for the given query string as JSON.
/// Useful as a developer-facing diagnostic — `curl
/// 'http://localhost:8080/api/parse?vendor=vendor-b&role=sensor'`
/// returns the structured Filter the SSE handler will subscribe on.
///
/// Returns 400 with `{"error": "..."}` on malformed input.
async fn parse_endpoint(Query(raw): Query<serde_json::Value>) -> impl IntoResponse {
    let query = collapse_query(raw);
    match parse_filter(&query) {
        Ok(f) => (StatusCode::OK, Json(ParseOk { filter: f })).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(ParseErr {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

/// Same shape as /api/parse but returns the `SubscriptionPlan` that
/// would be installed — visible diagnostic of "what would flashlight
/// subscribe on for this URL?".
async fn plan_endpoint(Query(raw): Query<serde_json::Value>) -> impl IntoResponse {
    let query = collapse_query(raw);
    match parse_filter(&query) {
        Ok(f) => {
            let plan = plan_subscription(&f);
            (StatusCode::OK, Json(PlanOk { filter: f, plan })).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(ParseErr {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

/// At-a-glance peek into the broadcast channel — whether a live
/// subscription is running, and how many receivers are currently
/// attached.
async fn stats_endpoint(State(state): State<AppState>) -> Json<Stats> {
    let (subscription_active, receiver_count) = match state.events.read().ok().and_then(|g| g.clone())
    {
        Some(tx) => (true, tx.receiver_count()),
        None => (false, 0),
    };
    Json(Stats {
        subscription_active,
        receiver_count,
        retained_samples: state.store.len(),
        last_sample_ms_ago: state.store.last_ingest_ms().map(|ms| {
            // Saturating: a clock step backwards should read 0, not wrap
            // to an enormous age that looks like a fault.
            panoscope_core::now_ms().saturating_sub(ms)
        }),
    })
}

/// Reconstruct the raw query string from axum's parsed Query map.
/// `axum::extract::Query<HashMap<String, String>>` collapses repeated
/// keys (defeating our OR-within semantics), so we round-trip via
/// `serde_json::Value` here so repeated `?vendor=` arrives intact.
pub(crate) fn collapse_query(raw: serde_json::Value) -> String {
    match raw {
        serde_json::Value::Object(map) => map
            .into_iter()
            .flat_map(|(k, v)| match v {
                serde_json::Value::String(s) => vec![format!("{k}={s}")],
                serde_json::Value::Array(arr) => arr
                    .into_iter()
                    .filter_map(|x| x.as_str().map(|s| format!("{k}={s}")))
                    .collect(),
                _ => vec![],
            })
            .collect::<Vec<_>>()
            .join("&"),
        _ => String::new(),
    }
}

#[derive(Serialize)]
struct ParseOk {
    filter: Filter,
}

#[derive(Serialize)]
struct PlanOk {
    filter: Filter,
    plan: SubscriptionPlan,
}

#[derive(Serialize)]
struct ParseErr {
    error: String,
}

#[derive(Serialize)]
struct Stats {
    subscription_active: bool,
    receiver_count: usize,
    /// Samples currently retained in the on-demand payload ring.
    retained_samples: usize,
    /// Milliseconds since the last sample was ingested, or `null` when
    /// none ever has been.
    ///
    /// `retained_samples` alone cannot answer "is the tail moving": a
    /// ring that filled and then froze reads exactly like one being
    /// actively cycled. That ambiguity lets a blind flashlight look
    /// healthy. This field is the one an operator (or a
    /// probe) can actually watch climb.
    last_sample_ms_ago: Option<u64>,
}

/// Compile-time assertion that `FilterError` carries Display + Debug.
#[allow(dead_code)]
fn _assert_filter_error_traits() {
    fn assert_display<T: std::fmt::Display>() {}
    fn assert_debug<T: std::fmt::Debug>() {}
    assert_display::<FilterError>();
    assert_debug::<FilterError>();
}
