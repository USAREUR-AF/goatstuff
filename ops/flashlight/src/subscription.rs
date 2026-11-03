// SPDX-License-Identifier: Apache-2.0
//
// Translate a parsed `Filter` into a `SubscriptionPlan` — the
// wire-level Zenoh subscriptions PLUS the post-filter predicates
// the sample loop applies after a sample arrives.
//
// Why split:
//   - `key`/`vendor` map to Zenoh key expressions and can be narrowed
//     at the WIRE level — the router only forwards samples that match
//     the subscription, saving bandwidth + CPU.
//   - `role`/`site`/`format` depend on the peer registry (peer
//     metadata declared via schema-decl) — they can't be expressed as
//     a key-expression pattern, so we apply them in the sample loop
//     against the topology built up from declarations + audit replay.
//
// Compose with `panoscope_core::Topology` (peer registry) on the
// apply side. PR #3 of the Phase C series wires the sample loop +
// Topology lookup; this module is the pure-Rust translation layer.

use std::collections::HashSet;

use serde::Serialize;

use crate::filter::Filter;

/// Subscription instructions: which key-expressions to register on
/// the Zenoh session, and which sample-time predicates to apply.
#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct SubscriptionPlan {
    /// Wire-level Zenoh key expressions. Empty means "no subscription"
    /// (caller should render the help page rather than subscribing to
    /// everything — per design doc).
    pub key_exprs: HashSet<String>,
    /// Vendor names matched anchor-aware against each sample key
    ///: a vendor `v` matches `v/**` (per-vendor goatnets) AND
    /// `<slot-anchor>/v/**` (slot-pool goatnets, ADR 1025). Not
    /// expressible as a Zenoh key expression without over-matching
    /// (`*/v/**` would catch non-anchor first segments like
    /// `ops/v/...`), so it's a key-shape predicate OR-ed with
    /// `key_exprs`, evaluated via `matches_key`.
    pub vendors: HashSet<String>,
    /// Post-filter predicates applied to each sample after it arrives.
    pub post: PostFilters,
}

#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct PostFilters {
    /// If non-empty, sample's publisher must have `role_class` in this set.
    pub roles: HashSet<String>,
    /// If non-empty, sample's publisher must have `site` in this set.
    pub sites: HashSet<String>,
    /// If non-empty, sample's publisher must have `format_hint` matching
    /// one of these formats (loose match — "protobuf" matches
    /// `format_hint = "protobuf:DenObservation"` too).
    pub formats: HashSet<String>,
}

impl SubscriptionPlan {
    /// True when the plan would subscribe to nothing. Treated as the
    /// empty-filter case at the apply-side — render the help page,
    /// never subscribe to `**`.
    ///
    /// PR #3 of Phase C wires the actual help-page short-circuit
    /// that calls this; PR #2 just exposes it for tests.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.key_exprs.is_empty() && self.vendors.is_empty()
    }

    /// Anchor-aware vendor match for a concrete sample key: a
    /// vendor `v` claims both `v/**` (per-vendor goatnets) and
    /// `<slot-anchor>/v/**` (slot-pool goatnets). OR-ed with the
    /// `key_exprs` match by the sample loop — `key=` and `vendor=`
    /// keep their wire-union semantics from the URL grammar;
    /// role/site/format post-filters AND on top.
    pub fn matches_vendor(&self, concrete_key: &str) -> bool {
        self.vendors
            .iter()
            .any(|v| panoscope_core::key_vendor_matches(concrete_key, v))
    }

    /// True when the plan has post-filter predicates the sample loop
    /// must evaluate. When false, every sample matching one of the
    /// `key_exprs` passes through without further checks.
    pub fn has_post_filters(&self) -> bool {
        !self.post.roles.is_empty()
            || !self.post.sites.is_empty()
            || !self.post.formats.is_empty()
    }
}

/// Translate a `Filter` into a `SubscriptionPlan`.
///
/// Rules:
///   - `?key=<expr>` adds `<expr>` verbatim to `key_exprs` (Zenoh's
///     own wildcard matcher handles `*` and `**`).
///   - `?vendor=<v>` lands in `vendors`, matched anchor-aware at
///     sample time (`v/**` AND `<slot-anchor>/v/**` — F-199). It is
///     deliberately NOT a `v/**` key expression: that goes silently
///     blind on slot-pool goatnets where wire keys lead with the
///     UUID anchor, and the `*/v/**` widening would over-match
///     non-anchor first segments (`ops/v/...`).
///   - `?role` / `?site` / `?format` populate `PostFilters`.
///   - Repeated params accumulate via the underlying `HashSet`
///     (OR-within semantics from the URL grammar).
///   - When no `key` and no `vendor` were declared, but ONLY post-
///     filters are present (e.g. `?role=sensor`), `key_exprs`
///     becomes `{"**"}` — we have to listen broadly and then filter
///     at sample time, since `role`/`site`/`format` are peer-side
///     facts not expressible as a key pattern. The design doc allows
///     this: "Server-side resolution via panoscope-core's peer
///     registry."
///   - An empty `Filter` (no key, no vendor, no post-filters)
///     produces an empty plan — the apply-side renders the help page.
pub fn plan_subscription(filter: &Filter) -> SubscriptionPlan {
    let mut plan = SubscriptionPlan::default();

    for k in &filter.keys {
        plan.key_exprs.insert(k.clone());
    }
    plan.vendors = filter.vendors.clone();

    plan.post.roles = filter.roles.clone();
    plan.post.sites = filter.sites.clone();
    plan.post.formats = filter.formats.clone();

    // If the filter has only post-filter dimensions (e.g. ?role=sensor
    // alone), we have nothing to narrow at the wire — subscribe broadly
    // and gate at sample time. Filter::is_empty checks all eight
    // dimensions; this branch fires when at least one of role/site/
    // format is set but no key/vendor narrow the wire. A non-empty
    // `vendors` set must NOT trigger this: the vendor predicate is the
    // narrowing, and a `**` key expression would bypass it.
    if plan.key_exprs.is_empty() && plan.vendors.is_empty() && plan.has_post_filters() {
        plan.key_exprs.insert("**".to_string());
    }

    plan
}

/// Apply the `role` / `site` / `format` post-filters to a single
/// event, using the hydrated peer topology to attribute the event's
/// key to a publisher and check that publisher's declared metadata.
///
/// Semantics:
///   - No active post-filter dimension → always passes (the wire
///     key-expression match already did the work).
///   - Otherwise the event passes iff SOME peer that declares a publish
///     pattern matching `event_key` satisfies EVERY active dimension
///     (role_class ∈ roles AND site ∈ sites AND format_hint ~ formats).
///   - An event whose key matches no declared publisher is DROPPED when
///     a post-filter is active — flashlight can't confirm it satisfies
///     the filter, so it must not leak through. This is the documented
///     v1 limitation (a peer is invisible to `?role=` until its first
///     schema-decl); see docs/design/flashlight.md open questions. It
///     is the correct behavior vs. the prior silent widen-to-`**`.
pub fn post_filters_pass(
    topology: &panoscope_core::topology::Topology,
    event_key: &str,
    post: &PostFilters,
) -> bool {
    if post.roles.is_empty() && post.sites.is_empty() && post.formats.is_empty() {
        return true;
    }
    for peer in topology.peers.values() {
        if !crate::keymatch::key_matches_any(event_key, &peer.publishes) {
            continue;
        }
        if !post.roles.is_empty()
            && !peer
                .role_class
                .as_ref()
                .is_some_and(|rc| post.roles.contains(rc))
        {
            continue;
        }
        if !post.sites.is_empty() && !peer.site.as_ref().is_some_and(|s| post.sites.contains(s)) {
            continue;
        }
        if !post.formats.is_empty() {
            let fmt_ok = peer
                .format_hint
                .as_ref()
                .is_some_and(|h| post.formats.iter().any(|w| format_matches(h, w)));
            if !fmt_ok {
                continue;
            }
        }
        return true;
    }
    false
}

/// Cheap loose-match for `format_hint` strings. Topic registrations
/// often carry `format = "protobuf:DenObservation"` (the message name
/// after the colon), but operators filter by family (`?format=protobuf`).
/// Match the family prefix; fall through to exact equality for non-
/// protobuf formats (`json`, `cbor`, `text`, etc.).
pub fn format_matches(hint: &str, wanted: &str) -> bool {
    if hint == wanted {
        return true;
    }
    // `protobuf:X` matches `protobuf` (family-level filter).
    if let Some(family) = hint.split(':').next() {
        if family == wanted {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn s(x: &str) -> String {
        x.to_string()
    }

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn empty_filter_yields_empty_plan() {
        let f = Filter::default();
        let p = plan_subscription(&f);
        assert!(p.is_empty());
        assert!(!p.has_post_filters());
    }

    #[test]
    fn single_key_verbatim() {
        let f = Filter {
            keys: set(&["vendor-a/observations/v1"]),
            ..Filter::default()
        };
        let p = plan_subscription(&f);
        assert_eq!(p.key_exprs, set(&["vendor-a/observations/v1"]));
        assert!(!p.has_post_filters());
    }

    #[test]
    fn wildcard_key_preserved() {
        let f = Filter {
            keys: set(&["vendor-a/**"]),
            ..Filter::default()
        };
        let p = plan_subscription(&f);
        assert_eq!(p.key_exprs, set(&["vendor-a/**"]));
    }

    #[test]
    fn vendor_becomes_anchor_aware_predicate() {
        let f = Filter {
            vendors: set(&["vendor-b"]),
            ..Filter::default()
        };
        let p = plan_subscription(&f);
        assert!(p.key_exprs.is_empty());
        assert_eq!(p.vendors, set(&["vendor-b"]));
        assert!(!p.is_empty());
        // Per-vendor key shape.
        assert!(p.matches_vendor("vendor-b/observations/v1"));
        // Slot-pool key shape — the F-199 blind spot.
        assert!(p.matches_vendor(
            "483a44bc6ab522416e639fbe726f79b4/vendor-b/observations/v1"
        ));
        // Vendor name behind a NON-anchor first segment must not match.
        assert!(!p.matches_vendor("ops/vendor-b/health"));
        assert!(!p.matches_vendor("vendor-a/observations/v1"));
    }

    #[test]
    fn multiple_vendors_or_within() {
        let f = Filter {
            vendors: set(&["vendor-b", "vendor-a"]),
            ..Filter::default()
        };
        let p = plan_subscription(&f);
        assert_eq!(p.vendors, set(&["vendor-b", "vendor-a"]));
        assert!(p.matches_vendor("vendor-b/x"));
        assert!(p.matches_vendor("vendor-a/x"));
    }

    #[test]
    fn key_and_vendor_unioned() {
        // ?key=foo/bar&vendor=vendor-b → key pattern on the wire set,
        // vendor as the anchor-aware predicate; the sample loop ORs them.
        let f = Filter {
            keys: set(&["foo/bar"]),
            vendors: set(&["vendor-b"]),
            ..Filter::default()
        };
        let p = plan_subscription(&f);
        assert_eq!(p.key_exprs, set(&["foo/bar"]));
        assert_eq!(p.vendors, set(&["vendor-b"]));
    }

    #[test]
    fn role_only_subscribes_broadly_with_post_filter() {
        let f = Filter {
            roles: set(&["sensor"]),
            ..Filter::default()
        };
        let p = plan_subscription(&f);
        // Wire subscription must be broad because role lives in the
        // peer registry, not the key path.
        assert_eq!(p.key_exprs, set(&["**"]));
        assert_eq!(p.post.roles, set(&["sensor"]));
        assert!(p.has_post_filters());
    }

    #[test]
    fn site_only_subscribes_broadly() {
        let f = Filter {
            sites: set(&["alpha-fob"]),
            ..Filter::default()
        };
        let p = plan_subscription(&f);
        assert_eq!(p.key_exprs, set(&["**"]));
        assert_eq!(p.post.sites, set(&["alpha-fob"]));
    }

    #[test]
    fn format_only_subscribes_broadly() {
        let f = Filter {
            formats: set(&["protobuf"]),
            ..Filter::default()
        };
        let p = plan_subscription(&f);
        assert_eq!(p.key_exprs, set(&["**"]));
        assert_eq!(p.post.formats, set(&["protobuf"]));
    }

    #[test]
    fn vendor_and_role_combines_narrow_with_post() {
        // ?vendor=vendor-b&role=sensor → vendor predicate narrows the
        // slice, then apply role-class post-filter.
        let f = Filter {
            vendors: set(&["vendor-b"]),
            roles: set(&["sensor"]),
            ..Filter::default()
        };
        let p = plan_subscription(&f);
        assert_eq!(p.vendors, set(&["vendor-b"]));
        assert_eq!(p.post.roles, set(&["sensor"]));
        // Crucially NOT falling through to a `**` key expression — that
        // would bypass the vendor predicate at the sample loop.
        assert!(p.key_exprs.is_empty());
    }

    #[test]
    fn format_matches_family() {
        // Operator passes `format=protobuf`; registered topic carries
        // `format = "protobuf:DenObservation"`. Family-level match
        // should succeed.
        assert!(format_matches("protobuf:DenObservation", "protobuf"));
    }

    #[test]
    fn format_matches_exact() {
        assert!(format_matches("json", "json"));
        assert!(format_matches("cbor", "cbor"));
    }

    #[test]
    fn format_does_not_match_unrelated() {
        assert!(!format_matches("protobuf:Foo", "json"));
        assert!(!format_matches("cbor", "protobuf"));
    }

    #[test]
    fn format_handles_no_colon_protobuf() {
        // Some topic declarations carry bare "protobuf" without the
        // `:MessageName` suffix; still match a family filter.
        assert!(format_matches("protobuf", "protobuf"));
    }

    #[test]
    fn plan_is_empty_distinguishes_help_page_case() {
        // The apply-side checks `plan.is_empty()` to decide between
        // help-page and live-tail. An empty Filter must yield an
        // empty plan, NOT a `{"**"}` plan.
        let empty_filter = Filter::default();
        let plan = plan_subscription(&empty_filter);
        assert!(plan.is_empty());
        assert!(plan.key_exprs.is_empty());
    }

    #[test]
    fn realistic_role_and_site_combination() {
        // ?role=sensor&site=alpha-fob → broad wire + both post-filters.
        let f = Filter {
            roles: set(&["sensor"]),
            sites: set(&["alpha-fob"]),
            ..Filter::default()
        };
        let p = plan_subscription(&f);
        assert_eq!(p.key_exprs, set(&["**"]));
        assert_eq!(p.post.roles, set(&["sensor"]));
        assert_eq!(p.post.sites, set(&["alpha-fob"]));
    }

    #[test]
    fn key_with_post_filter_keeps_narrow_wire() {
        // Even with a post-filter dimension present, an explicit key
        // narrows the wire — don't fall through to `**`.
        let f = Filter {
            keys: set(&["notional/tarsier/alpha-fob/track/v1"]),
            roles: set(&["sensor"]),
            ..Filter::default()
        };
        let p = plan_subscription(&f);
        assert_eq!(
            p.key_exprs,
            set(&["notional/tarsier/alpha-fob/track/v1"])
        );
        assert_eq!(p.post.roles, set(&["sensor"]));
    }

    #[test]
    fn _unused_helper_silenced() {
        let _ = s("");
    }
}
