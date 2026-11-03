// SPDX-License-Identifier: Apache-2.0
//
// Runner state: holds the live spec table, drives per-spec publisher
// tasks, and serves the cleanse z_delete path.
//
// One `SpecHandle` per spec_id. When a spec transitions to active, the
// runner spawns a publisher task that loops over the cadence ticker
// and publishes the payload(s) on the resolved topic key. When the spec
// transitions to paused/retired, the task's JoinHandle is aborted.
// Cleanse iterates the per-spec `published_keys` deque and issues
// `session.delete(key)` for each.
//
// Per-spec bookkeeping (published_keys + sequence counter) is shared
// between the publisher task and the runner-state handler via an
// Arc<Mutex<SpecRuntime>>. The outer RunnerState is Arc<Mutex<>>
// already (held in main.rs) — so we have nested locks here: the
// outer state for spec lookup, the inner spec_runtime for per-spec
// counter + key history. That's intentional: holding the outer lock
// across a publish would serialize the whole runner.

use crate::cadence::{Cadence, CadenceTicker};
use crate::control::ControlMessage;
use crate::payload::Payload;
use anyhow::{anyhow, Context, Result};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// Parsed spec state. `SpecState::from_str` is the single mapping
/// from the wire string to the typed enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecState {
    Draft,
    Active,
    Paused,
    Retired,
}

impl SpecState {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "draft" => Some(SpecState::Draft),
            "active" => Some(SpecState::Active),
            "paused" => Some(SpecState::Paused),
            "retired" => Some(SpecState::Retired),
            _ => None,
        }
    }
}

/// Per-spec mutable bookkeeping shared between the publisher task and
/// the runner-state handler. Sequence counter + bounded key history.
#[derive(Debug)]
pub struct SpecRuntime {
    pub seq: u64,
    pub published_keys: VecDeque<String>,
    pub max_keys: usize,
}

impl SpecRuntime {
    fn new(max_keys: usize) -> Self {
        Self {
            seq: 0,
            published_keys: VecDeque::new(),
            max_keys,
        }
    }

    fn record_key(&mut self, key: String) {
        // Avoid recording the same key twice in a row (the common case
        // for non-templated topic_pattern — every publish lands on the
        // same key). Cleanse only needs each distinct key once.
        if self.published_keys.back().map(|s| s.as_str()) == Some(key.as_str()) {
            return;
        }
        if self.published_keys.len() >= self.max_keys {
            self.published_keys.pop_front();
        }
        self.published_keys.push_back(key);
    }
}

/// One live spec the runner is tracking.
#[allow(dead_code)]
#[derive(Debug)]
pub struct SpecHandle {
    pub spec_id: i64,
    pub slug: String,
    pub state: SpecState,
    pub runtime: Arc<Mutex<SpecRuntime>>,
    pub task: Option<JoinHandle<()>>,
}

impl SpecHandle {
    fn new(spec_id: i64, slug: String, max_keys: usize) -> Self {
        Self {
            spec_id,
            slug,
            state: SpecState::Draft,
            runtime: Arc::new(Mutex::new(SpecRuntime::new(max_keys))),
            task: None,
        }
    }

    fn cancel_task(&mut self) {
        if let Some(handle) = self.task.take() {
            handle.abort();
        }
    }
}

/// Runner-wide state. Owned by main.rs as Arc<Mutex<RunnerState>>.
pub struct RunnerState {
    pub max_hz: f64,
    pub max_keys_per_spec: usize,
    pub specs: HashMap<i64, SpecHandle>,
}

impl RunnerState {
    pub fn new(max_hz: f64, max_keys_per_spec: usize) -> Self {
        Self {
            max_hz,
            max_keys_per_spec,
            specs: HashMap::new(),
        }
    }

    /// Single mutation point for control events. Dispatches on the
    /// command field.
    pub async fn apply_control_event(
        &mut self,
        session: &zenoh::Session,
        msg: ControlMessage,
    ) -> Result<()> {
        let spec_id = msg.spec_id;
        let slug = msg.spec.slug.clone();
        let target_state = msg
            .spec
            .parsed_state()
            .ok_or_else(|| anyhow!("unknown spec state {:?}", msg.spec.state))?;
        let command = msg.command.as_str();

        match command {
            "activate" => {
                self.activate(session, spec_id, slug, target_state, &msg).await?;
            }
            "pause" => {
                self.pause(spec_id, target_state);
            }
            "retire" => {
                self.retire(spec_id);
            }
            "cleanse" => {
                self.cleanse(session, spec_id).await?;
            }
            "update" => {
                // Portal does not currently publish "update" control
                // events (only lifecycle ones do), but the runner
                // accepts the command so cold-start replay of a spec
                // mid-lifecycle still parses cleanly. No-op on the
                // running task — operator must pause+activate to pick
                // up edits.
                info!(spec_id, slug = slug.as_str(), "ignoring update command; pause+activate to apply edits");
            }
            other => {
                warn!(spec_id, command = other, "unknown command; ignoring");
            }
        }
        Ok(())
    }

    async fn activate(
        &mut self,
        session: &zenoh::Session,
        spec_id: i64,
        slug: String,
        target_state: SpecState,
        msg: &ControlMessage,
    ) -> Result<()> {
        // Cold-start replay may deliver a spec in non-active state (the
        // most-recent control event was a pause/retire). Honor the
        // target_state — only spawn a publisher when the spec is
        // actually active.
        if target_state != SpecState::Active {
            info!(
                spec_id,
                slug = slug.as_str(),
                target_state = ?target_state,
                "activate command but target state is not active; deferring publisher spawn"
            );
            let handle = self
                .specs
                .entry(spec_id)
                .or_insert_with(|| SpecHandle::new(spec_id, slug.clone(), self.max_keys_per_spec));
            handle.cancel_task();
            handle.state = target_state;
            return Ok(());
        }

        let cadence_raw = Cadence::parse(&msg.spec.cadence_spec).context("parse cadence_spec")?;
        let (cadence, was_clamped) = cadence_raw.clamped(self.max_hz);
        if was_clamped {
            warn!(
                spec_id,
                slug = slug.as_str(),
                max_hz = self.max_hz,
                "cadence clamped to max-hz ceiling"
            );
        }
        let payload = Payload::parse(&msg.spec.payload_spec).context("parse payload_spec")?;
        let topic_pattern = msg.spec.topic_pattern.clone();

        // Stop any existing publisher BEFORE swapping in the new spec
        // so we never have two tasks competing for the same spec_id.
        let handle = self
            .specs
            .entry(spec_id)
            .or_insert_with(|| SpecHandle::new(spec_id, slug.clone(), self.max_keys_per_spec));
        handle.cancel_task();
        handle.state = SpecState::Active;
        handle.slug = slug.clone();
        let runtime = handle.runtime.clone();

        // Reset the runtime's sequence + key history so a re-activated
        // spec starts fresh (no leaked state from the prior run).
        {
            let mut rt = runtime.lock().await;
            rt.seq = 0;
            rt.published_keys.clear();
        }

        let session = session.clone();
        // Admin-path spawn — one publisher task per active spec, lifetime
        // bounded by pause/retire/shutdown_all. ADR 0709 D7 layer 3's
        // spawn-local rule targets goat-relay's shard-pinned hot path;
        // trashgoat is admin-path with no per-thread shard model.
        // Deviation visible per clippy.toml.
        #[allow(clippy::disallowed_methods)]
        let task = tokio::spawn(async move {
            run_spec(session, spec_id, slug, topic_pattern, cadence, payload, runtime).await;
        });
        handle.task = Some(task);
        Ok(())
    }

    fn pause(&mut self, spec_id: i64, target_state: SpecState) {
        if let Some(h) = self.specs.get_mut(&spec_id) {
            h.cancel_task();
            h.state = target_state;
            info!(spec_id, slug = h.slug.as_str(), "paused");
        } else {
            warn!(spec_id, "pause for unknown spec; ignoring");
        }
    }

    fn retire(&mut self, spec_id: i64) {
        // Keep the spec entry around with state=Retired so a follow-on
        // `cleanse` can still walk the published-keys deque and issue
        // session.delete(key). Removing the entry on retire breaks the
        // operator-natural retire-then-cleanse ordering — observed live
        // on the chaos run when 4 retire-then-cleanse cycles
        // all logged "cleanse for unknown spec; ignoring" and never
        // wiped the test keys from storage. Entries cleared on process
        // restart; finite operator-controlled chaos spec count means no
        // unbounded growth in practice.
        if let Some(h) = self.specs.get_mut(&spec_id) {
            h.cancel_task();
            h.state = SpecState::Retired;
            info!(spec_id, slug = h.slug.as_str(), "retired");
        } else {
            warn!(spec_id, "retire for unknown spec; ignoring");
        }
    }

    async fn cleanse(&mut self, session: &zenoh::Session, spec_id: i64) -> Result<()> {
        let runtime = match self.specs.get(&spec_id) {
            Some(h) => h.runtime.clone(),
            None => {
                warn!(spec_id, "cleanse for unknown spec; ignoring");
                return Ok(());
            }
        };
        // Snapshot the keys so we don't hold the runtime lock across
        // the network calls.
        let keys: Vec<String> = {
            let rt = runtime.lock().await;
            rt.published_keys.iter().cloned().collect()
        };
        info!(spec_id, count = keys.len(), "cleansing published keys");
        for key in &keys {
            if let Err(e) = session.delete(key).await {
                warn!(spec_id, key = key.as_str(), error = %e, "delete failed");
            }
        }
        // Wipe the in-memory record so a follow-up cleanse is cheap.
        {
            let mut rt = runtime.lock().await;
            rt.published_keys.clear();
        }
        Ok(())
    }

    /// Stop all per-spec publishers. Called from main on SIGINT.
    pub async fn shutdown_all(&mut self) {
        for h in self.specs.values_mut() {
            h.cancel_task();
        }
        info!(specs = self.specs.len(), "all per-spec publishers stopped");
    }
}

/// Per-spec publisher loop.
///
/// Owns the cadence ticker + payload generator + topic-pattern template;
/// runs until the cadence is exhausted or the task is aborted.
async fn run_spec(
    session: zenoh::Session,
    spec_id: i64,
    slug: String,
    topic_pattern: String,
    cadence: Cadence,
    payload: Payload,
    runtime: Arc<Mutex<SpecRuntime>>,
) {
    info!(spec_id, slug = slug.as_str(), topic = topic_pattern.as_str(), "publisher started");
    let mut ticker = CadenceTicker::new(cadence);
    loop {
        let wait = match ticker.next_wait() {
            Some(w) => w,
            None => {
                info!(spec_id, slug = slug.as_str(), "cadence exhausted; publisher idle");
                break;
            }
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        let seq = {
            let mut rt = runtime.lock().await;
            let s = rt.seq;
            rt.seq += 1;
            s
        };
        let resolved_topic = resolve_topic_pattern(&topic_pattern, seq);
        let publishes = payload.render(seq);
        for (suffix, bytes) in publishes {
            let key = if suffix.is_empty() {
                resolved_topic.clone()
            } else {
                format!("{resolved_topic}{suffix}")
            };
            match session.put(&key, bytes).await {
                Ok(()) => {
                    let mut rt = runtime.lock().await;
                    rt.record_key(key.clone());
                }
                Err(e) => {
                    warn!(spec_id, key = key.as_str(), error = %e, "publish failed");
                }
            }
        }
    }
    info!(spec_id, slug = slug.as_str(), "publisher task exiting");
}

/// Substitute `{seq}` / `{uuid}` / `{ts_ms}` markers in a topic_pattern.
/// Per docs/design/trashgoat.md §"Topic templating".
fn resolve_topic_pattern(pattern: &str, seq: u64) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            // Read until '}'; if we run off the end, emit the literal.
            let mut marker = String::new();
            let mut closed = false;
            for nc in chars.by_ref() {
                if nc == '}' {
                    closed = true;
                    break;
                }
                marker.push(nc);
            }
            if !closed {
                // Unterminated — emit literally.
                out.push('{');
                out.push_str(&marker);
                continue;
            }
            match marker.as_str() {
                "seq" => out.push_str(&seq.to_string()),
                "uuid" => out.push_str(&uuid::Uuid::new_v4().to_string()),
                "ts_ms" => out.push_str(&now_ms().to_string()),
                other => {
                    // Unknown marker — emit literally so the operator
                    // can see they typo'd it on the published key.
                    out.push('{');
                    out.push_str(other);
                    out.push('}');
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_state_parse() {
        assert_eq!(SpecState::from_str("draft"), Some(SpecState::Draft));
        assert_eq!(SpecState::from_str("active"), Some(SpecState::Active));
        assert_eq!(SpecState::from_str("paused"), Some(SpecState::Paused));
        assert_eq!(SpecState::from_str("retired"), Some(SpecState::Retired));
        assert_eq!(SpecState::from_str("invalid"), None);
    }

    #[test]
    fn topic_seq_substitution() {
        let out = resolve_topic_pattern("trashgoat/burst/v1/{seq}", 7);
        assert_eq!(out, "trashgoat/burst/v1/7");
    }

    #[test]
    fn topic_uuid_substitution_produces_distinct_keys() {
        let a = resolve_topic_pattern("trashgoat/x/v1/{uuid}", 0);
        let b = resolve_topic_pattern("trashgoat/x/v1/{uuid}", 0);
        assert_ne!(a, b);
        assert!(a.starts_with("trashgoat/x/v1/"));
        assert_eq!(a.len(), "trashgoat/x/v1/".len() + 36);
    }

    #[test]
    fn topic_ts_ms_substitution_increases() {
        let a = resolve_topic_pattern("k/{ts_ms}", 0);
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = resolve_topic_pattern("k/{ts_ms}", 0);
        // Lexically compare numeric suffixes.
        let a_num: u64 = a.trim_start_matches("k/").parse().unwrap();
        let b_num: u64 = b.trim_start_matches("k/").parse().unwrap();
        assert!(b_num >= a_num);
    }

    #[test]
    fn topic_unknown_marker_passes_through() {
        let out = resolve_topic_pattern("trashgoat/x/{wat}/v1", 0);
        assert_eq!(out, "trashgoat/x/{wat}/v1");
    }

    #[test]
    fn topic_literal_passthrough() {
        let out = resolve_topic_pattern("trashgoat/large-payload-12mb/v1", 999);
        assert_eq!(out, "trashgoat/large-payload-12mb/v1");
    }

    #[tokio::test]
    async fn retire_keeps_entry_for_followon_cleanse() {
        // Regression for the chaos-run bug: retire used to
        // `self.specs.remove(...)`, which made a subsequent cleanse
        // log "unknown spec; ignoring" and skip session.delete. After
        // the fix, retire marks state=Retired but keeps the entry so
        // cleanse-after-retire still walks the published-keys deque.
        let mut state = RunnerState::new(100.0, 100);
        state.specs.insert(
            42,
            SpecHandle::new(42, "chaos-bytes-1k".to_string(), 100),
        );
        // Seed a published key into the runtime so we can verify the
        // entry survives retire (the runtime is what cleanse walks).
        {
            let rt = state.specs.get(&42).unwrap().runtime.clone();
            let mut g = rt.lock().await;
            g.record_key("trashgoat/chaos-bytes-1k/v1".to_string());
        }
        state.retire(42);
        // Entry still present.
        assert!(state.specs.contains_key(&42));
        // State flipped to Retired.
        assert_eq!(state.specs.get(&42).unwrap().state, SpecState::Retired);
        // Published-key history preserved (cleanse can walk it).
        let rt = state.specs.get(&42).unwrap().runtime.clone();
        let g = rt.lock().await;
        assert_eq!(g.published_keys.len(), 1);
        assert_eq!(g.published_keys.front().unwrap(), "trashgoat/chaos-bytes-1k/v1");
    }

    #[tokio::test]
    async fn spec_runtime_record_key_dedups_consecutive() {
        let rt = Arc::new(Mutex::new(SpecRuntime::new(100)));
        let mut g = rt.lock().await;
        g.record_key("a".to_string());
        g.record_key("a".to_string());
        g.record_key("a".to_string());
        g.record_key("b".to_string());
        g.record_key("a".to_string());
        assert_eq!(g.published_keys.len(), 3);
    }

    #[tokio::test]
    async fn spec_runtime_record_key_bounded_by_max() {
        let rt = Arc::new(Mutex::new(SpecRuntime::new(3)));
        let mut g = rt.lock().await;
        g.record_key("a".to_string());
        g.record_key("b".to_string());
        g.record_key("c".to_string());
        g.record_key("d".to_string());
        assert_eq!(g.published_keys.len(), 3);
        // Oldest ("a") dropped.
        assert_eq!(g.published_keys.front().unwrap(), "b");
    }
}
