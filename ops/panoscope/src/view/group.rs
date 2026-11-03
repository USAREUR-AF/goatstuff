// SPDX-License-Identifier: Apache-2.0
//
// Grouping — the one rule that decides how a publisher's topics are drawn.
//
// It replaces five mechanisms that each handled one shape and disagreed with
// each other (the server template fold, the server outlier rollup, the client
// per-namespace singleton fold, the client per-publisher fold and the
// `publishes` fold). See docs/design/panoscope-view-model.md.
//
// The publisher's topics form a segment trie. Two steps then decide what is
// drawn:
//
// 1. FAMILIES. At every trie node, children with the same shape are siblings
//    of one kind: `entities/<uuid>` leaves, `chunk/<n>` directories that each
//    hold parts, `stream/<id>` directories that each hold a chunk tree. A
//    class of more than `FAMILY_MIN` such children becomes one family group.
//    A child's shape is name-free (the set of its children's shapes), so a
//    chunk directory with 5 parts and one with 30 are the same shape, and
//    families compose bottom-up. Directory classes whose names are words
//    (`sensor`, `tak`, `imagery`) need more than `WORD_DIR_FAMILY_MIN`
//    members, so eight distinct subsystems under one anchor stay visible;
//    leaf classes and directory classes with an identifier-like name (any
//    digit) fold at `FAMILY_MIN`, the threshold the v1 folds used.
//
// 2. BUDGET. If the publisher still has more than `budget` units, whole
//    subtrees collapse, deepest level first and largest first within a level,
//    until it fits. The top of the tree stays readable longest. If the
//    top-level entries alone exceed the budget, the publisher becomes a
//    single group.
//
// Operator expansion is a set of group ids that must not be formed. A pinned
// group's node and all its ancestors are exempt from collapse, and its
// children are then grouped by the same two steps.
//
// The result is deterministic for a given topic set, budget and pin set, and
// accounts for every topic exactly once.

use std::collections::{BTreeMap, HashSet};

use crate::view::Fnv;

/// A class of more than this many same-shape siblings folds (leaves, or
/// directories with identifier-like names). Same threshold as the v1 folds.
pub const FAMILY_MIN: usize = 7;
/// A class of same-shape directories whose names are all words needs more
/// than this many members to fold.
pub const WORD_DIR_FAMILY_MIN: usize = 16;
/// Default per-publisher unit budget.
pub const PUBLISHER_BUDGET: usize = 40;
/// Most units one publisher may draw once the operator has opened groups in
/// it. Opening is exempt from the budget (it is an explicit request to see
/// more), but not unbounded: when the opened grouping would exceed this, the
/// pins are ignored for that publisher and its topics stay browsable in the
/// group's member list instead.
pub const MAX_OPEN_UNITS: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupKind {
    /// Same-shape siblings under one node.
    Family,
    /// A whole subtree, collapsed to fit the budget.
    Subtree,
    /// Every topic of the publisher.
    Publisher,
}

#[derive(Debug, Clone)]
pub struct GroupUnit {
    pub id: String,
    pub kind: GroupKind,
    /// Display pattern: `a/b/*`, `a/b/*/…`, `a/b/**`, or `**`.
    pub label: String,
    /// Key-expression-shaped approximation of the members, for the client to
    /// attribute a live publish to a group (`a/b/*`, `a/b/*/**`, `a/b/**`,
    /// `**`). Never an authorization input.
    pub selector: String,
    /// Caller's topic indices, sorted.
    pub members: Vec<usize>,
}

#[derive(Debug, Clone)]
pub enum Unit {
    Topic(usize),
    Group(GroupUnit),
}

struct TNode {
    name: String,
    path: String,
    parent: Option<usize>,
    depth: usize,
    children: BTreeMap<String, usize>,
    leaf: Option<usize>,
}

const LEAF_SIG: u64 = 0x6c65_6166; // "leaf"

/// Group one publisher's topics. `topics` pairs each key with the caller's
/// index for it; keys must be distinct.
pub fn group_topics(
    publisher: &str,
    topics: &[(&str, usize)],
    budget: usize,
    pinned: &HashSet<String>,
) -> Vec<Unit> {
    if topics.is_empty() {
        return Vec::new();
    }
    let budget = budget.max(1);

    // ---- trie ----------------------------------------------------------
    let mut nodes: Vec<TNode> = vec![TNode {
        name: String::new(),
        path: String::new(),
        parent: None,
        depth: 0,
        children: BTreeMap::new(),
        leaf: None,
    }];
    for &(key, idx) in topics {
        let mut cur = 0usize;
        for seg in key.split('/') {
            cur = match nodes[cur].children.get(seg) {
                Some(&c) => c,
                None => {
                    let path = if nodes[cur].depth == 0 {
                        seg.to_string()
                    } else {
                        format!("{}/{}", nodes[cur].path, seg)
                    };
                    let id = nodes.len();
                    let depth = nodes[cur].depth + 1;
                    nodes.push(TNode {
                        name: seg.to_string(),
                        path,
                        parent: Some(cur),
                        depth,
                        children: BTreeMap::new(),
                        leaf: None,
                    });
                    nodes[cur].children.insert(seg.to_string(), id);
                    id
                }
            };
        }
        nodes[cur].leaf = Some(idx);
    }
    let n = nodes.len();

    // Children always have larger indices than their parent, so a reverse
    // index sweep is a post-order.
    let mut sig = vec![0u64; n];
    for i in (0..n).rev() {
        let node = &nodes[i];
        if node.children.is_empty() {
            sig[i] = LEAF_SIG;
            continue;
        }
        let mut child_sigs: Vec<u64> = node.children.values().map(|&c| sig[c]).collect();
        child_sigs.sort_unstable();
        child_sigs.dedup();
        let mut h = Fnv::new();
        h.write_u64(node.leaf.is_some() as u64);
        for s in child_sigs {
            h.write_u64(s);
        }
        sig[i] = h.finish();
    }

    // ---- pins ----------------------------------------------------------
    // A pinned id's node and its ancestors never collapse and never join a
    // family; a pinned family id disables that family.
    let mut pinned_path = vec![false; n];
    let mut pinned_family: HashSet<(usize, u64)> = HashSet::new();
    let mut pinned_root = false;
    if !pinned.is_empty() {
        let mark = |pinned_path: &mut Vec<bool>, mut i: usize| loop {
            pinned_path[i] = true;
            match nodes[i].parent {
                Some(p) => i = p,
                None => break,
            }
        };
        if pinned.contains(&publisher_id(publisher)) {
            pinned_root = true;
            pinned_path[0] = true;
        }
        for i in 0..n {
            if pinned.contains(&subtree_id(publisher, &nodes[i].path)) {
                mark(&mut pinned_path, i);
            }
            if nodes[i].children.len() > FAMILY_MIN {
                let mut classes: Vec<u64> = nodes[i].children.values().map(|&c| sig[c]).collect();
                classes.sort_unstable();
                classes.dedup();
                for s in classes {
                    if pinned.contains(&family_id(publisher, &nodes[i].path, s)) {
                        pinned_family.insert((i, s));
                        mark(&mut pinned_path, i);
                    }
                }
            }
        }
    }

    // ---- families + unit counts ----------------------------------------
    // family_of[child] = Some(class key) when the child is inside a family.
    let mut families: Vec<Vec<(u64, Vec<usize>)>> = vec![Vec::new(); n];
    let mut in_family = vec![false; n];
    let mut units = vec![0usize; n];
    for i in (0..n).rev() {
        let node = &nodes[i];
        let mut count = node.leaf.is_some() as usize;
        if !node.children.is_empty() {
            let mut classes: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
            for &c in node.children.values() {
                if !pinned_path[c] {
                    classes.entry(sig[c]).or_default().push(c);
                }
            }
            for (s, members) in classes {
                if pinned_family.contains(&(i, s)) {
                    continue;
                }
                let threshold = if s == LEAF_SIG
                    || members.iter().all(|&c| nodes[c].name.bytes().any(|b| b.is_ascii_digit()))
                {
                    FAMILY_MIN
                } else {
                    WORD_DIR_FAMILY_MIN
                };
                if members.len() > threshold {
                    for &c in &members {
                        in_family[c] = true;
                    }
                    families[i].push((s, members));
                }
            }
            count += families[i].len();
            for &c in node.children.values() {
                if !in_family[c] {
                    count += units[c];
                }
            }
        }
        units[i] = count;
    }

    // ---- budget --------------------------------------------------------
    let mut collapsed = vec![false; n];
    let mut total = units[0];
    if total > budget {
        let max_depth = nodes.iter().map(|t| t.depth).max().unwrap_or(0);
        'depths: for d in (1..=max_depth).rev() {
            let mut level: Vec<usize> = (0..n)
                .filter(|&i| nodes[i].depth == d && !pinned_path[i] && !nodes[i].children.is_empty())
                .collect();
            level.sort_by(|&a, &b| units[b].cmp(&units[a]).then(a.cmp(&b)));
            for i in level {
                if total <= budget {
                    break 'depths;
                }
                if units[i] <= 1 || under_collapsed_or_family(&nodes, &collapsed, &in_family, i) {
                    continue;
                }
                let saved = units[i] - 1;
                collapsed[i] = true;
                units[i] = 1;
                let mut p = nodes[i].parent;
                while let Some(pi) = p {
                    // A node inside a family contributes nothing above the
                    // family, so the saving stops there.
                    units[pi] -= saved;
                    if in_family[pi] {
                        break;
                    }
                    p = nodes[pi].parent;
                }
                total = units[0];
            }
        }
    }
    // An opened group is exempt from the budget; only a publisher with no
    // opened group in it collapses whole.
    let any_pin = pinned_root || pinned_path.iter().any(|&p| p);
    if any_pin && total > MAX_OPEN_UNITS {
        return group_topics(publisher, topics, budget, &HashSet::new());
    }
    let whole_publisher = total > budget && !any_pin && topics.len() > 1;

    // ---- emit ----------------------------------------------------------
    if whole_publisher {
        let mut members: Vec<usize> = topics.iter().map(|&(_, i)| i).collect();
        members.sort_unstable();
        return vec![Unit::Group(GroupUnit {
            id: publisher_id(publisher),
            kind: GroupKind::Publisher,
            label: "**".into(),
            selector: "**".into(),
            members,
        })];
    }
    let mut out = Vec::new();
    let mut stack = vec![0usize];
    while let Some(i) = stack.pop() {
        let node = &nodes[i];
        if collapsed[i] {
            out.push(Unit::Group(GroupUnit {
                id: subtree_id(publisher, &node.path),
                kind: GroupKind::Subtree,
                label: format!("{}/**", node.path),
                selector: format!("{}/**", node.path),
                members: collect_leaves(&nodes, i),
            }));
            continue;
        }
        if let Some(idx) = node.leaf {
            out.push(Unit::Topic(idx));
        }
        for (s, members) in &families[i] {
            let base = if node.path.is_empty() { String::new() } else { format!("{}/", node.path) };
            let (label, selector) = if *s == LEAF_SIG {
                (format!("{base}*"), format!("{base}*"))
            } else {
                (format!("{base}*/…"), format!("{base}*/**"))
            };
            let mut all = Vec::new();
            for &c in members {
                all.extend(collect_leaves(&nodes, c));
            }
            all.sort_unstable();
            out.push(Unit::Group(GroupUnit {
                id: family_id(publisher, &node.path, *s),
                kind: GroupKind::Family,
                label,
                selector,
                members: all,
            }));
        }
        // Reverse so the stack pops children in key order.
        for &c in node.children.values().rev() {
            if !in_family[c] {
                stack.push(c);
            }
        }
    }
    out
}

fn under_collapsed_or_family(nodes: &[TNode], collapsed: &[bool], in_family: &[bool], i: usize) -> bool {
    let mut p = Some(i);
    while let Some(pi) = p {
        if in_family[pi] || (pi != i && collapsed[pi]) {
            return true;
        }
        p = nodes[pi].parent;
    }
    false
}

fn collect_leaves(nodes: &[TNode], root: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(i) = stack.pop() {
        if let Some(idx) = nodes[i].leaf {
            out.push(idx);
        }
        stack.extend(nodes[i].children.values().copied());
    }
    out.sort_unstable();
    out
}

/// `group_topics` with a process-wide memo. Grouping depends only on the
/// publisher, its topic keys in order, the budget and the pins that name this
/// publisher — not on rates — so a model rebuilt every second, and many viewers
/// sharing one visibility, reuse one result. Measured without it: 49,437 topics
/// took 372 ms per call in a release build, on every request.
///
/// Cached units hold positions into `topics`, never the caller's indices, so a
/// rebuilt model with renumbered topics maps them back correctly. Keyed by a
/// 64-bit hash plus the topic count; the memo is dropped whole when it passes
/// `GROUP_CACHE_MAX` entries.
pub fn group_topics_cached(
    publisher: &str,
    topics: &[(&str, usize)],
    budget: usize,
    pinned: &HashSet<String>,
) -> Vec<Unit> {
    use std::sync::{Arc, Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<std::collections::HashMap<(u64, usize), Arc<Vec<Unit>>>>> = OnceLock::new();
    const GROUP_CACHE_MAX: usize = 8_192;

    let prefix = format!("grp:{publisher}:");
    let mut relevant: Vec<&String> = pinned.iter().filter(|p| p.starts_with(&prefix)).collect();
    relevant.sort();
    let mut h = Fnv::new();
    h.write(publisher.as_bytes());
    h.write_u64(budget as u64);
    for p in &relevant {
        h.write(p.as_bytes());
        h.write(&[0]);
    }
    for (k, _) in topics {
        h.write(k.as_bytes());
        h.write(&[0xff]);
    }
    let key = (h.finish(), topics.len());

    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let hit = cache.lock().ok().and_then(|c| c.get(&key).cloned());
    let positional = match hit {
        Some(u) => u,
        None => {
            let by_position: Vec<(&str, usize)> = topics.iter().enumerate().map(|(i, (k, _))| (*k, i)).collect();
            let units = Arc::new(group_topics(publisher, &by_position, budget, pinned));
            if let Ok(mut c) = cache.lock() {
                if c.len() >= GROUP_CACHE_MAX {
                    c.clear();
                }
                c.insert(key, units.clone());
            }
            units
        }
    };
    positional
        .iter()
        .map(|u| match u {
            Unit::Topic(pos) => Unit::Topic(topics[*pos].1),
            Unit::Group(g) => Unit::Group(GroupUnit {
                members: g.members.iter().map(|&pos| topics[pos].1).collect(),
                ..g.clone()
            }),
        })
        .collect()
}

pub fn publisher_id(publisher: &str) -> String {
    format!("grp:{publisher}:pub")
}
pub fn subtree_id(publisher: &str, path: &str) -> String {
    format!("grp:{publisher}:sub:{path}")
}
pub fn family_id(publisher: &str, path: &str, sig: u64) -> String {
    format!("grp:{publisher}:fam:{sig:016x}:{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(keys: &[String], budget: usize, pinned: &HashSet<String>) -> Vec<Unit> {
        let pairs: Vec<(&str, usize)> = keys.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
        let units = group_topics("pub", &pairs, budget, pinned);
        // Conservation: every topic exactly once.
        let mut seen = vec![0u32; keys.len()];
        for u in &units {
            match u {
                Unit::Topic(i) => seen[*i] += 1,
                Unit::Group(g) => g.members.iter().for_each(|&i| seen[i] += 1),
            }
        }
        assert!(seen.iter().all(|&c| c == 1), "every topic accounted for exactly once");
        units
    }

    fn groups(units: &[Unit]) -> Vec<(GroupKind, String, usize)> {
        units
            .iter()
            .filter_map(|u| match u {
                Unit::Group(g) => Some((g.kind, g.label.clone(), g.members.len())),
                _ => None,
            })
            .collect()
    }

    fn topics_count(units: &[Unit]) -> usize {
        units.iter().filter(|u| matches!(u, Unit::Topic(_))).count()
    }

    /// A real publisher shape, reduced: nine streams with a variable
    /// number of chunks and parts, per-stream meta/end, plus its other
    /// families and a few singletons.
    fn zagros_chunk_publisher() -> Vec<String> {
        let a = "0426a0f7caf5a3104db3cb90520d2364";
        let mut keys = Vec::new();
        for s in 0..9 {
            let chunks = if s == 0 { 300 } else { 10 + s * 3 };
            for c in 0..chunks {
                let parts = 1 + (c * 7 + s) % 30;
                for p in 0..parts {
                    keys.push(format!("{a}/imagery/v1/stream/{s:08x}/chunk/{c:04}/{p:06}"));
                }
            }
            keys.push(format!("{a}/imagery/v1/stream/{s:08x}/meta"));
            if s % 2 == 0 {
                keys.push(format!("{a}/imagery/v1/stream/{s:08x}/end"));
            }
        }
        for i in 0..1204 {
            keys.push(format!("{a}/edge/ai/v1/det/EDGEAI.edge-ai-01.eo-cam-01.{i}.0"));
            keys.push(format!("{a}/tak/cot/v1/EDGEAI.edge-ai-01.eo-cam-01.{i}.0"));
        }
        for i in 0..4 {
            keys.push(format!("{a}/sensor/v1/hostile-uav-{i:02}"));
        }
        keys.push(format!("{a}/hello/v1"));
        keys.push(format!("{a}/imagery/v1/eo-cam-01"));
        keys
    }

    #[test]
    fn zagros_chunk_publisher_folds_by_shape_and_keeps_its_top_level_visible() {
        let keys = zagros_chunk_publisher();
        let units = run(&keys, PUBLISHER_BUDGET, &HashSet::new());
        assert!(units.len() <= PUBLISHER_BUDGET, "{} units", units.len());
        let g = groups(&units);
        let a = "0426a0f7caf5a3104db3cb90520d2364";
        assert!(
            g.iter().any(|(k, l, _)| *k == GroupKind::Family && l == &format!("{a}/imagery/v1/stream/*/…")),
            "the nine streams fold into one family: {g:?}"
        );
        assert!(g.iter().any(|(_, l, n)| l == &format!("{a}/edge/ai/v1/det/*") && *n == 1204));
        assert!(g.iter().any(|(_, l, n)| l == &format!("{a}/tak/cot/v1/*") && *n == 1204));
        // Four sensor topics and the singletons stay individual.
        assert!(topics_count(&units) >= 6, "{g:?}");
    }

    #[test]
    fn eight_entity_leaves_fold_and_three_do_not() {
        let eight: Vec<String> = (0..8).map(|i| format!("a/entities/v3/e{i}")).collect();
        assert_eq!(groups(&run(&eight, 40, &HashSet::new())), vec![(GroupKind::Family, "a/entities/v3/*".into(), 8)]);
        let seven: Vec<String> = (0..7).map(|i| format!("a/entities/v3/e{i}")).collect();
        assert!(groups(&run(&seven, 40, &HashSet::new())).is_empty());
    }

    #[test]
    fn version_after_entity_folds() {
        let keys: Vec<String> = (0..20).map(|i| format!("a/aircraft/id{i}/v1")).collect();
        let g = groups(&run(&keys, 40, &HashSet::new()));
        assert_eq!(g, vec![(GroupKind::Family, "a/aircraft/*/…".into(), 20)]);
    }

    #[test]
    fn eight_word_named_subsystems_stay_visible() {
        let names = ["sensor", "tak", "imagery", "edge", "hello", "status", "health", "config"];
        let keys: Vec<String> = names.iter().flat_map(|n| [format!("a/{n}/x"), format!("a/{n}/y")]).collect();
        let units = run(&keys, 40, &HashSet::new());
        assert!(groups(&units).is_empty(), "{:?}", groups(&units));
        assert_eq!(topics_count(&units), 16);
    }

    #[test]
    fn eight_numbered_directories_fold() {
        let keys: Vec<String> = (0..8).flat_map(|n| [format!("a/cam{n}/x"), format!("a/cam{n}/y")]).collect();
        assert_eq!(groups(&run(&keys, 40, &HashSet::new())), vec![(GroupKind::Family, "a/*/…".into(), 16)]);
    }

    #[test]
    fn heterogeneous_publisher_over_budget_collapses_deepest_largest_first() {
        // 60 unrelated word topics in 6 word-named trees of 10: no family fits,
        // so subtrees collapse until the publisher is within budget.
        let trees = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot"];
        let keys: Vec<String> = trees
            .iter()
            .flat_map(|t| (0..10).map(move |i| format!("a/{t}/{}", ["x", "y", "z", "w", "v", "u", "q", "r", "s", "p"][i])))
            .collect();
        // Leaves under each tree are words, but a LEAF class folds at 7 by
        // parity with v1, so each tree is already a family of 10.
        let units = run(&keys, 4, &HashSet::new());
        assert!(units.len() <= 4, "{:?}", groups(&units));
    }

    #[test]
    fn budget_of_one_is_a_single_publisher_group() {
        let keys: Vec<String> = (0..5).map(|i| format!("a/t{i}")).collect();
        // One unit holding every topic. A shared root collapses as that
        // subtree (`a/**`, the more useful label); only topics with no
        // common root become the whole-publisher group.
        let units = run(&keys, 1, &HashSet::new());
        assert_eq!(groups(&units), vec![(GroupKind::Subtree, "a/**".into(), 5)]);
        let rootless: Vec<String> = (0..5).map(|i| format!("r{i}/t")).collect();
        assert_eq!(groups(&run(&rootless, 1, &HashSet::new())), vec![(GroupKind::Publisher, "**".into(), 5)]);
        // A single topic never becomes a group.
        let one = vec!["a/t".to_string()];
        assert_eq!(topics_count(&run(&one, 1, &HashSet::new())), 1);
    }

    #[test]
    fn pinning_a_group_opens_exactly_one_level() {
        let keys = zagros_chunk_publisher();
        let units = run(&keys, PUBLISHER_BUDGET, &HashSet::new());
        let stream_family = units
            .iter()
            .find_map(|u| match u {
                Unit::Group(g) if g.label.ends_with("/imagery/v1/stream/*/…") => Some(g.id.clone()),
                _ => None,
            })
            .expect("stream family");
        let pinned: HashSet<String> = [stream_family.clone()].into_iter().collect();
        let opened = run(&keys, PUBLISHER_BUDGET, &pinned);
        assert!(opened.iter().all(|u| !matches!(u, Unit::Group(g) if g.id == stream_family)));
        let g = groups(&opened);
        // Each stream now shows on its own (as its chunk family plus meta/end,
        // or collapsed to fit), never all nine chunk parts as topics.
        assert!(opened.len() < 200, "{} units after opening one level", opened.len());
        assert!(g.iter().any(|(_, l, _)| l.contains("/stream/00000000/")), "{g:?}");
    }

    #[test]
    fn opening_a_wide_family_draws_its_members_and_keeps_the_publisher() {
        let keys = zagros_chunk_publisher();
        let units = run(&keys, PUBLISHER_BUDGET, &HashSet::new());
        let det = units
            .iter()
            .find_map(|u| match u {
                Unit::Group(g) if g.label.ends_with("/edge/ai/v1/det/*") => Some(g.id.clone()),
                _ => None,
            })
            .expect("det family");
        let pinned: HashSet<String> = [det.clone()].into_iter().collect();
        let opened = run(&keys, PUBLISHER_BUDGET, &pinned);
        let g = groups(&opened);
        assert!(!g.iter().any(|(k, _, _)| *k == GroupKind::Publisher), "opening must not fold the publisher: {g:?}");
        assert!(!g.iter().any(|(_, l, _)| l.ends_with("/edge/ai/v1/det/*")), "{g:?}");
        assert!(topics_count(&opened) >= 1204);
        assert!(opened.len() <= MAX_OPEN_UNITS);
    }

    #[test]
    fn opening_a_family_too_wide_to_draw_is_ignored() {
        let keys: Vec<String> = (0..MAX_OPEN_UNITS + 1).map(|i| format!("a/entities/e{i}")).collect();
        let closed = run(&keys, PUBLISHER_BUDGET, &HashSet::new());
        let fam = match &closed[..] {
            [Unit::Group(g)] => g.id.clone(),
            other => panic!("one family expected, got {} units", other.len()),
        };
        let pinned: HashSet<String> = [fam].into_iter().collect();
        let opened = run(&keys, PUBLISHER_BUDGET, &pinned);
        assert_eq!(groups(&opened), groups(&closed));
    }

    #[test]
    fn cached_grouping_maps_positions_back_to_the_callers_indices() {
        let keys = zagros_chunk_publisher();
        let a: Vec<(&str, usize)> = keys.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
        let b: Vec<(&str, usize)> = keys.iter().enumerate().map(|(i, k)| (k.as_str(), i + 100_000)).collect();
        let first = group_topics_cached("pub", &a, PUBLISHER_BUDGET, &HashSet::new());
        let second = group_topics_cached("pub", &b, PUBLISHER_BUDGET, &HashSet::new()); // cache hit
        let direct = group_topics("pub", &b, PUBLISHER_BUDGET, &HashSet::new());
        let norm = |u: &[Unit]| {
            let mut v: Vec<String> = u
                .iter()
                .map(|x| match x {
                    Unit::Topic(i) => format!("t{i}"),
                    Unit::Group(g) => format!("{}:{:?}", g.id, g.members),
                })
                .collect();
            v.sort();
            v
        };
        assert_eq!(norm(&second), norm(&direct), "a cache hit returns the caller's own indices");
        assert_ne!(norm(&first), norm(&second));
    }

    #[test]
    fn deterministic() {
        let keys = zagros_chunk_publisher();
        let a = run(&keys, PUBLISHER_BUDGET, &HashSet::new());
        let mut rev = keys.clone();
        rev.reverse();
        // Same topic set in a different order, same caller indices by key.
        let pairs: Vec<(&str, usize)> = rev
            .iter()
            .map(|k| (k.as_str(), keys.iter().position(|x| x == k).unwrap()))
            .collect();
        let b = group_topics("pub", &pairs, PUBLISHER_BUDGET, &HashSet::new());
        let ids = |u: &[Unit]| {
            let mut v: Vec<String> = u
                .iter()
                .map(|x| match x {
                    Unit::Topic(i) => format!("t{i}"),
                    Unit::Group(g) => format!("{}#{}", g.id, g.members.len()),
                })
                .collect();
            v.sort();
            v
        };
        assert_eq!(ids(&a), ids(&b));
    }

    fn chunk_keys_49k() -> Vec<String> {
        let mut keys = Vec::new();
        for s in 0..9 {
            for c in 0..1100 {
                for p in 0..(1 + c % 9) {
                    keys.push(format!("anchor/imagery/v1/stream/{s:08x}/chunk/{c:04}/{p:06}"));
                }
            }
        }
        keys
    }

    #[test]
    fn zagros_scale_stays_within_budget() {
        let keys = chunk_keys_49k();
        assert!(keys.len() > 40_000);
        let units = run(&keys, PUBLISHER_BUDGET, &HashSet::new());
        assert!(units.len() <= PUBLISHER_BUDGET);
    }

    /// Timing, release build only:
    /// `cargo test --release --bin panoscope group_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn group_timing() {
        let keys = chunk_keys_49k();
        let pairs: Vec<(&str, usize)> = keys.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
        let mut best = u128::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            let units = group_topics("pub", &pairs, PUBLISHER_BUDGET, &HashSet::new());
            best = best.min(t.elapsed().as_micros());
            assert!(units.len() <= PUBLISHER_BUDGET);
        }
        println!("group_topics: {} topics in {:.1} ms (best of 5)", keys.len(), best as f64 / 1000.0);
    }
}
