// SPDX-License-Identifier: Apache-2.0
//
// POST /api/compute — server-side counterpart to the Tier 2 browser
// WASM module. Operator selects via `?compute=server` URL parameter
// (per docs/design/flashlight.md §URL grammar) when their device
// can't run WASM efficiently.
//
// Shape: stateless. Request body is `{ events: [...] }` (the same
// envelope the equivalence-test fixtures use, minus the
// `expected_snapshot_json` field). Server runs them through a fresh
// `flashlight_analytics::Engine` and returns the
// `AnalyticsSnapshot` as JSON. The native code path here is the
// same code that compiles to WASM via the cdylib target — the
// `wasm-bindgen-test` runner in CI (#586) asserts byte-equal output
// between the two compile targets, so an operator switching
// `?compute=browser` ↔ `?compute=server` sees identical analytics
// for the same input stream.
//
// Optional `?window_ms=` query parameter overrides the default
// 5-minute rolling window — same operator-facing knob the design
// doc calls out alongside the compute placement parameter.

use axum::{
    extract::Query,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use flashlight_analytics::{AnalyticsSnapshot, Engine, Event};
use serde::Deserialize;

/// Request body for POST /api/compute. The `events` array is the
/// ordered sequence of Tier 1 SSE events the operator's session has
/// accumulated — the server runs them through a fresh Engine,
/// reproducing the same analytics state the browser's WASM module
/// would have computed locally.
#[derive(Debug, Deserialize)]
pub struct ComputeRequest {
    pub events: Vec<Event>,
}

/// Optional query parameters. `window_ms` overrides the rolling-stats
/// + sparkline window length. Future capability flags (decode FDS
/// inline, schema-diff prefix policy) land alongside this.
#[derive(Debug, Default, Deserialize)]
pub struct ComputeQuery {
    pub window_ms: Option<i64>,
}

/// POST /api/compute handler. Returns 200 + AnalyticsSnapshot on
/// success. Payload-shape errors fall through to axum's default
/// `Json` extractor 400 response (which serializes the serde error
/// directly) — no need for a bespoke error envelope here.
pub async fn compute_endpoint(
    Query(q): Query<ComputeQuery>,
    Json(req): Json<ComputeRequest>,
) -> Response {
    let mut engine = match q.window_ms {
        Some(w) if w > 0 => Engine::with_window_ms(w),
        _ => Engine::new(),
    };
    for event in &req.events {
        engine.push_event(event);
    }
    let snapshot: AnalyticsSnapshot = engine.snapshot();
    (StatusCode::OK, Json(snapshot)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(ts_ms: i64, key: &str, payload: Vec<u8>, format: Option<&str>) -> Event {
        Event {
            key: key.to_string(),
            ts_ms,
            payload,
            format: format.map(|s| s.to_string()),
        }
    }

    #[tokio::test]
    async fn empty_request_yields_zero_state_snapshot() {
        let req = ComputeRequest { events: vec![] };
        let response = compute_endpoint(
            Query(ComputeQuery::default()),
            Json(req),
        )
        .await;
        // 200 + snapshot body — defaulting to fresh-Engine zero state.
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn endpoint_runs_events_through_engine() {
        // Drive two events; assert the resulting snapshot is what an
        // Engine fed those events would produce. This is the same
        // assertion the equivalence-test fixtures make — the endpoint
        // is a thin axum-wrapper around `Engine::push_event` /
        // `Engine::snapshot`, so its correctness reduces to the
        // crate's existing test coverage.
        let req = ComputeRequest {
            events: vec![
                ev(1_000, "k", b"hello".to_vec(), Some("protobuf:A")),
                ev(2_000, "k", b"world".to_vec(), None),
            ],
        };
        // Run the same fixture through Engine in-process.
        let mut expected_engine = Engine::new();
        for e in &req.events {
            expected_engine.push_event(e);
        }
        let expected = expected_engine.snapshot();

        // Drive through the endpoint and parse the body.
        let response = compute_endpoint(
            Query(ComputeQuery::default()),
            Json(req),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let got: AnalyticsSnapshot = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(got, expected);
    }

    #[tokio::test]
    async fn window_ms_query_parameter_overrides_default() {
        // window_ms=60_000 (1 minute) instead of default 300_000 (5 min).
        // Drive one event, snapshot the window_ms field on stats.
        let req = ComputeRequest {
            events: vec![ev(1_000, "k", vec![0u8; 10], None)],
        };
        let response = compute_endpoint(
            Query(ComputeQuery { window_ms: Some(60_000) }),
            Json(req),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let got: AnalyticsSnapshot = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(got.stats.window_ms, 60_000);
        assert_eq!(got.sparkline.window_ms, 60_000);
    }
}
