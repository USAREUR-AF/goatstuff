// SPDX-License-Identifier: Apache-2.0
//
// Key-expression matching — does a concrete Zenoh key (e.g. the
// `sample.key_expr()` from a live sample) match any of a set of
// pattern key-expressions (the wire-level subscription set from a
// `SubscriptionPlan`)?
//
// We use `zenoh::key_expr::KeyExpr::intersects` for canonical
// matching — Zenoh's matcher implements the same `*` / `**`
// semantics the router uses on the wire, so an in-process filter
// can't drift from the router's view. The wrapper here just maps
// strings → KeyExpr handles + checks if any pattern intersects the
// concrete key.
//
// Why match in-process at all if the wire is already narrowed?
//
// PR #2 wired a BROAD `**` subscription so the broadcast channel
// carries everything. Each SSE client (PR #3) re-narrows in-process
// per request — that decouples the wire subscription from the
// per-client filter and lets N concurrent slices share one Zenoh
// session. A later PR can switch to per-request narrow wire
// subscriptions if N grows large enough to justify the router
// chatter.

use std::collections::HashSet;

use zenoh::key_expr::KeyExpr;

/// True when `concrete_key` matches AT LEAST ONE pattern in
/// `patterns`. Empty pattern set → false (the caller should have
/// rendered the help page already).
///
/// Patterns are parsed lazily; an invalid pattern is skipped with a
/// trace-level log line rather than failing the whole filter (the
/// filter parser validates the URL grammar, not the resulting
/// key-expression — a future change could move that validation up).
pub fn key_matches_any(concrete_key: &str, patterns: &HashSet<String>) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let Ok(key) = KeyExpr::new(concrete_key) else {
        // The concrete key arrived from an actual Zenoh sample so it
        // really should parse; treat parse failure as "no match" rather
        // than panicking in the SSE hot path.
        return false;
    };
    patterns.iter().any(|p| {
        match KeyExpr::new(p.as_str()) {
            Ok(pat) => pat.intersects(&key),
            Err(_) => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn empty_patterns_never_match() {
        assert!(!key_matches_any("anything", &HashSet::new()));
    }

    #[test]
    fn literal_match() {
        let pats = set(&["vendor-a/observations/v1"]);
        assert!(key_matches_any("vendor-a/observations/v1", &pats));
        assert!(!key_matches_any("vendor-a/observations/v2", &pats));
    }

    #[test]
    fn double_wildcard_matches_descendants() {
        let pats = set(&["vendor-a/**"]);
        assert!(key_matches_any("vendor-a/observations/v1", &pats));
        assert!(key_matches_any("vendor-a/control/v1", &pats));
        assert!(key_matches_any("vendor-a", &pats));
        assert!(!key_matches_any("vendor-b/observations/v1", &pats));
    }

    #[test]
    fn single_wildcard_matches_one_segment() {
        let pats = set(&["vendor-a/*/v1"]);
        assert!(key_matches_any("vendor-a/observations/v1", &pats));
        assert!(key_matches_any("vendor-a/control/v1", &pats));
        // Two segments — Zenoh single * is one segment, so this
        // should NOT match.
        assert!(!key_matches_any("vendor-a/observations/raw/v1", &pats));
    }

    #[test]
    fn union_of_patterns_any_match() {
        let pats = set(&["vendor-b/**", "vendor-a/**"]);
        assert!(key_matches_any("vendor-b/sensorgrid/raw", &pats));
        assert!(key_matches_any("vendor-a/observations/v1", &pats));
        assert!(!key_matches_any("vendor-c/foo", &pats));
    }

    #[test]
    fn invalid_pattern_silently_skipped() {
        // Mix a definitely-invalid pattern with a valid one; the
        // valid one still catches matches.
        let pats = set(&["", "vendor-a/**"]);
        assert!(key_matches_any("vendor-a/x", &pats));
    }

    #[test]
    fn broad_subscription_matches_everything() {
        let pats = set(&["**"]);
        assert!(key_matches_any("anything", &pats));
        assert!(key_matches_any("a/b/c/d/e", &pats));
        assert!(key_matches_any("foo", &pats));
    }

    #[test]
    fn realistic_notional_vendor_filter() {
        // Operator URL: ?vendor=notional → plan key_exprs = {"notional/**"}
        // Concrete samples carry the per-peer + per-site segments.
        let pats = set(&["notional/**"]);
        assert!(key_matches_any(
            "notional/tarsier/alpha-fob/track/v1",
            &pats
        ));
        assert!(key_matches_any(
            "notional/mongoose/bravo-fob/effector/v1",
            &pats
        ));
        // Real audit-subscriber tail also carries non-notional vendors;
        // the per-client filter rejects those.
        assert!(!key_matches_any("demogame/nova-a/event/v1", &pats));
    }
}
