// SPDX-License-Identifier: Apache-2.0
//
// WebSocket fanout for live fabric updates.
//
// Protocol per docs/design/panoscope.md §"Sampling + back-pressure":
//
//   1. On connect, the server sends one `{"type":"snapshot", ...}`
//      message with the current full Topology + FlowState (same shape
//      as GET /api/snapshot) — unless the URL carries `snapshot=0`, which
//      pages drawing from /api/view send.
//
//   2. The server then streams `{"type":"delta", "events":[...]}`
//      messages every `ws_coalesce_ms` (default 100ms). Each delta
//      carries the WsEvents accumulated since the last tick. Ticks
//      with no events are skipped — browsers only wake on real
//      activity.
//
//   3. If the broadcast channel lags (slow consumer), the server
//      sends one `{"type":"lagged", "missed":N}` warning then
//      continues; the client is expected to fetch /api/snapshot to
//      resync.

use std::collections::HashSet;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, warn};

use crate::viewer::ViewerScope;
use crate::zenoh_client::{SharedState, WsEvent};

/// Inbound client message — operator-driven LIVE-follow control. The
/// only inbound shape v1 accepts; anything else is ignored. Per
/// docs/design/panoscope.md §"LIVE-follow on the stream view".
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum InboundMessage {
    /// Add `topic` to this connection's watch set. Subsequent
    /// `SampleArrived` events for that topic will be forwarded.
    Watch { topic: String },
    /// Remove `topic` from this connection's watch set.
    Unwatch { topic: String },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutgoingMessage<'a> {
    Snapshot {
        nodes: serde_json::Value,
        edges: serde_json::Value,
        sampling_drops: serde_json::Value,
        generated_at_ms: u64,
    },
    Delta {
        events: &'a [WsEvent],
    },
    Lagged {
        missed: u64,
    },
}

/// Handle one WebSocket connection. Sends the initial snapshot (unless the
/// page asked not to — see `WsQuery` in main.rs) then enters the coalescing
/// loop. Returns when the client disconnects or the broadcast receiver
/// errors fatally.
pub async fn handle_socket(
    socket: WebSocket,
    state: SharedState,
    coalesce_ms: u64,
    scope: ViewerScope,
    send_snapshot: bool,
) {
    let mut socket = socket;
    let mut rx = state.events.subscribe();

    // Per-connection watch set for LIVE-follow opt-in. Empty by default;
    // populated by inbound {"op":"watch","topic":"<x>"} messages.
    // `SampleArrived` events are filtered against this; all other
    // event kinds are unfiltered (they're global fabric truths).
    // Lifecycle is tied to this scope — drops on connection exit, no
    // external state to clean up.
    let mut watching: HashSet<String> = HashSet::new();

    // Step 1: send the initial snapshot (filtered to the viewer's scope).
    if send_snapshot {
        if let Err(e) = send_initial_snapshot(&mut socket, &state, &scope).await {
            warn!(error = %e, "ws: initial snapshot send failed");
            return;
        }
    }

    // Step 2: coalesced delta loop.
    let mut tick = tokio::time::interval(Duration::from_millis(coalesce_ms));
    let mut buffer: Vec<WsEvent> = Vec::with_capacity(64);

    loop {
        tokio::select! {
            // Drain pending events into the buffer, filtering
            // SampleArrived against the per-connection watch set.
            recv = rx.recv() => {
                match recv {
                    Ok(ev) => {
                        if let WsEvent::SampleArrived { ref topic, .. } = ev {
                            if !watching.contains(topic) {
                                continue;
                            }
                        }
                        // Per-viewer RBAC scope gate (ADR 1073) — no-op at
                        // Full scope; lower tiers only receive events for
                        // keys/peers their scope covers. PeerSeen carries no
                        // node_class, but infrastructure nodes are visible to
                        // every tier (the snapshot rule) — rescue those via a
                        // topology lookup so the live stream and the snapshot
                        // agree about infra joins. Rare event; the read lock
                        // is off the per-sample hot path.
                        if !scope.allows_event(&ev) {
                            // Infra is visible to every scope; notional
                            // producers to notional-granted scopes — but
                            // events carry neither node_class nor the
                            // synthetic flag, so rescue via topology to
                            // keep the live stream consistent with the
                            // snapshot: PeerSeen resolves the peer itself,
                            // topic events resolve the topic's publisher
                            // (the same second chance /api/samples runs).
                            // The lookup fires only for already-denied
                            // events, and allows_synthetic_key is false for
                            // every scope except notional-granted ones.
                            let rescue = match &ev {
                                WsEvent::PeerSeen { peer_id, .. } => {
                                    use panoscope_core::NodeClass;
                                    state.topology.read().await.peers.get(peer_id).is_some_and(
                                        |p| {
                                            p.node_class() == NodeClass::Infra
                                                || scope.allows_synthetic_key(
                                                    p.synthetic == Some(true),
                                                )
                                        },
                                    )
                                }
                                WsEvent::PublishSampled { topic, .. }
                                | WsEvent::PublishDropped { topic, .. }
                                | WsEvent::SampleArrived { topic, .. } => scope
                                    .allows_synthetic_key(
                                        crate::topic_is_synthetic(&state, topic).await,
                                    ),
                            };
                            if !rescue {
                                continue;
                            }
                        }
                        buffer.push(ev);
                    }
                    Err(RecvError::Lagged(missed)) => {
                        let msg = OutgoingMessage::Lagged { missed };
                        if !send_json(&mut socket, &msg).await {
                            break;
                        }
                    }
                    Err(RecvError::Closed) => {
                        debug!("ws: events channel closed; exiting");
                        break;
                    }
                }
            }

            // On each tick, flush whatever's in the buffer.
            _ = tick.tick() => {
                if !buffer.is_empty() {
                    let msg = OutgoingMessage::Delta { events: &buffer };
                    if !send_json(&mut socket, &msg).await {
                        break;
                    }
                    buffer.clear();
                }
            }

            // Client sent us something (ping / close / watch / unwatch).
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(Message::Close(_))) => {
                        debug!("ws: client closed");
                        break;
                    }
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {
                        // axum handles pongs for us; nothing to do.
                    }
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<InboundMessage>(&text) {
                            Ok(InboundMessage::Watch { topic }) => {
                                // Scope-gated: watching an out-of-scope
                                // topic never streams (belt to the
                                // allows_event suspenders above). Notional
                                // topics get the same synthetic-publisher
                                // second chance as /api/samples.
                                if scope.allows_key(&topic)
                                    || scope.allows_synthetic_key(
                                        crate::topic_is_synthetic(&state, &topic).await,
                                    )
                                {
                                    watching.insert(topic);
                                } else {
                                    debug!(topic = %topic, "ws: watch refused by viewer scope");
                                }
                            }
                            Ok(InboundMessage::Unwatch { topic }) => {
                                watching.remove(&topic);
                            }
                            Err(e) => {
                                debug!(error = %e, "ws: ignoring unrecognized inbound message");
                            }
                        }
                    }
                    Some(Ok(_)) => {
                        // Ignore binary frames — v1 inbound is JSON text only.
                    }
                    Some(Err(e)) => {
                        warn!(error = %e, "ws: recv error");
                        break;
                    }
                    None => break,
                }
            }
        }
    }
}

async fn send_initial_snapshot(
    socket: &mut WebSocket,
    state: &SharedState,
    scope: &ViewerScope,
) -> anyhow::Result<()> {
    // The same shared build /api/snapshot serves. No ingest locks are held
    // across serialization or the send below.
    let build = crate::snapshot_view::shared_snapshot_build(state).await;
    let (nodes, edges) = scope.filter_snapshot((*build.nodes).clone(), (*build.edges).clone());
    let sampling_drops = serde_json::to_value(scope.filter_sampling_drops(&build.sampling_drops))?;

    let msg = OutgoingMessage::Snapshot {
        nodes: serde_json::to_value(&nodes)?,
        edges: serde_json::to_value(&edges)?,
        sampling_drops,
        generated_at_ms: build.built_at_ms,
    };
    let body = serde_json::to_string(&msg)?;
    socket.send(Message::Text(body.into())).await?;
    Ok(())
}

/// Returns false if the send fails (client disconnected).
async fn send_json<T: Serialize>(socket: &mut WebSocket, msg: &T) -> bool {
    match serde_json::to_string(msg) {
        Ok(body) => socket.send(Message::Text(body.into())).await.is_ok(),
        Err(e) => {
            warn!(error = %e, "ws: serialize error; dropping message");
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn outgoing_snapshot_serializes_with_type_tag() {
        let msg = OutgoingMessage::Snapshot {
            nodes: serde_json::json!([]),
            edges: serde_json::json!([]),
            sampling_drops: serde_json::json!({}),
            generated_at_ms: 123,
        };
        let body: Value = serde_json::from_str(&serde_json::to_string(&msg).unwrap()).unwrap();
        assert_eq!(body["type"], "snapshot");
        assert_eq!(body["generated_at_ms"], 123);
    }

    #[test]
    fn outgoing_delta_carries_events() {
        let events = vec![WsEvent::PeerSeen {
            peer_id: "p1".into(),
            ts_ms: 100,
        }];
        let msg = OutgoingMessage::Delta { events: &events };
        let body: Value = serde_json::from_str(&serde_json::to_string(&msg).unwrap()).unwrap();
        assert_eq!(body["type"], "delta");
        assert_eq!(body["events"][0]["kind"], "peer_seen");
        assert_eq!(body["events"][0]["peer_id"], "p1");
    }

    #[test]
    fn outgoing_lagged_reports_missed_count() {
        let msg = OutgoingMessage::Lagged { missed: 7 };
        let body: Value = serde_json::from_str(&serde_json::to_string(&msg).unwrap()).unwrap();
        assert_eq!(body["type"], "lagged");
        assert_eq!(body["missed"], 7);
    }

    #[test]
    fn inbound_watch_parses() {
        let raw = r#"{"op":"watch","topic":"notional/falcon/orders/v1"}"#;
        let msg: InboundMessage = serde_json::from_str(raw).unwrap();
        match msg {
            InboundMessage::Watch { topic } => assert_eq!(topic, "notional/falcon/orders/v1"),
            _ => panic!("expected watch"),
        }
    }

    #[test]
    fn inbound_unwatch_parses() {
        let raw = r#"{"op":"unwatch","topic":"x"}"#;
        let msg: InboundMessage = serde_json::from_str(raw).unwrap();
        match msg {
            InboundMessage::Unwatch { topic } => assert_eq!(topic, "x"),
            _ => panic!("expected unwatch"),
        }
    }

    #[test]
    fn inbound_unknown_op_errors_cleanly() {
        let raw = r#"{"op":"bogus","topic":"x"}"#;
        let r: Result<InboundMessage, _> = serde_json::from_str(raw);
        assert!(r.is_err());
    }

    #[test]
    fn sample_arrived_serializes_with_kind_tag() {
        let ev = WsEvent::SampleArrived {
            topic: "t/v1".into(),
            ts_ms: 42,
        };
        let body: Value = serde_json::from_str(&serde_json::to_string(&ev).unwrap()).unwrap();
        assert_eq!(body["kind"], "sample_arrived");
        assert_eq!(body["topic"], "t/v1");
        assert_eq!(body["ts_ms"], 42);
    }
}
