// SPDX-License-Identifier: Apache-2.0
//! HTTP wiring. Routes, cache validators, and the subscription that feeds the
//! store — nothing else.
//!
//! State lives in [`crate::store`], the payload shape in [`crate::view`], the
//! bytes in [`crate::assets`]. This file should stay boring.
//!
//! # The property to protect in review
//!
//! **A request does no work.** Not merely no *fabric* work — no work at all.
//! Every handler either returns a compiled-in asset or clones one `Arc` holding
//! bytes serialized on the write path. If a change here makes a handler
//! compute, serialize, or `get()` anything, it has undone the reason this
//! component is shaped the way it is (ADR 1123, ADR 1160 §5).

use std::sync::Arc;

use anyhow::Result;
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use prost::Message;
use tokio::sync::RwLock;

use crate::assets;
use crate::store::Store;
use crate::view;
use crate::wire::RouterTopologyV1;

type Shared = Arc<RwLock<Store>>;

/// How often to re-grade without a publish. A router going quiet IS the event,
/// and staleness is the absence of a write — so nothing would ever surface it.
const REFRESH_SECS: u64 = 10;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn if_none_match(h: &HeaderMap) -> Option<&str> {
    h.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok())
}

/// Serve a compiled-in asset, honouring the client's cache validator.
fn asset(path: &'static str, headers: &HeaderMap) -> axum::response::Response {
    let Some(a) = assets::get(path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    asset_from(a, headers)
}

fn asset_from(a: &'static assets::Asset, headers: &HeaderMap) -> axum::response::Response {
    if if_none_match(headers) == Some(a.etag.as_str()) {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, a.etag.clone())]).into_response();
    }
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, a.content_type.to_string()),
            (header::ETAG, a.etag.clone()),
            // Revalidate every time. These are fixed per image so the check is
            // cheap, and a stale bundle an operator cannot refresh away during
            // an incident is a worse trade than the round trip.
            (header::CACHE_CONTROL, "no-cache".to_string()),
        ],
        a.body,
    )
        .into_response()
}

async fn index(headers: HeaderMap) -> impl IntoResponse {
    asset("/", &headers)
}

/// One route for every static file, so adding a module is a table entry rather
/// than a route plus a handler plus a place to forget one of them.
async fn static_asset(
    axum::extract::Path(rest): axum::extract::Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Lookup is against a fixed table of compiled-in paths, so a traversal
    // attempt simply misses; there is no filesystem behind this.
    match assets::get(&format!("/assets/{rest}")) {
        Some(a) => asset_from(a, &headers),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// The payload. Clones one `Arc` and returns it; the bytes were built on the
/// write path. An unchanged fabric answers 304 with no body, which is the
/// common case — a settled routing plane changes nothing between polls.
async fn api(State(store): State<Shared>, headers: HeaderMap) -> impl IntoResponse {
    let cached = { store.read().await.cached() };
    let Some(c) = cached else {
        // Before the first sample. Honest, rather than an empty object that
        // would render as "no routers" — which means something different.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "application/json")],
            br#"{"error":"no router has reported yet"}"#.to_vec(),
        )
            .into_response();
    };
    if if_none_match(&headers) == Some(c.etag.as_str()) {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, c.etag)]).into_response();
    }
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json".to_string()),
            (header::ETAG, c.etag),
            (header::CACHE_CONTROL, "no-cache".to_string()),
        ],
        c.body.to_vec(),
    )
        .into_response()
}

/// Liveness only. Deliberately NOT a fabric probe: a health endpoint that
/// reached the fabric would be a scheduled version of the thing ADR 1123
/// forbids, fired by every uptime checker on the mesh.
async fn health(State(store): State<Shared>) -> impl IntoResponse {
    let n = store.read().await.len();
    Json(serde_json::json!({ "status": "ok", "routers_known": n }))
}

pub async fn run(session: zenoh::Session, site: Option<String>, bind: &str) -> Result<()> {
    let store: Shared = Arc::new(RwLock::new(Store::new(now_ms())));

    let selector = match &site {
        Some(s) => format!("ops/fabric/topology/{s}/**"),
        None => "ops/fabric/topology/**".to_string(),
    };
    tracing::info!(selector = %selector, bind = %bind, "subscribing");

    let subscriber = session
        .declare_subscriber(&selector)
        .await
        .map_err(|e| anyhow::anyhow!("declare subscriber {selector}: {e}"))?;

    // clippy.toml forbids the global spawn because goat-relay pins hot-path
    // tasks to one shard for life (ADR 0709 D7 layer 3). This is neither
    // goat-relay nor a hot path: long-lived supervisor tasks on a service with
    // no shard affinity. The policy's own escape hatch is an explicit allow
    // with the reasoning stated, so a reviewer sees the deviation.
    let ingest = store.clone();
    #[allow(clippy::disallowed_methods)]
    tokio::spawn(async move {
        while let Ok(sample) = subscriber.recv_async().await {
            let key = sample.key_expr().as_str().to_string();
            let id = key
                .strip_prefix("ops/fabric/topology/")
                .unwrap_or(&key)
                .to_string();
            match RouterTopologyV1::decode(&*sample.payload().to_bytes()) {
                Ok(snap) => {
                    let now = now_ms();
                    let mut st = ingest.write().await;
                    if st.ingest(id, snap, now) {
                        let body = view::build(&st, now);
                        st.set_cached(body);
                    }
                }
                Err(e) => tracing::warn!(key = %key, "undecodable RouterTopologyV1: {e}"),
            }
        }
        tracing::warn!("topology subscription ended");
    });

    let ticker = store.clone();
    #[allow(clippy::disallowed_methods)]
    tokio::spawn(async move {
        let mut t = tokio::time::interval(std::time::Duration::from_secs(REFRESH_SECS));
        loop {
            t.tick().await;
            let now = now_ms();
            let mut st = ticker.write().await;
            if st.refresh(now) {
                let body = view::build(&st, now);
                st.set_cached(body);
            }
        }
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/assets/*rest", get(static_asset))
        .route("/api/topology", get(api))
        .route("/health", get(health))
        .with_state(store);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::store::Store;
    use crate::view::testing::{ingest, refresh};
    use crate::wire::{GatewayMode, RegionV1, RouterTopologyV1, SessionV1};

    fn router(node: &str, zid: &str, peers: &[&str]) -> RouterTopologyV1 {
        RouterTopologyV1 {
            zid: zid.into(),
            node: node.into(),
            site: "zagros-sandbox".into(),
            interval_secs: 30,
            gateway_mode: GatewayMode::Auto as i32,
            regions: vec![RegionV1 {
                name: "north".into(),
                link_state_v: peers.len() as u32 + 1,
                link_state_e: peers.len() as u32,
                member_zids: vec![zid.into()],
                session_count: peers.len() as u32,
                ..Default::default()
            }],
            sessions: peers
                .iter()
                .map(|p| SessionV1 {
                    peer_zid: (*p).into(),
                    whatami: "router".into(),
                    region: "north".into(),
                    link_dst: vec![],
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn an_edge_is_rendered_once_not_once_per_direction() {
        let mut st = Store::new(0);
        ingest(&mut st, "s/a".into(), router("a", "aaaa", &["bbbb"]), 1_000);
        ingest(&mut st, "s/b".into(), router("b", "bbbb", &["aaaa"]), 1_000);
        let v: serde_json::Value = serde_json::from_slice(&st.cached().unwrap().body).unwrap();
        assert_eq!(
            v["edges"].as_array().unwrap().len(),
            1,
            "A->B and B->A are one line"
        );
    }

    #[test]
    fn a_stale_router_stays_in_the_payload_and_is_marked() {
        let mut st = Store::new(0);
        ingest(&mut st, "s/a".into(), router("a", "aaaa", &[]), 0);
        refresh(&mut st, 600_000);
        let v: serde_json::Value = serde_json::from_slice(&st.cached().unwrap().body).unwrap();
        assert_eq!(v["routers"].as_array().unwrap().len(), 1, "never dropped");
        assert_eq!(v["routers"][0]["stale"], true);
        assert_eq!(v["counts"]["stale"], 1);
    }

    #[test]
    fn staleness_surfaces_without_any_publish() {
        // Staleness is the ABSENCE of a write, so a store that only rebuilt on
        // ingest would never report it. The refresh tick is what closes that.
        let mut st = Store::new(0);
        ingest(&mut st, "s/a".into(), router("a", "aaaa", &[]), 0);
        let before: serde_json::Value = serde_json::from_slice(&st.cached().unwrap().body).unwrap();
        assert_eq!(before["counts"]["stale"], 0);

        refresh(&mut st, 900_000);
        let after: serde_json::Value = serde_json::from_slice(&st.cached().unwrap().body).unwrap();
        assert_eq!(after["counts"]["stale"], 1);
        assert!(
            after["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["id"] == "T-006"),
            "a quiet router must raise T-006 without anyone publishing"
        );
    }
}
