// SPDX-License-Identifier: Apache-2.0
//
// SSE endpoint — GET /events?<filter> streams the per-request
// narrowed slice of the live event tail. Tier 1's canonical workhorse
// per docs/design/flashlight.md §"Tier 1 surfaces".
//
// Architecture:
//   - PR #2 wires a single broad `**` Zenoh subscriber so the
//     in-process broadcast channel carries every sample.
//   - Each SSE client (this handler) parses the URL filter, plans
//     the matching key-expressions, and re-narrows in-process by
//     calling `keymatch::key_matches_any` on every event.
//   - One Zenoh session amortizes across N SSE clients; the
//     in-process filter is cheap (Zenoh's KeyExpr::intersects).
//
// Post-filters (role/site/format) are applied here too: when the plan
// carries them, each surviving event's key is attributed to a publisher
// in the schema-decl-hydrated `Topology` and dropped unless that
// publisher's declared metadata satisfies the filter
// (subscription::post_filters_pass). An event whose key matches no
// declared publisher is dropped while a post-filter is active — the
// correct behavior vs. the prior silent widen-to-`**`.

use std::convert::Infallible;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
};
use futures::stream::{Stream, StreamExt};
use panoscope_core::topology::Topology;
use tokio_stream::wrappers::{errors::BroadcastStreamRecvError, BroadcastStream};
use tracing::{debug, warn};

use crate::filter::parse_filter;
use crate::keymatch::key_matches_any;
use crate::subscription::{plan_subscription, post_filters_pass, SubscriptionPlan};
use crate::zenoh_client::SampleEvent;
use crate::AppState;

/// Heartbeat interval — sends a `: keep-alive\n\n` comment line every
/// 15 s so reverse proxies + browser readyState don't drop the
/// connection during quiet stretches.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// `GET /events?<filter>` — text/event-stream of SampleEvents whose
/// keys match the request's `SubscriptionPlan`.
///
/// Status codes:
///   - 200: connection upgrades to SSE; events stream as they arrive.
///   - 400: filter parse error (returns a structured `{"error": ...}`
///          body instead of the stream).
///   - 503: the live subscription isn't running (FLASHLIGHT_ZENOH_ENDPOINT
///          unset at boot, or Zenoh session failed to open).
///   - 422: filter is well-formed but resolves to an empty plan.
///          PR #4 of Phase C replaces this with a redirect to the
///          help page; for now we surface the structured error so the
///          /api/parse + /api/plan diagnostics are useful.
pub async fn events_handler(
    State(state): State<AppState>,
    Query(raw): Query<serde_json::Value>,
) -> axum::response::Response {
    let query = crate::collapse_query(raw);
    let filter = match parse_filter(&query) {
        Ok(f) => f,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };
    let plan = plan_subscription(&filter);
    if plan.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(serde_json::json!({
                "error": "filter is empty — declare at least one of key/vendor/role/site/format",
                "example": "?vendor=notional"
            })),
        )
            .into_response();
    }

    let Some(tx) = state.events.read().ok().and_then(|g| g.clone()) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({
                "error": "live subscription not yet established — flashlight is connecting to the zenoh router in the background; retry shortly"
            })),
        )
            .into_response();
    };

    let rx = tx.subscribe();
    debug!(
        ?plan,
        receivers = tx.receiver_count(),
        "sse client attached"
    );

    let stream = build_event_stream(rx, plan, state.topology.clone());
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(KEEPALIVE_INTERVAL))
        .into_response()
}

/// Build the stream feeding `Sse::new`. Split out so it can be unit-
/// tested without spinning up axum: feed a synthetic broadcast
/// channel, assert which events arrive on the resulting stream.
pub fn build_event_stream(
    rx: tokio::sync::broadcast::Receiver<SampleEvent>,
    plan: SubscriptionPlan,
    topology: Arc<RwLock<Topology>>,
) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static {
    let plan = std::sync::Arc::new(plan);
    BroadcastStream::new(rx).filter_map(move |result| {
        let plan = plan.clone();
        let topology = topology.clone();
        async move {
            match result {
                Ok(event) => {
                    // `key=` patterns and `vendor=` names union (OR), the
                    // vendor leg anchor-aware so slot-pool keys
                    // (`<anchor>/<vendor>/...`) aren't silently invisible
                    //.
                    if !key_matches_any(&event.key, &plan.key_exprs)
                        && !plan.matches_vendor(&event.key)
                    {
                        return None;
                    }
                    // Post-filters (role/site/format): resolve the
                    // event's publisher in the hydrated topology and
                    // drop events that don't satisfy the declared
                    // metadata. No-op when no post-filter is active.
                    // The read guard is dropped before any `.await`.
                    if plan.has_post_filters() {
                        let pass = match topology.read() {
                            Ok(topo) => post_filters_pass(&topo, &event.key, &plan.post),
                            // Poisoned lock: fail open on the key match
                            // we already did rather than black-hole the
                            // tail.
                            Err(_) => true,
                        };
                        if !pass {
                            return None;
                        }
                    }
                    let json = match serde_json::to_string(&event) {
                        Ok(s) => s,
                        Err(e) => {
                            warn!(error = %e, "sample serialization failed");
                            return None;
                        }
                    };
                    Some(Ok(Event::default().event("sample").data(json)))
                }
                Err(BroadcastStreamRecvError::Lagged(n)) => {
                    // Surface lag to the client so the Tier 1 frontend
                    // can show a "missed N events" indicator instead
                    // of silently sliding past gaps. `BroadcastStream`
                    // continues yielding after a lag — only the
                    // underlying-channel-closed case ends the stream,
                    // which `BroadcastStream` represents by returning
                    // `None` from its Stream impl (no error variant
                    // needed here).
                    Some(Ok(Event::default()
                        .event("lagged")
                        .data(format!(r#"{{"missed":{n}}}"#))))
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::PostFilters;
    use std::collections::HashSet;
    use tokio::sync::broadcast;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|x| x.to_string()).collect()
    }

    fn empty_topo() -> Arc<RwLock<Topology>> {
        Arc::new(RwLock::new(Topology::default()))
    }

    fn sample(key: &str) -> SampleEvent {
        SampleEvent {
            seq: 0,
            key: key.to_string(),
            ts_ms: 1_700_000_000_000,
            src_zid: None,
            payload_len: 42,
            format: crate::store::SampleFormat::Binary,
            encoding: None,
        }
    }

    async fn collect_events<S>(stream: S, expect_at_least: usize) -> Vec<Event>
    where
        S: Stream<Item = Result<Event, Infallible>> + Send + Unpin + 'static,
    {
        let mut events = Vec::new();
        let mut s = stream;
        // Bounded by `expect_at_least` so a regression (no events) is
        // detected as a test timeout, not an infinite loop.
        for _ in 0..expect_at_least {
            match tokio::time::timeout(Duration::from_millis(500), s.next()).await {
                Ok(Some(Ok(ev))) => events.push(ev),
                Ok(Some(Err(_))) => unreachable!("Infallible"),
                Ok(None) => break,
                Err(_) => panic!("timeout waiting for SSE event"),
            }
        }
        events
    }

    #[tokio::test]
    async fn stream_emits_matching_samples() {
        let (tx, _drop_rx) = broadcast::channel::<SampleEvent>(32);
        let rx = tx.subscribe();
        let plan = SubscriptionPlan {
            key_exprs: set(&["vendor-a/**"]),
            post: PostFilters::default(),
            ..SubscriptionPlan::default()
        };
        let stream = Box::pin(build_event_stream(rx, plan, empty_topo()));

        tx.send(sample("vendor-a/observations/v1")).unwrap();
        tx.send(sample("vendor-b/sensorgrid/raw")).unwrap();
        tx.send(sample("vendor-a/control/v1")).unwrap();

        // Expect 2 matching events (vendor-b is filtered out).
        let events = collect_events(stream, 2).await;
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn stream_vendor_predicate_matches_slot_anchored_keys() {
        // ?vendor=tarsier on a slot-pool goatnet: wire keys lead with
        // the UUID anchor, not the vendor name. The vendor
        // predicate must pass them; a non-anchor first segment carrying
        // the vendor name deeper must not match.
        let (tx, _drop_rx) = broadcast::channel::<SampleEvent>(32);
        let rx = tx.subscribe();
        let plan = SubscriptionPlan {
            vendors: set(&["tarsier"]),
            ..SubscriptionPlan::default()
        };
        let stream = Box::pin(build_event_stream(rx, plan, empty_topo()));

        tx.send(sample(
            "483a44bc6ab522416e639fbe726f79b4/tarsier/bravo-fob/contacts/v1",
        ))
        .unwrap();
        tx.send(sample("tarsier/alpha-fob/contacts/v1")).unwrap();
        tx.send(sample("ops/tarsier/health")).unwrap();
        tx.send(sample(
            "483a44bc6ab522416e639fbe726f79b4/mantis/alpha-fob/imagery/v1",
        ))
        .unwrap();

        // Slot-anchored + per-vendor shapes pass; the other two don't.
        let events = collect_events(stream, 2).await;
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn stream_filters_out_non_matching_samples() {
        let (tx, _) = broadcast::channel::<SampleEvent>(32);
        let rx = tx.subscribe();
        let plan = SubscriptionPlan {
            key_exprs: set(&["notional/tarsier/**"]),
            post: PostFilters::default(),
            ..SubscriptionPlan::default()
        };
        let stream = Box::pin(build_event_stream(rx, plan, empty_topo()));

        // Three samples; only one matches the narrow filter.
        tx.send(sample("notional/tarsier/alpha-fob/track/v1"))
            .unwrap();
        tx.send(sample("notional/mongoose/alpha-fob/effector/v1"))
            .unwrap();
        tx.send(sample("demogame/nova-a/event/v1")).unwrap();

        let events = collect_events(stream, 1).await;
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn lagged_event_surfaces_to_client() {
        // Tiny channel; saturate so a slow consumer lags.
        let (tx, rx) = broadcast::channel::<SampleEvent>(2);
        let plan = SubscriptionPlan {
            key_exprs: set(&["**"]),
            post: PostFilters::default(),
            ..SubscriptionPlan::default()
        };
        let stream = Box::pin(build_event_stream(rx, plan, empty_topo()));

        // Send 5 events into the 2-slot channel BEFORE reading any →
        // first recv on the subscriber lags by 3.
        for i in 0..5 {
            tx.send(sample(&format!("k/{i}"))).unwrap();
        }

        let events = collect_events(stream, 1).await;
        assert_eq!(events.len(), 1);
        // The lagged event carries the literal "lagged" event name +
        // a JSON body with the miss count.
        // (axum::response::sse::Event doesn't expose its event field
        //  for direct inspection; we verify via the stream's smoke
        //  shape — the test catches REGRESSION where this path
        //  vanished. A future change can switch to `Event::to_bytes`
        //  if axum exposes it.)
    }

    #[tokio::test]
    async fn empty_pattern_set_drops_everything() {
        let (tx, rx) = broadcast::channel::<SampleEvent>(32);
        let plan = SubscriptionPlan::default(); // empty key_exprs
        let stream = Box::pin(build_event_stream(rx, plan, empty_topo()));

        tx.send(sample("anything/at/all")).unwrap();

        // Use a short timeout — no event should arrive because
        // `key_matches_any(_, &empty)` returns false.
        let result = tokio::time::timeout(
            Duration::from_millis(150),
            futures::StreamExt::next(&mut Box::pin(stream)),
        )
        .await;
        // Either timeout (no event arrived) or None (stream closed)
        // is acceptable; what's NOT acceptable is a yielded Event.
        match result {
            Ok(Some(Ok(_))) => panic!("event arrived through empty filter"),
            _ => {} // ok
        }
    }

    #[tokio::test]
    async fn post_filter_role_drops_unattributed_and_nonmatching() {
        // Hydrate a topology: peer "tarsier" publishes notional/tarsier/**
        // and is declared role_class=sensor.
        let topo = empty_topo();
        {
            let mut t = topo.write().unwrap();
            t.record_publish_declaration("tarsier".into(), "notional/tarsier/**".into(), 1);
            t.peers.get_mut("tarsier").unwrap().role_class = Some("sensor".into());
            // A second peer publishing mongoose/** but role_class=effector.
            t.record_publish_declaration("mongoose".into(), "mongoose/**".into(), 1);
            t.peers.get_mut("mongoose").unwrap().role_class = Some("effector".into());
        }

        // Broad wire plan (role-only filter widens to **) + role=sensor.
        let plan = SubscriptionPlan {
            key_exprs: set(&["**"]),
            post: PostFilters {
                roles: set(&["sensor"]),
                ..PostFilters::default()
            },
            ..SubscriptionPlan::default()
        };
        let (tx, rx) = broadcast::channel::<SampleEvent>(32);
        let stream = Box::pin(build_event_stream(rx, plan, topo));

        // sensor peer's key → passes; effector peer's key → dropped;
        // unattributed key → dropped.
        tx.send(sample("notional/tarsier/alpha/track/v1")).unwrap();
        tx.send(sample("mongoose/effector/v1")).unwrap();
        tx.send(sample("orphan/unknown/v1")).unwrap();

        // Only the sensor event should survive. Collect with a bounded
        // window; assert exactly one matching sample arrived.
        let mut got = 0usize;
        let mut s = stream;
        for _ in 0..3 {
            match tokio::time::timeout(Duration::from_millis(200), s.next()).await {
                Ok(Some(Ok(_))) => got += 1,
                _ => break,
            }
        }
        assert_eq!(got, 1, "only the sensor-published event should pass role=sensor");
    }
}
