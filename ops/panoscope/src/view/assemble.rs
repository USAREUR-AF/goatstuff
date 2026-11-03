// SPDX-License-Identifier: Apache-2.0
//
// View assembly — what one viewer sees, cut from the shared fabric model.
//
// Order, and why:
//   1. Peers: window, scope (from raw publish roots), source / infra / test
//      filters. A hidden peer hides everything it publishes.
//   2. Topics: publisher visible, last event inside the window, and the key
//      inside the viewer's scope. Scope is applied to member topics BEFORE
//      grouping, so a group only ever counts what this viewer may see and a
//      group id never stands in for an authorization decision.
//   3. Organisations the viewer collapsed become one node each.
//   4. Each remaining publisher's single-publisher topics are grouped
//      (`group::group_topics`); topics shared by several visible publishers
//      stay individual pins so every publisher's edge reaches them.
//   5. Edges: publisher → pin/group, pin/group → subscriber. A group keeps its
//      members' subscribers, so a consumer of folded topics stays connected.
//   6. Element cap: if the response is too large, publishers are regrouped at
//      a smaller budget, and the response says so.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::view::group::{self, GroupKind, Unit};
use crate::view::model::FabricModel;
use crate::view::Fnv;
use crate::viewer::ViewerScope;

/// Upper bound on nodes + edges in one response. When grouping at the normal
/// budget exceeds it, publishers are regrouped at `REDUCED_BUDGETS` in turn
/// and `truncated` records the budget that fit (or that nothing did).
pub const MAX_ELEMENTS: usize = 5_000;
const REDUCED_BUDGETS: [usize; 2] = [8, 1];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Detail {
    #[default]
    Normal,
    /// Every publisher as one unit (the former SIMPLIFY view).
    Simplified,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFilter {
    #[default]
    All,
    Real,
    Notional,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InfraFilter {
    #[default]
    All,
    Hide,
    Only,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestFilter {
    #[default]
    Hide,
    Only,
}

/// Everything a viewer controls. Sent by the browser on every request, so a
/// reload or a deep link reproduces the view.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewParams {
    /// History window in hours; 0 = everything retained.
    pub window_hours: u64,
    pub detail: Detail,
    /// Group ids the operator opened.
    pub expand: Vec<String>,
    /// Peers the operator folded to a single unit.
    pub fold_peers: Vec<String>,
    /// Organisations (peer `vendor`) drawn as one node; `"*"` means all.
    pub collapse_orgs: Vec<String>,
    pub source: SourceFilter,
    pub infra: InfraFilter,
    pub test: TestFilter,
}

impl ViewParams {
    fn collapses(&self, org: &str) -> bool {
        self.collapse_orgs.iter().any(|o| o == "*" || o == org)
    }
}

/// One peer as this viewer sees it. Field names match `NodeView`, but the
/// two key lists are cut to the viewer: `publishes` holds only topics this
/// viewer can see (drawn as pins, or by the group label that holds them), and
/// `subscribes` only patterns inside the viewer's scope. `NodeView` carries
/// the raw lists, which name other organisations' keys.
#[derive(Serialize)]
pub struct PeerOut<'a> {
    pub peer_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot_alias: Option<&'a str>,
    pub display_name: Option<&'a str>,
    pub vendor: Option<&'a str>,
    pub role: crate::model::Role,
    pub publishes: Vec<String>,
    pub subscribes: Vec<&'a str>,
    pub first_seen_ms: u64,
    pub last_seen_ms: u64,
    pub site: Option<&'a str>,
    pub role_class: Option<&'a str>,
    pub format_hint: Option<&'a str>,
    pub synthetic: Option<bool>,
    pub node_kind: Option<crate::model::NodeKind>,
    pub attest_source: Option<&'a str>,
    pub locator: Option<&'a str>,
    pub node_class: panoscope_core::NodeClass,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_subkind: Option<&'static str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub universal_observer: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hygiene: Option<&'a crate::snapshot_view::NodeHygiene>,
    /// Topics this peer publishes that the viewer can see.
    pub visible_topics: usize,
    pub folded: bool,
}

/// Cap on a peer's `publishes` list; the count is always in `visible_topics`.
const PUBLISHES_LIST_MAX: usize = 50;

#[derive(Serialize)]
pub struct TopicOut<'a> {
    pub id: String,
    pub key: &'a str,
    pub publishers: Vec<&'a str>,
    pub projected_eps: f64,
    pub total_events: u64,
    pub last_event_ms: u64,
    pub format: Option<&'a str>,
    pub schema_family: Option<&'a str>,
    pub subscriber_count: usize,
}

#[derive(Serialize)]
pub struct GroupOut<'a> {
    pub id: String,
    pub kind: GroupKind,
    pub publisher: &'a str,
    pub label: String,
    pub selector: String,
    pub topic_count: usize,
    pub projected_eps: f64,
    pub total_events: u64,
    pub last_event_ms: u64,
    pub formats: BTreeMap<&'a str, usize>,
    pub subscriber_count: usize,
}

#[derive(Serialize)]
pub struct OrgOut {
    pub id: String,
    pub org: String,
    pub peer_count: usize,
    pub topic_count: usize,
    pub projected_eps: f64,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Publish,
    Subscribe,
}

#[derive(Serialize)]
pub struct EdgeOut {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    pub projected_eps: f64,
    pub total_events: u64,
    pub last_event_ms: u64,
    pub topic_count: usize,
}

#[derive(Serialize, Default)]
pub struct Totals {
    pub peers: usize,
    pub topics: usize,
    pub groups: usize,
    pub msg_per_sec: f64,
    pub total_events: u64,
    /// Distinct organisations among visible peers (the HUD's slot count).
    pub orgs: usize,
    pub elements: usize,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Truncation {
    pub max_elements: usize,
    /// Per-publisher budget the response was grouped at.
    pub publisher_budget: usize,
    /// True when even the smallest budget did not fit under the cap.
    pub over_cap: bool,
    /// True when the opened groups (`params.expand`) were ignored because the
    /// view did not fit under the cap with them.
    pub opened_ignored: bool,
}

/// The server's drawing limits, so the browser can say what an action will do
/// instead of guessing.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Limits {
    pub max_elements: usize,
    pub publisher_budget: usize,
    /// Most units a publisher draws with groups opened in it
    /// (`group::MAX_OPEN_UNITS`); a larger group stays closed.
    pub max_open_units: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_elements: MAX_ELEMENTS,
            publisher_budget: group::PUBLISHER_BUDGET,
            max_open_units: group::MAX_OPEN_UNITS,
        }
    }
}

#[derive(Serialize)]
pub struct ViewResponse<'a> {
    pub generation: u64,
    pub built_at_ms: u64,
    pub etag: String,
    pub peers: Vec<PeerOut<'a>>,
    pub topics: Vec<TopicOut<'a>>,
    pub groups: Vec<GroupOut<'a>>,
    pub orgs: Vec<OrgOut>,
    pub edges: Vec<EdgeOut>,
    pub totals: Totals,
    pub truncated: Option<Truncation>,
    pub limits: Limits,
    pub sampling_drops: BTreeMap<&'a str, u64>,
}

/// Stable id helpers — the browser keys its elements on these.
pub fn topic_id(key: &str) -> String {
    format!("topic:{key}")
}
pub fn org_id(org: &str) -> String {
    format!("org:{org}")
}

/// Visibility decided once per request, shared by assembly and the member /
/// search endpoints so all three agree on what this viewer can see.
pub struct Visible {
    pub peer: Vec<bool>,
    pub topic: Vec<bool>,
    /// Visible topics per publisher (model topic indices).
    pub topics_of: Vec<Vec<usize>>,
    /// Visible publisher count per topic key.
    pub publishers_of_key: HashMap<String, u32>,
}

pub fn visibility(model: &FabricModel, scope: &ViewerScope, params: &ViewParams, now_ms: u64) -> Visible {
    let cutoff = if params.window_hours == 0 {
        0
    } else {
        now_ms.saturating_sub(params.window_hours.saturating_mul(3_600_000))
    };
    let peer: Vec<bool> = model
        .peers
        .iter()
        .zip(model.facts.iter())
        .map(|(n, f)| {
            if n.last_seen_ms < cutoff {
                return false;
            }
            if !scope.allows_peer_facts(&n.peer_id, n.node_class, n.synthetic, &f.publish_roots) {
                return false;
            }
            let synthetic = n.synthetic == Some(true);
            let source_ok = match params.source {
                SourceFilter::All => true,
                SourceFilter::Real => !synthetic,
                SourceFilter::Notional => synthetic,
            };
            let infra = n.node_class == panoscope_core::NodeClass::Infra;
            let infra_ok = match params.infra {
                InfraFilter::All => true,
                InfraFilter::Hide => !infra,
                InfraFilter::Only => infra,
            };
            let test_ok = match params.test {
                TestFilter::Hide => !f.test_publisher,
                TestFilter::Only => f.test_publisher,
            };
            source_ok && infra_ok && test_ok
        })
        .collect();

    let mut topic = vec![false; model.topics.len()];
    let mut topics_of = vec![Vec::new(); model.peers.len()];
    let mut publishers_of_key: HashMap<String, u32> = HashMap::new();
    for (ti, t) in model.topics.iter().enumerate() {
        if !peer[t.publisher] || t.last_event_ms < cutoff {
            continue;
        }
        let synthetic = model.peers[t.publisher].synthetic == Some(true);
        if !scope.allows_topic(&t.key, synthetic) {
            continue;
        }
        topic[ti] = true;
        topics_of[t.publisher].push(ti);
        *publishers_of_key.entry(t.key.clone()).or_insert(0) += 1;
    }
    Visible { peer, topic, topics_of, publishers_of_key }
}

/// Group every visible, non-collapsed publisher at `budget_for(publisher)`.
/// Returns `(publisher, units)`; unit indices are model topic indices.
pub fn group_publishers(
    model: &FabricModel,
    vis: &Visible,
    params: &ViewParams,
    collapsed_peer: &[bool],
    budget_for: &dyn Fn(usize) -> usize,
) -> Vec<(usize, Vec<Unit>)> {
    let pinned: HashSet<String> = params.expand.iter().cloned().collect();
    let mut out = Vec::new();
    for (pi, topics) in vis.topics_of.iter().enumerate() {
        if topics.is_empty() || collapsed_peer[pi] {
            continue;
        }
        let mut units = Vec::new();
        let mut single: Vec<(&str, usize)> = Vec::new();
        for &ti in topics {
            let key = model.topics[ti].key.as_str();
            if vis.publishers_of_key.get(key).copied().unwrap_or(0) > 1 {
                units.push(Unit::Topic(ti));
            } else {
                single.push((key, ti));
            }
        }
        units.extend(group::group_topics_cached(&model.peers[pi].peer_id, &single, budget_for(pi), &pinned));
        out.push((pi, units));
    }
    out
}

pub fn assemble<'a>(
    model: &'a FabricModel,
    scope: &ViewerScope,
    params: &ViewParams,
    now_ms: u64,
) -> ViewResponse<'a> {
    assemble_with_units(model, scope, params, now_ms).0
}

/// `assemble`, also returning the grouping it drew, so member listing and
/// search answer from exactly the units the viewer sees (including the budget
/// the element cap may have forced).
pub fn assemble_with_units<'a>(
    model: &'a FabricModel,
    scope: &ViewerScope,
    params: &ViewParams,
    now_ms: u64,
) -> (ViewResponse<'a>, Vec<(usize, Vec<Unit>)>) {
    let vis = visibility(model, scope, params, now_ms);

    // Collapsed organisations.
    let collapsed_peer: Vec<bool> = model
        .peers
        .iter()
        .enumerate()
        .map(|(i, n)| vis.peer[i] && n.vendor.as_deref().is_some_and(|v| params.collapses(v)))
        .collect();

    let base_budget = |pi: usize| -> usize {
        if params.detail == Detail::Simplified || params.fold_peers.iter().any(|p| p == &model.peers[pi].peer_id) {
            1
        } else {
            group::PUBLISHER_BUDGET
        }
    };

    let mut grouped = group_publishers(model, &vis, params, &collapsed_peer, &base_budget);
    let mut response = build(model, scope, params, &vis, &collapsed_peer, &grouped);
    let mut truncated = None;
    if response.totals.elements > MAX_ELEMENTS {
        let mut fit = false;
        let mut used = group::PUBLISHER_BUDGET;
        for b in REDUCED_BUDGETS {
            used = b;
            let reduced = |pi: usize| base_budget(pi).min(b);
            grouped = group_publishers(model, &vis, params, &collapsed_peer, &reduced);
            response = build(model, scope, params, &vis, &collapsed_peer, &grouped);
            if response.totals.elements <= MAX_ELEMENTS {
                fit = true;
                break;
            }
        }
        let mut opened_ignored = false;
        if !fit && !params.expand.is_empty() {
            // Opened groups are what the operator asked for, but not at the
            // price of an unbounded response: fall back to the closed view.
            let closed = ViewParams { expand: Vec::new(), ..params.clone() };
            let one = |pi: usize| base_budget(pi).min(1);
            grouped = group_publishers(model, &vis, &closed, &collapsed_peer, &one);
            response = build(model, scope, &closed, &vis, &collapsed_peer, &grouped);
            fit = response.totals.elements <= MAX_ELEMENTS;
            opened_ignored = true;
        }
        truncated = Some(Truncation { max_elements: MAX_ELEMENTS, publisher_budget: used, over_cap: !fit, opened_ignored });
    }
    response.truncated = truncated;
    response.etag = etag(model.generation, scope, params, now_ms);
    (response, grouped)
}

fn build<'a>(
    model: &'a FabricModel,
    scope: &ViewerScope,
    params: &ViewParams,
    vis: &Visible,
    collapsed_peer: &[bool],
    grouped: &[(usize, Vec<Unit>)],
) -> ViewResponse<'a> {
    // Where a subscriber peer renders: itself, its collapsed org, or nowhere.
    let render_id = |si: usize| -> Option<String> {
        if !vis.peer[si] {
            None
        } else if collapsed_peer[si] {
            model.peers[si].vendor.as_deref().map(org_id)
        } else {
            Some(model.peers[si].peer_id.clone())
        }
    };

    #[derive(Default)]
    struct Agg {
        eps: f64,
        events: u64,
        last: u64,
        topics: usize,
    }
    let mut edges: BTreeMap<(String, String, EdgeKind), Agg> = BTreeMap::new();
    let mut add = |from: String, to: String, kind: EdgeKind, eps: f64, events: u64, last: u64| {
        let a = edges.entry((from, to, kind)).or_default();
        a.eps += eps;
        a.events += events;
        a.last = a.last.max(last);
        a.topics += 1;
    };

    let mut topics_out: Vec<TopicOut> = Vec::new();
    let mut topic_pin_index: HashMap<&str, usize> = HashMap::new();
    let mut groups_out: Vec<GroupOut> = Vec::new();

    for (pi, units) in grouped {
        let publisher = model.peers[*pi].peer_id.as_str();
        for u in units {
            match u {
                Unit::Topic(ti) => {
                    let t = &model.topics[*ti];
                    let id = topic_id(&t.key);
                    let slot = *topic_pin_index.entry(t.key.as_str()).or_insert_with(|| {
                        topics_out.push(TopicOut {
                            id: id.clone(),
                            key: &t.key,
                            publishers: Vec::new(),
                            projected_eps: 0.0,
                            total_events: 0,
                            last_event_ms: 0,
                            format: t.format.as_deref(),
                            schema_family: t.schema_family.as_deref(),
                            subscriber_count: 0,
                        });
                        topics_out.len() - 1
                    });
                    let pin = &mut topics_out[slot];
                    pin.publishers.push(publisher);
                    pin.projected_eps += t.projected_eps;
                    pin.total_events += t.total_events;
                    pin.last_event_ms = pin.last_event_ms.max(t.last_event_ms);
                    add(publisher.to_string(), id.clone(), EdgeKind::Publish, t.projected_eps, t.total_events, t.last_event_ms);
                    let mut subs = 0;
                    for &si in &t.subscribers {
                        if let Some(to) = render_id(si) {
                            subs += 1;
                            add(id.clone(), to, EdgeKind::Subscribe, t.projected_eps, t.total_events, t.last_event_ms);
                        }
                    }
                    topics_out[slot].subscriber_count = topics_out[slot].subscriber_count.max(subs);
                }
                Unit::Group(g) => {
                    let mut eps = 0.0;
                    let mut events = 0u64;
                    let mut last = 0u64;
                    let mut formats: BTreeMap<&str, usize> = BTreeMap::new();
                    let mut sub_agg: BTreeMap<String, (f64, u64, u64)> = BTreeMap::new();
                    for &ti in &g.members {
                        let t = &model.topics[ti];
                        eps += t.projected_eps;
                        events += t.total_events;
                        last = last.max(t.last_event_ms);
                        *formats.entry(t.format.as_deref().unwrap_or("unknown")).or_insert(0) += 1;
                        for &si in &t.subscribers {
                            if let Some(to) = render_id(si) {
                                let e = sub_agg.entry(to).or_insert((0.0, 0, 0));
                                e.0 += t.projected_eps;
                                e.1 += t.total_events;
                                e.2 = e.2.max(t.last_event_ms);
                            }
                        }
                    }
                    add(publisher.to_string(), g.id.clone(), EdgeKind::Publish, eps, events, last);
                    let subscriber_count = sub_agg.len();
                    for (to, (e, ev, l)) in sub_agg {
                        add(g.id.clone(), to, EdgeKind::Subscribe, e, ev, l);
                    }
                    groups_out.push(GroupOut {
                        id: g.id.clone(),
                        kind: g.kind,
                        publisher,
                        label: g.label.clone(),
                        selector: g.selector.clone(),
                        topic_count: g.members.len(),
                        projected_eps: eps,
                        total_events: events,
                        last_event_ms: last,
                        formats,
                        subscriber_count,
                    });
                }
            }
        }
    }

    // Collapsed organisations: one node each; their publishers' topics become
    // org → subscriber edges, intra-org traffic is dropped.
    let mut orgs: BTreeMap<String, OrgOut> = BTreeMap::new();
    for (pi, n) in model.peers.iter().enumerate() {
        if !collapsed_peer[pi] {
            continue;
        }
        let org = n.vendor.clone().unwrap_or_default();
        let entry = orgs.entry(org.clone()).or_insert_with(|| OrgOut {
            id: org_id(&org),
            org: org.clone(),
            peer_count: 0,
            topic_count: 0,
            projected_eps: 0.0,
        });
        entry.peer_count += 1;
        for &ti in &vis.topics_of[pi] {
            let t = &model.topics[ti];
            entry.topic_count += 1;
            entry.projected_eps += t.projected_eps;
            for &si in &t.subscribers {
                if let Some(to) = render_id(si) {
                    if to != org_id(&org) {
                        add(org_id(&org), to, EdgeKind::Subscribe, t.projected_eps, t.total_events, t.last_event_ms);
                    }
                }
            }
        }
    }

    let fold_set: HashSet<&str> = params.fold_peers.iter().map(String::as_str).collect();
    // What each publisher's `publishes` list says: its drawn units, as topic
    // keys or group labels, never a topic this viewer cannot see.
    let mut unit_labels: HashMap<usize, Vec<String>> = HashMap::new();
    for (pi, units) in grouped {
        let labels = unit_labels.entry(*pi).or_default();
        for u in units {
            match u {
                Unit::Topic(ti) => labels.push(model.topics[*ti].key.clone()),
                Unit::Group(g) => labels.push(format!("{} ({} topics)", g.label, g.members.len())),
            }
        }
        labels.sort();
        labels.truncate(PUBLISHES_LIST_MAX);
    }
    let peers: Vec<PeerOut> = model
        .peers
        .iter()
        .enumerate()
        .filter(|(i, _)| vis.peer[*i] && !collapsed_peer[*i])
        .map(|(i, n)| PeerOut {
            peer_id: &n.peer_id,
            slot_alias: n.slot_alias.as_deref(),
            display_name: n.display_name.as_deref(),
            vendor: n.vendor.as_deref(),
            role: n.role,
            publishes: unit_labels.remove(&i).unwrap_or_default(),
            subscribes: n.subscribes.iter().map(String::as_str).filter(|p| scope.allows_key(p)).collect(),
            first_seen_ms: n.first_seen_ms,
            last_seen_ms: n.last_seen_ms,
            site: n.site.as_deref(),
            role_class: n.role_class.as_deref(),
            format_hint: n.format_hint.as_deref(),
            synthetic: n.synthetic,
            node_kind: n.node_kind,
            attest_source: n.attest_source.as_deref(),
            locator: n.locator.as_deref(),
            node_class: n.node_class,
            node_subkind: n.node_subkind,
            universal_observer: n.universal_observer,
            hygiene: n.hygiene.as_ref(),
            visible_topics: vis.topics_of[i].len(),
            folded: fold_set.contains(n.peer_id.as_str()),
        })
        .collect();

    let edges: Vec<EdgeOut> = edges
        .into_iter()
        .map(|((from, to, kind), a)| EdgeOut {
            from,
            to,
            kind,
            projected_eps: a.eps,
            total_events: a.events,
            last_event_ms: a.last,
            topic_count: a.topics,
        })
        .collect();

    let mut org_names: HashSet<&str> = HashSet::new();
    let mut totals = Totals::default();
    for (i, n) in model.peers.iter().enumerate() {
        if !vis.peer[i] {
            continue;
        }
        totals.peers += 1;
        match n.vendor.as_deref() {
            Some(v) => {
                org_names.insert(v);
            }
            None => {
                if !is_hex_zid(&n.peer_id) {
                    if let Some(head) = n.peer_id.split('/').next() {
                        org_names.insert(head);
                    }
                }
            }
        }
    }
    totals.orgs = org_names.len();
    totals.topics = vis.publishers_of_key.len();
    for (ti, t) in model.topics.iter().enumerate() {
        if vis.topic[ti] {
            totals.msg_per_sec += t.projected_eps;
            totals.total_events += t.total_events;
        }
    }
    totals.groups = groups_out.len();
    totals.elements = peers.len() + topics_out.len() + groups_out.len() + orgs.len() + edges.len();

    let sampling_drops: BTreeMap<&str, u64> = model
        .sampling_drops
        .iter()
        .filter(|(k, _)| scope.allows_key(k))
        .map(|(k, v)| (k.as_str(), *v))
        .collect();

    ViewResponse {
        generation: model.generation,
        built_at_ms: model.built_at_ms,
        etag: String::new(),
        peers,
        topics: topics_out,
        groups: groups_out,
        orgs: orgs.into_values().collect(),
        edges,
        totals,
        truncated: None,
        limits: Limits::default(),
        sampling_drops,
    }
}

fn is_hex_zid(s: &str) -> bool {
    (16..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Same model generation + same viewer scope + same parameters ⇒ same bytes.
/// Entity tag of one view. The generation changes when the picture does; the
/// minute is included for a windowed view, because a topic ages out of the
/// window with no change to the model, and an idle fabric would otherwise
/// keep answering 304 with a topic that should have left.
pub fn etag(generation: u64, scope: &ViewerScope, params: &ViewParams, now_ms: u64) -> String {
    let mut h = Fnv::new();
    h.write_u64(generation);
    if params.window_hours > 0 {
        h.write_u64(now_ms / 60_000);
    }
    h.write(scope.cache_key().as_bytes());
    h.write(serde_json::to_string(params).unwrap_or_default().as_bytes());
    format!("\"v{generation}-{:016x}\"", h.finish())
}

/// The response cache's key: the same inputs as `etag`, kept as values rather
/// than hashed. The tag may collide harmlessly; the key that decides whose body
/// a request is served may not (see `view::cache`).
pub fn view_key(
    scope: &ViewerScope,
    params: &ViewParams,
    now_ms: u64,
) -> Result<crate::view::cache::ViewKey, serde_json::Error> {
    Ok(crate::view::cache::ViewKey {
        minute: if params.window_hours > 0 { now_ms / 60_000 } else { 0 },
        scope: scope.cache_key(),
        params: serde_json::to_string(params)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FlowState, Topology};
        use crate::view::model::build_model;
    use crate::view::subs::SubscriptionIndex;

    const NOW: u64 = 10 * 3_600_000;

    /// A small fabric: an outlier chunk streamer with two subscribers, a small
    /// org publisher, a synthetic producer, a load-test publisher, and a topic
    /// published by two peers.
    fn fabric() -> FabricModel {
        let mut t = Topology::default();
        let mut f = FlowState::default();
        let mut publish = |t: &mut Topology, f: &mut FlowState, peer: &str, key: &str, at: u64| {
            t.record_publish_declaration(peer.into(), key.into(), at);
            f.record_event(peer.into(), key.into(), at, 5.0);
        };
        let a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        for s in 0..9 {
            for c in 0..30 {
                for p in 0..(1 + c % 5) {
                    publish(&mut t, &mut f, "chunker", &format!("{a}/imagery/v1/stream/{s:08x}/chunk/{c:04}/{p:06}"), NOW - 1_000);
                }
            }
        }
        publish(&mut t, &mut f, "chunker", &format!("{a}/hello/v1"), NOW - 1_000);
        t.record_subscribe_declaration("viewer-1".into(), format!("{a}/imagery/v1/**"), NOW);
        t.record_subscribe_declaration("viewer-2".into(), format!("{a}/**"), NOW);
        let b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        for i in 0..3 {
            publish(&mut t, &mut f, "small", &format!("{b}/status/item-{i}/v1"), NOW - 2_000);
        }
        // Undeclared publishes from two peers: no schema-declared owner, so the
        // topic keeps both publishers (a declared topic collapses to its owner).
        for peer in ["small", "other"] {
            t.upsert_peer(peer.into(), NOW - 2_000);
            f.record_event(peer.into(), "shared/topic/v1".into(), NOW - 2_000, 5.0);
        }
        publish(&mut t, &mut f, "synth", "notional/tarsier/v1", NOW - 2_000);
        t.peers.get_mut("synth").unwrap().synthetic = Some(true);
        publish(&mut t, &mut f, "lt", "cccccccccccccccccccccccccccccccc/loadtest/1", NOW - 2_000);
        // An old topic outside a 1 h window.
        publish(&mut t, &mut f, "small", &format!("{b}/old/v1"), NOW - 5 * 3_600_000);
        t.peers.get_mut("small").unwrap().last_seen_ms = NOW - 2_000;
        t.peers.get_mut("small").unwrap().vendor = Some("orgb".into());
        let mut idx = SubscriptionIndex::default();
        build_model(&t, &f, &|_| None, &panoscope_core::AliasRegistry::empty(), &mut idx, &FabricModel::empty(), NOW)
    }

    fn ids(v: &ViewResponse) -> HashSet<String> {
        v.peers.iter().map(|p| p.peer_id.to_string())
            .chain(v.topics.iter().map(|t| t.id.clone()))
            .chain(v.groups.iter().map(|g| g.id.clone()))
            .chain(v.orgs.iter().map(|o| o.id.clone()))
            .collect()
    }

    #[test]
    fn every_visible_topic_is_drawn_exactly_once() {
        let m = fabric();
        let v = assemble(&m, &ViewerScope::Full, &ViewParams::default(), NOW);
        let pinned: usize = v.topics.iter().map(|t| t.publishers.len()).sum();
        let grouped: usize = v.groups.iter().map(|g| g.topic_count).sum();
        let visible = visibility(&m, &ViewerScope::Full, &ViewParams::default(), NOW).topic.iter().filter(|&&x| x).count();
        assert_eq!(pinned + grouped, visible);
        // Every edge endpoint is an element in the response.
        let all = ids(&v);
        for e in &v.edges {
            assert!(all.contains(&e.from) && all.contains(&e.to), "dangling edge {} -> {}", e.from, e.to);
        }
    }

    #[test]
    fn the_chunk_streamer_folds_and_its_subscribers_stay_connected() {
        let m = fabric();
        let v = assemble(&m, &ViewerScope::Full, &ViewParams::default(), NOW);
        let chunker_groups: Vec<&GroupOut> = v.groups.iter().filter(|g| g.publisher == "chunker").collect();
        assert!(!chunker_groups.is_empty());
        assert!(v.topics.iter().filter(|t| t.publishers.contains(&"chunker")).count() <= group::PUBLISHER_BUDGET);
        for viewer in ["viewer-1", "viewer-2"] {
            assert!(
                v.edges.iter().any(|e| e.to == viewer && e.kind == EdgeKind::Subscribe),
                "{viewer} keeps an edge to what it consumes"
            );
        }
    }

    #[test]
    fn a_shared_topic_is_one_pin_with_both_publishers() {
        let m = fabric();
        let v = assemble(&m, &ViewerScope::Full, &ViewParams::default(), NOW);
        let pin = v.topics.iter().find(|t| t.key == "shared/topic/v1").expect("shared pin");
        let mut pubs = pin.publishers.clone();
        pubs.sort();
        assert_eq!(pubs, vec!["other", "small"]);
    }

    #[test]
    fn scoped_viewer_never_receives_a_foreign_topic_name() {
        let m = fabric();
        let scope = ViewerScope::Scoped {
            anchors: ["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string()].into_iter().collect(),
            include_notional: false,
        };
        let v = assemble(&m, &scope, &ViewParams::default(), NOW);
        let body = serde_json::to_string(&v).unwrap();
        assert!(!body.contains("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"), "no chunker key leaks");
        assert!(!body.contains("notional/tarsier"), "no notional key without the grant");
        assert!(body.contains("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/status"));
    }

    #[test]
    fn window_hides_old_topics_not_just_old_peers() {
        let m = fabric();
        let all = assemble(&m, &ViewerScope::Full, &ViewParams::default(), NOW);
        let one_hour = ViewParams { window_hours: 1, ..Default::default() };
        let w = assemble(&m, &ViewerScope::Full, &one_hour, NOW);
        let key = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/old/v1";
        assert!(serde_json::to_string(&all).unwrap().contains(key));
        assert!(!serde_json::to_string(&w).unwrap().contains(key), "neither drawn nor listed");
    }

    #[test]
    fn test_publishers_are_hidden_by_default_and_alone_in_test_only() {
        let m = fabric();
        let v = assemble(&m, &ViewerScope::Full, &ViewParams::default(), NOW);
        assert!(!v.peers.iter().any(|p| p.peer_id == "lt"));
        let only = assemble(&m, &ViewerScope::Full, &ViewParams { test: TestFilter::Only, ..Default::default() }, NOW);
        let peers: Vec<&str> = only.peers.iter().map(|p| p.peer_id).collect();
        assert_eq!(peers, vec!["lt"]);
    }

    #[test]
    fn simplified_and_folded_peers_are_one_unit() {
        let m = fabric();
        let s = assemble(&m, &ViewerScope::Full, &ViewParams { detail: Detail::Simplified, ..Default::default() }, NOW);
        let chunker_units = s.groups.iter().filter(|g| g.publisher == "chunker").count()
            + s.topics.iter().filter(|t| t.publishers == vec!["chunker"]).count();
        assert_eq!(chunker_units, 1);
        let f = assemble(&m, &ViewerScope::Full, &ViewParams { fold_peers: vec!["chunker".into()], ..Default::default() }, NOW);
        assert!(f.peers.iter().any(|p| p.peer_id == "chunker" && p.folded));
    }

    #[test]
    fn collapsed_org_is_one_node_with_outbound_edges_only() {
        let m = fabric();
        let v = assemble(&m, &ViewerScope::Full, &ViewParams { collapse_orgs: vec!["orgb".into()], ..Default::default() }, NOW);
        assert!(v.orgs.iter().any(|o| o.id == "org:orgb" && o.peer_count == 1));
        assert!(!v.peers.iter().any(|p| p.peer_id == "small"));
        let all = ids(&v);
        assert!(v.edges.iter().all(|e| all.contains(&e.from) && all.contains(&e.to)));
    }

    #[test]
    fn expanding_a_group_opens_it_and_the_etag_changes() {
        let m = fabric();
        let base = assemble(&m, &ViewerScope::Full, &ViewParams::default(), NOW);
        let g = base.groups.iter().find(|g| g.publisher == "chunker").unwrap().id.clone();
        let p = ViewParams { expand: vec![g.clone()], ..Default::default() };
        let opened = assemble(&m, &ViewerScope::Full, &p, NOW);
        assert!(!opened.groups.iter().any(|x| x.id == g));
        assert_ne!(base.etag, opened.etag);
        assert_eq!(base.etag, assemble(&m, &ViewerScope::Full, &ViewParams::default(), NOW).etag);
    }

    #[test]
    fn opened_groups_that_break_the_cap_are_ignored_and_reported() {
        // Three publishers of 1,900 entities each: every family is openable on
        // its own (under MAX_OPEN_UNITS), but all three opened draw ~11,400
        // elements, which no budget reduction can shrink.
        let mut t = Topology::default();
        let mut f = FlowState::default();
        for p in 0..3 {
            for e in 0..1_900 {
                let key = format!("pub{p}/entities/e{e}");
                t.record_publish_declaration(format!("p{p}"), key.clone(), NOW);
                f.record_event(format!("p{p}"), key, NOW, 1.0);
            }
        }
        let mut idx = SubscriptionIndex::default();
        let m = build_model(&t, &f, &|_| None, &panoscope_core::AliasRegistry::empty(), &mut idx, &FabricModel::empty(), NOW);
        let closed = assemble(&m, &ViewerScope::Full, &ViewParams::default(), NOW);
        assert!(closed.truncated.is_none());
        let expand: Vec<String> = closed.groups.iter().map(|g| g.id.clone()).collect();
        assert_eq!(expand.len(), 3);
        let one = assemble(&m, &ViewerScope::Full, &ViewParams { expand: expand[..1].to_vec(), ..Default::default() }, NOW);
        assert!(one.truncated.is_none(), "one opened family fits");
        assert_eq!(one.totals.elements, closed.totals.elements - 2 + 2 * 1_900);
        let all = assemble(&m, &ViewerScope::Full, &ViewParams { expand, ..Default::default() }, NOW);
        let tr = all.truncated.as_ref().expect("cap reported");
        assert!(tr.opened_ignored && !tr.over_cap, "{tr:?}");
        assert!(all.totals.elements <= MAX_ELEMENTS);
        assert_eq!(all.groups.len(), 3);
    }

    #[test]
    fn element_cap_regroups_and_says_so() {
        // 1,500 publishers of 3 distinct topics each: nothing folds at the
        // normal budget (3 pins + 3 edges + 1 peer each = 10,500 elements), so
        // the view regroups every publisher to one unit (4,500) and reports it.
        let mut t = Topology::default();
        let mut f = FlowState::default();
        for p in 0..1500 {
            for w in ["alpha", "bravo", "charlie"] {
                let key = format!("pub{p}/{w}/v1");
                t.record_publish_declaration(format!("p{p}"), key.clone(), NOW);
                f.record_event(format!("p{p}"), key, NOW, 5.0);
            }
        }
        let mut idx = SubscriptionIndex::default();
        let m = build_model(&t, &f, &|_| None, &panoscope_core::AliasRegistry::empty(), &mut idx, &FabricModel::empty(), NOW);
        let v = assemble(&m, &ViewerScope::Full, &ViewParams::default(), NOW);
        let tr = v.truncated.as_ref().expect("cap reported");
        assert!(v.totals.elements <= MAX_ELEMENTS, "{} elements", v.totals.elements);
        assert_eq!(tr.publisher_budget, 1);
        assert!(!tr.over_cap);
        assert_eq!(v.totals.topics, 4_500, "totals count what exists, not what is drawn");
    }

    #[test]
    fn a_visible_peer_does_not_leak_other_orgs_keys_through_its_lists() {
        // A peer granted by one anchor that also publishes and subscribes under
        // another org's anchor.
        let mut t = Topology::default();
        let mut f = FlowState::default();
        for key in ["mine000000000000000000000000000a/t/v1", "theirs0000000000000000000000000b/t/v1"] {
            t.record_publish_declaration("dual".into(), key.into(), NOW);
            f.record_event("dual".into(), key.into(), NOW, 5.0);
        }
        t.record_subscribe_declaration("dual".into(), "theirs0000000000000000000000000b/**".into(), NOW);
        let mut idx = SubscriptionIndex::default();
        let m = build_model(&t, &f, &|_| None, &panoscope_core::AliasRegistry::empty(), &mut idx, &FabricModel::empty(), NOW);
        let scope = ViewerScope::Scoped {
            anchors: ["mine000000000000000000000000000a".to_string()].into_iter().collect(),
            include_notional: false,
        };
        let v = assemble(&m, &scope, &ViewParams::default(), NOW);
        assert_eq!(v.peers.len(), 1);
        let body = serde_json::to_string(&v).unwrap();
        assert!(!body.contains("theirs"), "{body}");
    }
}

#[cfg(test)]
mod timing {
    use super::*;
    use crate::model::{FlowState, Topology};
    use crate::view::model::build_model;
    use crate::view::subs::SubscriptionIndex;

    /// A chunk-streamer shape: one streamer with thousands of topics and 23
    /// wildcard subscribers, plus 150 small publishers.
    /// `cargo test --release --bin panoscope view_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn view_timing() {
        const NOW: u64 = 10 * 3_600_000;
        let mut t = Topology::default();
        let mut f = FlowState::default();
        let a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let mut n = 0;
        'outer: for s in 0..9 {
            for c in 0..400 {
                for p in 0..5 {
                    let key = format!("{a}/imagery/v1/stream/{s:08x}/chunk/{c:04}/{p:06}");
                    t.record_publish_declaration("chunker".into(), key.clone(), NOW);
                    f.record_event("chunker".into(), key, NOW, 5.0);
                    n += 1;
                    if n >= 8_998 {
                        break 'outer;
                    }
                }
            }
        }
        for v in 0..23 {
            t.record_subscribe_declaration(format!("viewer-{v}"), format!("{a}/imagery/v1/**"), NOW);
        }
        for p in 0..150 {
            for i in 0..3 {
                let key = format!("pub{p:03}/status/item-{i}/v1");
                t.record_publish_declaration(format!("p{p}"), key.clone(), NOW);
                f.record_event(format!("p{p}"), key, NOW, 5.0);
            }
        }
        let mut idx = SubscriptionIndex::default();
        let t0 = std::time::Instant::now();
        let m = build_model(&t, &f, &|_| None, &panoscope_core::AliasRegistry::empty(), &mut idx, &FabricModel::empty(), NOW);
        let cold_build = t0.elapsed();
        let mut m2 = build_model(&t, &f, &|_| None, &panoscope_core::AliasRegistry::empty(), &mut idx, &m, NOW);
        let mut warm_build = std::time::Duration::MAX;
        for _ in 0..5 {
            let t1 = std::time::Instant::now();
            let next = build_model(&t, &f, &|_| None, &panoscope_core::AliasRegistry::empty(), &mut idx, &m2, NOW);
            let took = t1.elapsed();
            if took < warm_build {
                warm_build = took;
            }
            m2 = next;
        }
        let p = ViewParams::default();
        let t2 = std::time::Instant::now();
        let v = assemble(&m2, &ViewerScope::Full, &p, NOW);
        let cold_view = t2.elapsed();
        let body_len = serde_json::to_vec(&v).unwrap().len();
        drop(v);
        let mut warm = std::time::Duration::MAX;
        for _ in 0..10 {
            let t3 = std::time::Instant::now();
            let v = assemble(&m2, &ViewerScope::Full, &p, NOW);
            let _ = serde_json::to_vec(&v).unwrap();
            warm = warm.min(t3.elapsed());
        }
        println!(
            "topics={} links={} | model build: cold {:.1} ms, warm best-of-5 {:.1} ms | view: first {:.1} ms, repeat best-of-10 {:.1} ms incl. JSON | body {} bytes",
            m2.build.topics, m2.build.subscriber_links,
            cold_build.as_secs_f64() * 1e3, warm_build.as_secs_f64() * 1e3,
            cold_view.as_secs_f64() * 1e3, warm.as_secs_f64() * 1e3, body_len
        );
        println!("warm phases: {:?}", m2.build.phase_ms);
    }
}
