// SPDX-License-Identifier: Apache-2.0
//
// The view model as a running service: the builder task that keeps
// `SharedState::view_model` current, and the lookups the HTTP layer serves
// from it (group members, topic search with the groups to open).
//
// The builder copies what it reads under each ingest lock (milliseconds) and
// builds with no lock held, so the data tail — which takes those locks for
// write on every sample — never waits on a build. Requests read an
// `Arc<FabricModel>` and never touch the ingest locks at all.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::sample_buffer::SampleFormat;
use crate::view::assemble::{self, topic_id, ViewParams};
use crate::view::group::Unit;
use crate::view::model::{build_model, FabricModel};
use crate::view::subs::SubscriptionIndex;
use crate::viewer::ViewerScope;
use crate::zenoh_client::SharedState;

/// Fastest rebuild cadence. One build per tick serves every viewer.
pub const MODEL_TICK: Duration = Duration::from_millis(1_000);

/// Wait before the next build: `max(MODEL_TICK, 2 × last build)`.
///
/// A fixed 1 s tick with a build that itself takes ~1.1 s (a large goatnet:
/// 1.07–1.17 s) rebuilt back to back, pinning a core and copying topology and
/// flow under their read locks every second. Tying the gap to the build time
/// keeps the builder at no more than about a third of a core, however large
/// the fabric grows.
pub fn next_model_tick(last_build: Duration) -> Duration {
    MODEL_TICK.max(last_build.saturating_mul(2))
}

/// Build once now (so the first request after boot is not empty), then keep
/// building, waiting `next_model_tick(last build)` between builds.
pub fn spawn_model_builder(state: SharedState) {
    tokio::spawn(async move {
        let index = Arc::new(Mutex::new(SubscriptionIndex::default()));
        loop {
            let started = std::time::Instant::now();
            if let Err(e) = rebuild(&state, &index).await {
                warn!(error = %e, "view model: build failed; serving the previous model");
            }
            tokio::time::sleep(next_model_tick(started.elapsed())).await;
        }
    });
}

/// Key-shape hygiene report for one publisher, from the shared model's topic
/// records (no probe, no ingest lock). A publisher the model does not render
/// gets the verdict for an empty key space, as before.
pub fn publisher_hygiene(model: &FabricModel, source: &str) -> panoscope_core::PublisherHygiene {
    let stats: Vec<panoscope_core::TopicStat> = model
        .peer(source)
        .map(|pi| {
            model.facts[pi]
                .topics
                .iter()
                .map(|&ti| {
                    let t = &model.topics[ti];
                    panoscope_core::TopicStat {
                        key: t.key.clone(),
                        total_events: t.total_events,
                        has_schema: t.schema_family.is_some() || t.format.is_some(),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    panoscope_core::classify_publisher(&stats)
}

pub async fn rebuild(state: &SharedState, index: &Arc<Mutex<SubscriptionIndex>>) -> anyhow::Result<()> {
    let topology = state.topology.read().await.clone();
    let flow = state.flow.read().await.clone();
    let aliases = state.aliases.read().await.clone();
    let formats: HashMap<String, SampleFormat> = {
        let samples = state.samples.read().await;
        flow.edges
            .values()
            .filter_map(|e| samples.latest_format(&e.topic).map(|f| (e.topic.clone(), f)))
            .collect()
    };
    let prev = state.view_model.read().await.clone();
    let index = index.clone();
    let now = crate::model::now_ms();
    let model = tokio::task::spawn_blocking(move || {
        let mut idx = index.lock().expect("subscription index lock");
        build_model(&topology, &flow, &|t| formats.get(t).copied(), &aliases, &mut idx, &prev, now)
    })
    .await?;
    debug!(
        generation = model.generation,
        build_ms = model.build.build_ms,
        topics = model.build.topics,
        "view model: built"
    );
    *state.view_model.write().await = Arc::new(model);
    Ok(())
}

// ---------------------------------------------------------------------------
// Group members
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct GroupQuery {
    #[serde(default)]
    pub params: ViewParams,
    pub id: String,
    /// Case-insensitive substring filter on the topic key.
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    200
}

#[derive(Serialize)]
pub struct MemberRow<'a> {
    pub key: &'a str,
    pub projected_eps: f64,
    pub total_events: u64,
    pub last_event_ms: u64,
    pub format: Option<&'a str>,
    pub schema_family: Option<&'a str>,
    pub subscriber_count: usize,
}

#[derive(Serialize)]
pub struct GroupMembers<'a> {
    pub id: String,
    pub generation: u64,
    /// Members of the group this viewer can see, before the `q` filter.
    pub total: usize,
    /// Members matching `q`.
    pub matched: usize,
    pub rows: Vec<MemberRow<'a>>,
}

/// Members of one group as the viewer's own view would draw it. `None` when
/// the id is not a group in that view (never formed, opened, or out of scope).
pub fn group_members<'a>(
    model: &'a FabricModel,
    scope: &ViewerScope,
    q: &GroupQuery,
    now_ms: u64,
) -> Option<GroupMembers<'a>> {
    let (_, grouped) = assemble::assemble_with_units(model, scope, &q.params, now_ms);
    let members = grouped.iter().flat_map(|(_, units)| units.iter()).find_map(|u| match u {
        Unit::Group(g) if g.id == q.id => Some(g.members.clone()),
        _ => None,
    })?;
    let needle = q.q.to_lowercase();
    let mut matched: Vec<usize> = members
        .iter()
        .copied()
        .filter(|&ti| needle.is_empty() || model.topics[ti].key.to_lowercase().contains(&needle))
        .collect();
    matched.sort_by(|&a, &b| model.topics[a].key.cmp(&model.topics[b].key));
    let rows = matched
        .iter()
        .skip(q.offset)
        .take(q.limit.min(1_000))
        .map(|&ti| {
            let t = &model.topics[ti];
            MemberRow {
                key: &t.key,
                projected_eps: t.projected_eps,
                total_events: t.total_events,
                last_event_ms: t.last_event_ms,
                format: t.format.as_deref(),
                schema_family: t.schema_family.as_deref(),
                subscriber_count: t.subscribers.len(),
            }
        })
        .collect();
    Some(GroupMembers {
        id: q.id.clone(),
        generation: model.generation,
        total: members.len(),
        matched: matched.len(),
        rows,
    })
}

// ---------------------------------------------------------------------------
// Search + reveal
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    pub params: ViewParams,
    pub q: String,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
}

fn default_search_limit() -> usize {
    30
}

#[derive(Serialize)]
pub struct SearchHit<'a> {
    pub key: &'a str,
    pub publisher: &'a str,
    /// The element drawing this topic in the current view: `topic:<key>` or a
    /// group id.
    pub drawn_as: String,
}

#[derive(Serialize)]
pub struct SearchResult<'a> {
    pub generation: u64,
    pub total: usize,
    pub hits: Vec<SearchHit<'a>>,
}

/// Visible topics whose key contains `q` (case-insensitive), each with the
/// element that draws it in the viewer's current view.
pub fn search<'a>(model: &'a FabricModel, scope: &ViewerScope, q: &SearchQuery, now_ms: u64) -> SearchResult<'a> {
    let needle = q.q.to_lowercase();
    let (_, grouped) = assemble::assemble_with_units(model, scope, &q.params, now_ms);
    let mut drawn: HashMap<usize, String> = HashMap::new();
    for (_, units) in &grouped {
        for u in units {
            match u {
                Unit::Topic(ti) => {
                    drawn.insert(*ti, topic_id(&model.topics[*ti].key));
                }
                Unit::Group(g) => {
                    for &ti in &g.members {
                        drawn.insert(ti, g.id.clone());
                    }
                }
            }
        }
    }
    let mut hits: Vec<(usize, String)> = drawn
        .into_iter()
        .filter(|(ti, _)| needle.is_empty() || model.topics[*ti].key.to_lowercase().contains(&needle))
        .collect();
    hits.sort_by(|a, b| model.topics[a.0].key.cmp(&model.topics[b.0].key));
    let total = hits.len();
    SearchResult {
        generation: model.generation,
        total,
        hits: hits
            .into_iter()
            .take(q.limit.min(200))
            .map(|(ti, drawn_as)| SearchHit {
                key: &model.topics[ti].key,
                publisher: &model.peers[model.topics[ti].publisher].peer_id,
                drawn_as,
            })
            .collect(),
    }
}

#[derive(Debug, Deserialize)]
pub struct RevealQuery {
    #[serde(default)]
    pub params: ViewParams,
    pub key: String,
    pub publisher: String,
}

#[derive(Serialize)]
pub struct Reveal {
    /// Group ids to add to `params.expand` so the topic is drawn as its own
    /// pin. Empty when it already is.
    pub expand: Vec<String>,
    /// `topic:<key>` when revealed; `None` when the topic is not visible to
    /// this viewer or could not be opened within the depth limit.
    pub topic_id: Option<String>,
}

/// Open groups one level at a time until the topic is its own pin.
pub fn reveal(model: &FabricModel, scope: &ViewerScope, q: &RevealQuery, now_ms: u64) -> Reveal {
    const MAX_DEPTH: usize = 16;
    let Some(pi) = model.peer(&q.publisher) else {
        return Reveal { expand: Vec::new(), topic_id: None };
    };
    let mut params = q.params.clone();
    let already: HashSet<String> = params.expand.iter().cloned().collect();
    let mut added = Vec::new();
    for _ in 0..MAX_DEPTH {
        let (_, grouped) = assemble::assemble_with_units(model, scope, &params, now_ms);
        let Some((_, units)) = grouped.iter().find(|(p, _)| *p == pi) else { break };
        let holder = units.iter().find_map(|u| match u {
            Unit::Topic(ti) if model.topics[*ti].key == q.key => Some(None),
            Unit::Group(g) if g.members.iter().any(|&ti| model.topics[ti].key == q.key) => Some(Some(g.id.clone())),
            _ => None,
        });
        match holder {
            Some(None) => return Reveal { expand: added, topic_id: Some(topic_id(&q.key)) },
            Some(Some(gid)) if !already.contains(&gid) && !added.contains(&gid) => {
                params.expand.push(gid.clone());
                added.push(gid);
            }
            _ => break,
        }
    }
    Reveal { expand: added, topic_id: None }
}

// ---------------------------------------------------------------------------
// Topic list (for `goat flow` and other programs)
// ---------------------------------------------------------------------------

/// Most rows one `/api/topics` response carries.
pub const MAX_TOPIC_ROWS: usize = 50_000;

#[derive(Debug, Deserialize)]
pub struct TopicsQuery {
    /// Key expression the listed topics must be included in; every topic when
    /// absent.
    #[serde(default)]
    pub key: Option<String>,
    /// Hours of history, as in the view; 0 lists everything the model holds.
    #[serde(default)]
    pub window_hours: u64,
    #[serde(default = "default_topics_limit")]
    pub limit: usize,
}

fn default_topics_limit() -> usize {
    10_000
}

#[derive(Serialize)]
pub struct TopicRow<'a> {
    pub key: &'a str,
    pub publishers: Vec<&'a str>,
    pub projected_eps: f64,
    pub total_events: u64,
    pub last_event_ms: u64,
    pub format: Option<&'a str>,
    pub schema_family: Option<&'a str>,
    /// Subscribers this viewer can see.
    pub subscriber_count: usize,
}

#[derive(Serialize)]
pub struct TopicList<'a> {
    pub generation: u64,
    /// When the model was built, on panoscope's clock. Ages are computed
    /// against this, never the caller's clock.
    pub built_at_ms: u64,
    /// Topics matching the query before `limit`.
    pub total: usize,
    pub truncated: bool,
    pub topics: Vec<TopicRow<'a>>,
}

/// Every topic the viewer can see, one row per key, never grouped — the
/// per-topic answer a program wants, where the view answers what to draw.
/// Visibility is the view's rule (the publisher is visible and the key is in
/// scope) without the view's display filters (source, infra, test).
pub fn topics<'a>(model: &'a FabricModel, scope: &ViewerScope, q: &TopicsQuery, now_ms: u64) -> TopicList<'a> {
    use crate::snapshot_view::keyexpr_matches;
    let cutoff = if q.window_hours == 0 { 0 } else { now_ms.saturating_sub(q.window_hours.saturating_mul(3_600_000)) };
    let peer_visible: Vec<bool> = model
        .peers
        .iter()
        .zip(model.facts.iter())
        .map(|(n, f)| n.last_seen_ms >= cutoff && scope.allows_peer_facts(&n.peer_id, n.node_class, n.synthetic, &f.publish_roots))
        .collect();
    let mut rows: std::collections::BTreeMap<&str, (TopicRow<'a>, HashSet<usize>)> = std::collections::BTreeMap::new();
    for t in &model.topics {
        if !peer_visible[t.publisher] || t.last_event_ms < cutoff {
            continue;
        }
        let publisher = &model.peers[t.publisher];
        if !scope.allows_topic(&t.key, publisher.synthetic == Some(true)) {
            continue;
        }
        if let Some(k) = q.key.as_deref() {
            if !keyexpr_matches(k, &t.key) {
                continue;
            }
        }
        let (row, subs) = rows.entry(&t.key).or_insert_with(|| {
            (
                TopicRow {
                    key: &t.key,
                    publishers: Vec::new(),
                    projected_eps: 0.0,
                    total_events: 0,
                    last_event_ms: 0,
                    format: t.format.as_deref(),
                    schema_family: t.schema_family.as_deref(),
                    subscriber_count: 0,
                },
                HashSet::new(),
            )
        });
        row.publishers.push(&publisher.peer_id);
        row.projected_eps += t.projected_eps;
        row.total_events += t.total_events;
        row.last_event_ms = row.last_event_ms.max(t.last_event_ms);
        subs.extend(t.subscribers.iter().copied().filter(|&si| peer_visible[si]));
    }
    let total = rows.len();
    let limit = q.limit.min(MAX_TOPIC_ROWS);
    TopicList {
        generation: model.generation,
        built_at_ms: model.built_at_ms,
        total,
        truncated: total > limit,
        topics: rows
            .into_values()
            .take(limit)
            .map(|(mut row, subs)| {
                row.subscriber_count = subs.len();
                row
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fast_build_keeps_the_one_second_tick() {
        assert_eq!(next_model_tick(Duration::from_millis(0)), MODEL_TICK);
        assert_eq!(next_model_tick(Duration::from_millis(120)), MODEL_TICK);
        assert_eq!(next_model_tick(Duration::from_millis(500)), MODEL_TICK);
    }

    /// The slow-build case: a 1.1 s build must not be followed by
    /// another build 1 s later.
    #[test]
    fn a_slow_build_waits_twice_its_duration() {
        assert_eq!(next_model_tick(Duration::from_millis(1_100)), Duration::from_millis(2_200));
        assert_eq!(next_model_tick(Duration::from_secs(30)), Duration::from_secs(60));
    }

    #[test]
    fn an_absurd_build_time_does_not_overflow() {
        assert_eq!(next_model_tick(Duration::MAX), Duration::MAX);
    }
}
