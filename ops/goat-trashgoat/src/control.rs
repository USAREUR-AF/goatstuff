// SPDX-License-Identifier: Apache-2.0
//
// Control-topic dispatcher.
//
// The portal publishes JSON messages to ops/trashgoat/control/<id>/v1
// on every spec state change (per docs/design/trashgoat.md §"Control
// wire contract"). Each message carries the full spec, the command, and
// an audit envelope.
//
// At runner cold-start we replay the topic via session.get() to pick up
// the current spec table, then subscribe for live updates. Both paths
// feed `dispatch_control_event`, which is the single mutation point for
// the runner state.

use crate::runner::{RunnerState, SpecState};
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Wire shape of the portal's control-topic message. Mirrors
/// trashgoatControlMessage in the portal's
/// internal/api/trashgoat.go. Some fields are kept for log + audit
/// visibility even if no runtime branch consumes them.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct ControlMessage {
    pub spec_id: i64,
    pub spec: ControlSpec,
    pub command: String,
    #[serde(default)]
    pub issued_by: String,
    #[serde(default)]
    pub issued_at: i64,
}

/// Wire shape of the spec block carried inside a control message.
/// Mirrors registry.TrashgoatSpec's JSON-tagged fields. payload_spec
/// and cadence_spec arrive as JSON-encoded strings on the wire (the
/// Go struct serializes a `string` field), but the deserializer is
/// permissive — runner parsers handle both string-wrapped and nested
/// shapes (see cadence::unwrap_string_payload).
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct ControlSpec {
    pub id: i64,
    pub slug: String,
    #[serde(default)]
    pub description: String,
    pub topic_pattern: String,
    pub payload_spec: serde_json::Value,
    pub cadence_spec: serde_json::Value,
    /// One of: "draft" / "active" / "paused" / "retired".
    pub state: String,
    #[serde(default)]
    pub notes: String,
}

impl ControlSpec {
    pub fn parsed_state(&self) -> Option<SpecState> {
        SpecState::from_str(self.state.as_str())
    }
}

/// Cold-start replay. The portal keeps the most-recent control
/// message at each ops/trashgoat/control/<id>/v1 key
/// (Zenoh REST PUT is last-write-wins per key). A `session.get()`
/// against the same key-expression returns whatever is currently
/// pinned, so we can rebuild the spec table without needing
/// portal-side persistence-of-history.
///
/// Best-effort: failures here just mean the runner starts empty and
/// picks up the next live state change. We never block startup on
/// the replay completing.
pub async fn cold_start_snapshot(
    query_session: &zenoh::Session,
    control_key: &str,
    runner_state: &Arc<Mutex<RunnerState>>,
    publish_session: &zenoh::Session,
) -> Result<()> {
    let replies = query_session
        .get(control_key)
        .await
        .map_err(|e| anyhow!("session.get({control_key}) failed: {e}"))?;
    let mut got = 0_usize;
    while let Ok(reply) = replies.recv_async().await {
        match reply.result() {
            Ok(sample) => {
                let key_expr = sample.key_expr().to_string();
                let bytes = sample.payload().to_bytes();
                if let Err(e) = dispatch_control_event(
                    publish_session,
                    runner_state,
                    &key_expr,
                    bytes.as_ref(),
                )
                .await
                {
                    warn!(error = %e, key_expr = %key_expr, "cold-start dispatch failed");
                }
                got += 1;
            }
            Err(e) => {
                warn!(error = ?e, "cold-start reply carried an error; skipping");
            }
        }
    }
    info!(replayed = got, "cold-start snapshot complete");
    Ok(())
}

/// Single mutation point for the runner state. Called both from
/// cold-start replay and the live subscriber loop.
pub async fn dispatch_control_event(
    session: &zenoh::Session,
    runner_state: &Arc<Mutex<RunnerState>>,
    key_expr: &str,
    payload: &[u8],
) -> Result<()> {
    let msg: ControlMessage =
        serde_json::from_slice(payload).with_context(|| {
            format!("parse control message on {key_expr} ({} bytes)", payload.len())
        })?;
    info!(
        spec_id = msg.spec_id,
        slug = msg.spec.slug.as_str(),
        command = msg.command.as_str(),
        state = msg.spec.state.as_str(),
        issued_by = msg.issued_by.as_str(),
        "control event"
    );

    let mut state = runner_state.lock().await;
    state.apply_control_event(session, msg).await
}
