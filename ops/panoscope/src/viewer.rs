// SPDX-License-Identifier: Apache-2.0
//
// Per-viewer RBAC scoping (ADR 1073, docs/design/rbac-adherence.md §3.3).
//
// The oauth2-proxy-panoscope sidecar authenticates the browser and injects
// `X-Auth-Request-Groups` / `X-Auth-Request-Email`; this module turns those
// headers into a ViewerScope and filters everything panoscope serves to it.
//
// Modes (PANOSCOPE_RBAC_MODE):
//   off     — every request is Full scope; byte-identical to pre-RBAC
//             behavior. The shipping default: nothing changes until an
//             operator deploys the sidecar and flips the flag.
//   enforce — scope derives from the groups header, FAIL-CLOSED: a request
//             with no/unknown groups gets the minimal Observer scope. The
//             mesh-only Traefik middleware is still in front (defense in
//             depth) but is a network boundary, not identity.
//
// Scopes evaluate the ADR 1074 data-access grammar carried in the same
// claim as the platform groups (SoT: ops/permissions/data-access-roles.yaml):
//   data.view.all             → DataAll (everything but platform-audit)
//   data.view.org:<prefix> /
//   data.view.producer:<a>    → Scoped{anchors}
//   data.view.notional        → include_notional (default-granted per
//                               goatnet flag; signal = the producer's
//                               schema-decl `synthetic: true`)
//   platform group ADMINS       → Full via the SoT's declared cross-axis
//                               implication (G2/G4; IAM-ADMIN deliberately
//                               NOT implied — matrix grants it `scoped`)
// No qualifying value → the floor (infra + ops health [+ notional when
// the default is on]).
//
// This is DISPLAY/API-layer authorization, not wire confidentiality: any
// goatnet cert holder can still open their own Zenoh session (ADR 1025's
// read posture). What this buys is least-privilege presentation and real
// enforcement of the declared matrix on the HTTP path.
//
// Deliberate exceptions (aggregate-only, no topic/peer identities):
// `/api/stats` (global sample counters — feeds the HUD every tier keeps),
// `/healthz`, `/version`, `/api/legend` (display config). Everything that
// names a topic, peer, or payload is scope-gated.

use std::collections::HashSet;

use axum::http::HeaderMap;

use crate::snapshot_view::{EdgeView, NodeView};
use crate::zenoh_client::WsEvent;
use panoscope_core::NodeClass;

// The groups claim is read from `X-Forwarded-Groups` ONLY, and that choice is
// load-bearing for security. Our sidecar is oauth2-proxy as a REVERSE PROXY
// (PASS_USER_HEADERS=true): it strips any client-supplied `X-Forwarded-*` and
// injects its own from the validated session, so `X-Forwarded-Groups` cannot be
// forged by the browser. We deliberately do NOT read `X-Auth-Request-Groups`:
// in reverse-proxy mode oauth2-proxy only SETS the `X-Auth-Request-*` family on
// the `/oauth2/auth` subrequest RESPONSE (for nginx auth_request), and does NOT
// strip a client-supplied `X-Auth-Request-Groups` from the upstream request — so
// trusting it let a logged-in FLOOR viewer forge `X-Auth-Request-Groups: ADMINS`
// and escalate to Full. Both bugs caught in the ZAGROS live-fire: reading
// only `x-auth-request-groups` (pre-M-079) floored every real login because the
// sidecar never sets it upstream; reading it AT ALL is the forgery hole.
pub const GROUPS_HEADER: &str = "x-forwarded-groups";

/// The trusted login-email header, set by the same oauth2-proxy sidecar
/// that sets `GROUPS_HEADER` (paired, non-forgeable — the sidecar strips
/// any client-supplied copy). Used only to key the ADR 1074 B1 data-roles
/// resolver lookup; never itself a grant.
pub const EMAIL_HEADER: &str = "x-forwarded-email";

/// The trusted viewer login email for this request, if the sidecar set it.
/// `None` when unauthenticated or the header is absent — the resolver
/// enrichment then no-ops (base scope stands).
pub fn viewer_email(headers: &HeaderMap) -> Option<String> {
    headers
        .get(EMAIL_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
}

/// Namespace prefixes every authenticated viewer may see: environment
/// health + control surfaces (`ops/`, minus the operator-audit carve-out
/// below) and schema self-declarations (`attest/`). Declared in the SoT
/// (`boundaries.common_floor`); CI asserts this constant matches it.
const COMMON_PREFIXES: [&str; 2] = ["ops/", "attest/"];

/// Restricted even for DataAll: the operator-action audit namespace
/// (already deny-carved on the wire ACL per M-068; kept here as
/// defense-in-depth so a replayed/cached sample can't surface it).
/// Declared in the SoT (`boundaries.platform_only`); CI-checked.
const AUDIT_PREFIX: &str = "ops/audit/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RbacMode {
    Off,
    Enforce,
}

#[derive(Debug, Clone)]
pub struct RbacConfig {
    pub mode: RbacMode,
    /// Platform groups whose SoT-declared cross-axis implication is
    /// `data.view.all` at Full strength (ADR 1074 implications: ADMINS
    /// only — the second pass narrowed IAM-ADMIN to `scoped`; an
    /// identity administrator gets the access COP, not standing
    /// all-mission-data view). CI ties this to the matrix + SoT.
    pub full_groups: HashSet<String>,
    /// Default-grant `data.view.notional` to every authenticated viewer
    /// (the SoT's `default: true` implication; per-goatnet flag).
    pub notional_default: bool,
    /// Platform groups admitted to the identity roster (`/who` — the
    /// identity COP). This is the platform-administration axis, not the
    /// data-access grammar: the roster names identities, grants, and
    /// reconcile state — id-console-class information. Default =
    /// `ADMINS,IAM-ADMIN` (the matrix's identity-administration
    /// grantors); overridden per-goatnet via env, same pattern as
    /// `full_groups`.
    pub who_groups: HashSet<String>,
    /// ADR 1074 B1: enrich a Scoped participant's scope with the
    /// registry-derived data-roles resolved live from the portal
    /// (`GET /api/access/data-roles`), so a data-plane participant sees
    /// their org/producer namespaces without those grants having to ride
    /// the Ory groups claim. Default OFF (claim-only scoping, pre-B1
    /// behavior); per-goatnet opt-in once the portal lookup token is
    /// wired. Fail-closed: a resolver miss/error leaves the base
    /// (claim-derived) scope untouched — never widens on failure.
    pub participant_resolver: bool,
}

impl Default for RbacConfig {
    fn default() -> Self {
        Self {
            mode: RbacMode::Off,
            full_groups: ["ADMINS"].iter().map(|s| s.to_string()).collect(),
            notional_default: true,
            who_groups: ["ADMINS", "IAM-ADMIN"].iter().map(|s| s.to_string()).collect(),
            participant_resolver: false,
        }
    }
}

impl RbacConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let mut c = Self::default();
        if let Ok(v) = std::env::var("PANOSCOPE_RBAC_MODE") {
            c.mode = match v.to_ascii_lowercase().as_str() {
                "" | "off" => RbacMode::Off,
                "enforce" => RbacMode::Enforce,
                other => anyhow::bail!("PANOSCOPE_RBAC_MODE must be off|enforce, got {other:?}"),
            };
        }
        if let Ok(v) = std::env::var("PANOSCOPE_RBAC_FULL_GROUPS") {
            c.full_groups = split_groups(&v).into_iter().collect();
        }
        if let Ok(v) = std::env::var("PANOSCOPE_RBAC_NOTIONAL_DEFAULT") {
            c.notional_default =
                matches!(v.to_ascii_lowercase().as_str(), "" | "true" | "1" | "yes");
        }
        if let Ok(v) = std::env::var("PANOSCOPE_RBAC_WHO_GROUPS") {
            c.who_groups = split_groups(&v).into_iter().collect();
        }
        if let Ok(v) = std::env::var("PANOSCOPE_PARTICIPANT_RESOLVER") {
            c.participant_resolver =
                matches!(v.to_ascii_lowercase().as_str(), "on" | "true" | "1" | "yes");
        }
        Ok(c)
    }

    /// May the request behind `headers` read the identity roster (`/who`)?
    /// Mode off ⇒ yes (pre-RBAC behavior, same rule as every other
    /// endpoint). Enforce ⇒ FAIL-CLOSED group-membership check against
    /// `who_groups` — independent of the data-access lattice, because the
    /// roster is identity-administration information, not mission data:
    /// a `data.view.all` grant deliberately does NOT admit it, and a
    /// re-widened `full_groups` does not silently widen it either.
    pub fn allows_who(&self, headers: &HeaderMap) -> bool {
        if self.mode == RbacMode::Off {
            return true;
        }
        headers
            .get_all(GROUPS_HEADER)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(split_groups)
            .any(|g| self.who_groups.contains(&g))
    }
}

/// The ADR 1074 data-access grammar values this surface evaluates.
/// Kept as constants so the parser and the SoT can be diffed by eye;
/// CI (check-data-access-roles.sh) asserts they parse against the SoT.
pub const ROLE_VIEW_ALL: &str = "data.view.all";
pub const ROLE_VIEW_NOTIONAL: &str = "data.view.notional";
pub const ROLE_VIEW_ORG_PREFIX: &str = "data.view.org:";
pub const ROLE_VIEW_PRODUCER_PREFIX: &str = "data.view.producer:";

/// What one authenticated viewer is allowed to see (the ADR 1074 v1
/// lattice as evaluated by this surface).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewerScope {
    /// Platform tier (ADMINS implication): everything, including
    /// platform telemetry. Also every request when RBAC is off.
    Full,
    /// Holder of `data.view.all`: every producer namespace, real and
    /// notional — but not platform-audit telemetry (that is the platform
    /// axis, not mission data; G2 orthogonality).
    DataAll,
    /// Explicit org/producer grants + optionally the notional fleet +
    /// the common floor. `anchors` empty + `include_notional` false is
    /// the bare fail-closed floor.
    Scoped {
        anchors: HashSet<String>,
        include_notional: bool,
    },
}

impl ViewerScope {
    /// Derive the scope for one request. The claim carries BOTH axes:
    /// UPPERCASE platform groups (Full via the SoT's declared implication)
    /// and lowercase `data.*` grammar values (comma/pipe/space separated;
    /// oauth2-proxy emits comma-joined — be liberal). Fail-closed: enforce
    /// mode with a missing/unmatched header yields the bare floor
    /// (+ notional when the goatnet default is on), never Full.
    pub fn from_headers(headers: &HeaderMap, cfg: &RbacConfig) -> Self {
        if cfg.mode == RbacMode::Off {
            return ViewerScope::Full;
        }
        let groups: Vec<String> = headers
            .get_all(GROUPS_HEADER)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(split_groups)
            .collect();
        // Cross-axis implication (SoT: ADMINS ⇒ data.view.all at Full
        // strength — platform operators also see platform telemetry).
        if groups.iter().any(|g| cfg.full_groups.contains(g)) {
            return ViewerScope::Full;
        }
        // Explicit apex grant: mission-data-everything, not platform audit.
        if groups.iter().any(|g| g == ROLE_VIEW_ALL) {
            return ViewerScope::DataAll;
        }
        // Parametric grants: org / producer values carry the registry key.
        let anchors: HashSet<String> = groups
            .iter()
            .filter_map(|g| {
                g.strip_prefix(ROLE_VIEW_ORG_PREFIX)
                    .or_else(|| g.strip_prefix(ROLE_VIEW_PRODUCER_PREFIX))
            })
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .collect();
        let include_notional =
            cfg.notional_default || groups.iter().any(|g| g == ROLE_VIEW_NOTIONAL);
        ViewerScope::Scoped { anchors, include_notional }
    }

    /// ADR 1074 B1: fold registry-derived data-roles (from the portal
    /// `/api/access/data-roles` resolver) into this claim-derived scope.
    /// The resolver returns the SAME `data.view.*` grammar the claim
    /// carries, so this is a union, not a new axis:
    ///
    ///   - `Full` / `DataAll` are already maximal (or platform-tier) — a
    ///     participant resolver never downgrades them; returned as-is.
    ///   - `data.view.all` in the resolver set upgrades a `Scoped` viewer
    ///     to `DataAll` (all mission data, still not platform audit).
    ///   - `data.view.org:<x>` / `data.view.producer:<x>` add their anchor.
    ///   - `data.view.notional` turns on the notional class.
    ///
    /// Fail-closed by construction: an empty role set (resolver miss or
    /// error, handled by the caller) returns the base scope unchanged —
    /// enrichment only ever widens, never narrows.
    #[must_use]
    pub fn enriched_with(self, roles: &[String]) -> Self {
        match self {
            ViewerScope::Full | ViewerScope::DataAll => self,
            ViewerScope::Scoped { mut anchors, mut include_notional } => {
                if roles.iter().any(|r| r == ROLE_VIEW_ALL) {
                    return ViewerScope::DataAll;
                }
                for r in roles {
                    if let Some(v) = r
                        .strip_prefix(ROLE_VIEW_ORG_PREFIX)
                        .or_else(|| r.strip_prefix(ROLE_VIEW_PRODUCER_PREFIX))
                    {
                        if !v.is_empty() {
                            anchors.insert(v.to_string());
                        }
                    } else if r == ROLE_VIEW_NOTIONAL {
                        include_notional = true;
                    }
                }
                ViewerScope::Scoped { anchors, include_notional }
            }
        }
    }

    /// May this viewer see samples/topics under `key`? Callers that can
    /// resolve the key's publisher should ALSO consult
    /// [`ViewerScope::allows_synthetic_key`] for the notional class —
    /// key text alone cannot prove a producer is synthetic.
    pub fn allows_key(&self, key: &str) -> bool {
        match self {
            ViewerScope::Full => true,
            ViewerScope::DataAll => !key.starts_with(AUDIT_PREFIX),
            ViewerScope::Scoped { anchors, .. } => {
                key_in_common(key) || key_in_orgs(key, anchors)
            }
        }
    }

    /// Second chance for a key denied by [`allows_key`]: allowed when the
    /// viewer holds the notional grant AND the caller has resolved the
    /// key's publisher to a `synthetic: true` self-declaration.
    pub fn allows_synthetic_key(&self, publisher_is_synthetic: bool) -> bool {
        publisher_is_synthetic
            && matches!(self, ViewerScope::Scoped { include_notional: true, .. })
    }

    /// May this viewer see this node at all? Infrastructure is visible to
    /// every authenticated scope (substrate, not mission data); notional
    /// producers (schema-decl `synthetic: true`) are visible under the
    /// notional grant; granted app participants match by pinned-anchor
    /// peer_id or by publishing under a granted anchor — subscribes
    /// deliberately do NOT qualify (would leak foreign consumers).
    pub fn allows_node(&self, node: &NodeView) -> bool {
        match self {
            ViewerScope::Full | ViewerScope::DataAll => true,
            ViewerScope::Scoped { anchors, include_notional } => {
                node.node_class == NodeClass::Infra
                    || (*include_notional && node.synthetic == Some(true))
                    || anchors.contains(&node.peer_id)
                    || node.publishes.iter().any(|k| key_in_orgs(k, anchors))
            }
        }
    }

    /// `allows_node` over a peer's raw facts rather than its display view:
    /// `publish_roots` holds the leading segment of every topic the peer
    /// publishes (plus the anchor of any `volatile/<anchor>/…` key), so the
    /// anchor check answers from the real publish set, not the folded
    /// `NodeView.publishes` strings. Equivalent to `allows_node` for anchors
    /// without `/` (every anchor the grant grammar produces).
    pub fn allows_peer_facts(
        &self,
        peer_id: &str,
        node_class: NodeClass,
        synthetic: Option<bool>,
        publish_roots: &HashSet<String>,
    ) -> bool {
        match self {
            ViewerScope::Full | ViewerScope::DataAll => true,
            ViewerScope::Scoped { anchors, include_notional } => {
                node_class == NodeClass::Infra
                    || (*include_notional && synthetic == Some(true))
                    || anchors.contains(peer_id)
                    || publish_roots.iter().any(|r| anchors.contains(r))
            }
        }
    }

    /// May this viewer see this topic, given whether its publisher is a
    /// self-declared synthetic producer? `allows_key` plus the notional
    /// second chance, in one place.
    pub fn allows_topic(&self, key: &str, publisher_synthetic: bool) -> bool {
        self.allows_key(key) || self.allows_synthetic_key(publisher_synthetic)
    }

    /// A stable string for this scope, for cache keys and ETags.
    pub fn cache_key(&self) -> String {
        match self {
            ViewerScope::Full => "full".into(),
            ViewerScope::DataAll => "data-all".into(),
            ViewerScope::Scoped { anchors, include_notional } => {
                let mut a: Vec<&String> = anchors.iter().collect();
                a.sort();
                format!("scoped:{}:{}", include_notional, a.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(","))
            }
        }
    }

    /// May this viewer probe/inspect the given publisher peer id
    /// (`/api/publisher/hygiene`)? Scoped: only granted anchors (notional
    /// probing rides the caller's synthetic resolution via
    /// [`allows_synthetic_key`]).
    pub fn allows_peer_inspect(&self, peer_id: &str) -> bool {
        match self {
            ViewerScope::Full | ViewerScope::DataAll => true,
            ViewerScope::Scoped { anchors, .. } => anchors.contains(peer_id),
        }
    }

    /// Sampling-drop counters this viewer may see. The map is keyed by topic,
    /// so it names topics: filtered by `allows_key` like any other key, never
    /// sent whole. Fail-closed:
    /// the notional second chance needs a publisher this map does not carry.
    pub fn filter_sampling_drops(
        &self,
        drops: &std::collections::HashMap<String, u64>,
    ) -> std::collections::HashMap<String, u64> {
        drops
            .iter()
            .filter(|(k, _)| self.allows_key(k))
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }

    /// Filter a built snapshot to this scope. Keeps a node when
    /// `allows_node`; keeps an edge when its topic key is allowed — or its
    /// source is a kept `synthetic: true` producer under the notional
    /// grant (without this, notional mode showed synthetic nodes with no
    /// topics/edges — a half-granted default) — AND both endpoints
    /// survived (fail-closed: a hidden endpoint hides the edge).
    pub fn filter_snapshot(
        &self,
        nodes: Vec<NodeView>,
        edges: Vec<EdgeView>,
    ) -> (Vec<NodeView>, Vec<EdgeView>) {
        if matches!(self, ViewerScope::Full) {
            return (nodes, edges);
        }
        let nodes: Vec<NodeView> = nodes.into_iter().filter(|n| self.allows_node(n)).collect();
        let kept: HashSet<&str> = nodes.iter().map(|n| n.peer_id.as_str()).collect();
        // Kept synthetic producers, whose edges ride the notional grant
        // (the allows_synthetic_key second chance, resolved here from the
        // node's own schema-decl flag).
        let synth: HashSet<&str> = if matches!(self, ViewerScope::Scoped { include_notional: true, .. }) {
            nodes
                .iter()
                .filter(|n| n.synthetic == Some(true))
                .map(|n| n.peer_id.as_str())
                .collect()
        } else {
            HashSet::new()
        };
        let edges = edges
            .into_iter()
            .filter(|e| {
                (self.allows_key(&e.topic) || synth.contains(e.source.as_str()))
                    && kept.contains(e.source.as_str())
                    && e.sink.as_deref().map_or(true, |s| kept.contains(s))
            })
            .collect();
        (nodes, edges)
    }

    /// May this viewer see one producer row of the `/api/top` table? A
    /// footprint beat rides a floor-visible carrier key
    /// (`ops/fabric/footprint/<peer>`), but its CONTENT names an app
    /// producer — the same class of information the snapshot's node filter
    /// protects ("an app peer's existence is itself scoped information").
    /// So the row gates on the producer identity, not the carrier key:
    /// granted anchors, infrastructure peers, and — under the notional
    /// grant — producers the topology resolves as `synthetic: true`
    /// (pinned-anchor peer or a synthetic peer publishing under the
    /// anchor). Fail-closed: an anchor the topology can't resolve is
    /// visible only when explicitly granted.
    pub fn allows_producer_row(
        &self,
        anchor: &str,
        topology: &panoscope_core::Topology,
    ) -> bool {
        match self {
            ViewerScope::Full | ViewerScope::DataAll => true,
            ViewerScope::Scoped { anchors, include_notional } => {
                if anchors.contains(anchor) {
                    return true;
                }
                if let Some(p) = topology.peers.get(anchor) {
                    if p.node_class() == NodeClass::Infra {
                        return true;
                    }
                    if *include_notional && p.synthetic == Some(true) {
                        return true;
                    }
                }
                // Legacy random-ZID producers: the footprint anchor is the
                // slot anchor, not the topology peer_id — resolve synthetic
                // through any synthetic peer publishing under the anchor.
                *include_notional
                    && topology.peers.values().any(|p| {
                        p.synthetic == Some(true)
                            && p.publishes.iter().any(|k| seg_prefixed(k, anchor))
                    })
            }
        }
    }

    /// Filter one live WS event. Topic-bearing events gate on `allows_key`;
    /// peer-bearing events gate on the inspect rule (an app peer's
    /// existence is itself scoped information for lower scopes). The ws
    /// loop additionally rescues infra + notional PeerSeen via topology
    /// lookup (node_class / synthetic aren't carried on the event).
    pub fn allows_event(&self, ev: &WsEvent) -> bool {
        match self {
            ViewerScope::Full => true,
            _ => match ev {
                WsEvent::PeerSeen { peer_id, .. } => self.allows_peer_inspect(peer_id),
                WsEvent::PublishSampled { topic, .. }
                | WsEvent::PublishDropped { topic, .. }
                | WsEvent::SampleArrived { topic, .. } => self.allows_key(topic),
            },
        }
    }
}

fn key_in_common(key: &str) -> bool {
    !key.starts_with(AUDIT_PREFIX) && COMMON_PREFIXES.iter().any(|p| key.starts_with(p))
}

fn key_in_orgs(key: &str, orgs: &HashSet<String>) -> bool {
    // `<anchor>/...` or `volatile/<anchor>/...` (ADR 1032 class).
    let bare = key.strip_prefix("volatile/").unwrap_or(key);
    orgs.iter().any(|o| seg_prefixed(key, o) || seg_prefixed(bare, o))
}

/// True when `key` starts with `anchor` at a whole-segment boundary
/// (`abc123/...` matches anchor `abc123`; `abc123fff/...` does not).
fn seg_prefixed(key: &str, anchor: &str) -> bool {
    key.starts_with(anchor) && key[anchor.len()..].starts_with('/')
}

fn split_groups(raw: &str) -> Vec<String> {
    raw.split([',', '|', ' '])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot_view::NodeView;
    use panoscope_core::Role;

    fn cfg_enforce() -> RbacConfig {
        RbacConfig {
            mode: RbacMode::Enforce,
            ..RbacConfig::default()
        }
    }

    fn cfg_enforce_no_notional() -> RbacConfig {
        RbacConfig {
            mode: RbacMode::Enforce,
            notional_default: false,
            ..RbacConfig::default()
        }
    }

    fn headers(groups: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(g) = groups {
            h.insert(GROUPS_HEADER, g.parse().unwrap());
        }
        h
    }

    fn scoped(anchors: &[&str], include_notional: bool) -> ViewerScope {
        ViewerScope::Scoped {
            anchors: anchors.iter().map(|s| s.to_string()).collect(),
            include_notional,
        }
    }

    fn node(peer_id: &str, class: NodeClass, publishes: &[&str]) -> NodeView {
        NodeView {
            peer_id: peer_id.into(),
            slot_alias: None,
            display_name: None,
            vendor: None,
            role: Role::Source,
            publishes: publishes.iter().map(|s| s.to_string()).collect(),
            subscribes: vec![],
            first_seen_ms: 0,
            last_seen_ms: 0,
            site: None,
            role_class: None,
            format_hint: None,
            synthetic: None,
            node_kind: None,
            attest_source: None,
            locator: None,
            node_class: class,
            node_subkind: None,
            universal_observer: false,
            hygiene: None,
        }
    }

    #[test]
    fn off_mode_is_full_even_without_headers() {
        let cfg = RbacConfig::default();
        assert_eq!(ViewerScope::from_headers(&headers(None), &cfg), ViewerScope::Full);
    }

    #[test]
    fn enforce_no_header_fails_closed_to_floor() {
        // Fail-closed floor: no anchors; notional only because the
        // goatnet default grants it (SoT default implication).
        assert_eq!(
            ViewerScope::from_headers(&headers(None), &cfg_enforce()),
            scoped(&[], true)
        );
        assert_eq!(
            ViewerScope::from_headers(&headers(None), &cfg_enforce_no_notional()),
            scoped(&[], false)
        );
    }

    #[test]
    fn forwarded_groups_header_is_the_trusted_source() {
        // The reverse-proxy sidecar injects the groups claim as
        // X-Forwarded-Groups on the upstream request (GROUPS_HEADER). Reading
        // it is what makes a real login resolve.
        let mut h = HeaderMap::new();
        h.insert(GROUPS_HEADER, "ADMINS".parse().unwrap());
        assert_eq!(ViewerScope::from_headers(&h, &cfg_enforce()), ViewerScope::Full);
    }

    #[test]
    fn x_auth_request_groups_is_ignored_no_forgery_escalation() {
        // SECURITY: oauth2-proxy in reverse-proxy mode does NOT strip a
        // client-supplied X-Auth-Request-Groups, so it is forgeable. It must
        // never grant scope — a floor viewer sending X-Auth-Request-Groups:
        // ADMINS stays on the floor.
        let mut h = HeaderMap::new();
        h.insert("x-auth-request-groups", "ADMINS".parse().unwrap());
        assert_eq!(
            ViewerScope::from_headers(&h, &cfg_enforce()),
            scoped(&[], true),
            "a forged x-auth-request-groups must NOT escalate"
        );
    }

    #[test]
    fn platform_groups_imply_full_via_declared_edge() {
        for g in ["ADMINS", "OTHER,ADMINS"] {
            assert_eq!(
                ViewerScope::from_headers(&headers(Some(g)), &cfg_enforce()),
                ViewerScope::Full,
                "groups={g}"
            );
        }
    }

    #[test]
    fn iam_admin_is_not_a_data_grant() {
        // Second-pass narrowing: IAM-ADMIN administers identity/access,
        // which does not need standing all-mission-data view (the matrix
        // grants it panoscope `scoped`). Re-widening = one SoT implication
        // row + the matrix row back to `read` + the role default.
        assert_eq!(
            ViewerScope::from_headers(&headers(Some("IAM-ADMIN")), &cfg_enforce()),
            scoped(&[], true)
        );
    }

    #[test]
    fn obs_groups_are_not_a_data_grant() {
        // G2 orthogonality: platform-obs privilege is not data access.
        assert_eq!(
            ViewerScope::from_headers(&headers(Some("OBS-READONLY,GRAFANA-ADMIN")), &cfg_enforce()),
            scoped(&[], true)
        );
    }

    #[test]
    fn view_all_grant_gets_dataall_not_full() {
        // An observer with data.view.all sees all mission data but is
        // NOT platform tier (orthogonality both ways).
        assert_eq!(
            ViewerScope::from_headers(&headers(Some("data.view.all")), &cfg_enforce()),
            ViewerScope::DataAll
        );
    }

    #[test]
    fn org_and_producer_grants_collect_anchors() {
        let scope = ViewerScope::from_headers(
            &headers(Some("data.view.org:abc123,data.view.producer:def456")),
            &cfg_enforce(),
        );
        assert_eq!(scope, scoped(&["abc123", "def456"], true));
    }

    #[test]
    fn explicit_notional_grant_works_when_default_off() {
        let scope = ViewerScope::from_headers(
            &headers(Some("data.view.notional")),
            &cfg_enforce_no_notional(),
        );
        assert_eq!(scope, scoped(&[], true));
    }

    #[test]
    fn full_beats_data_grants() {
        assert_eq!(
            ViewerScope::from_headers(
                &headers(Some("data.view.org:abc123,ADMINS")),
                &cfg_enforce()
            ),
            ViewerScope::Full
        );
    }

    #[test]
    fn dataall_sees_everything_but_platform_audit() {
        let m = ViewerScope::DataAll;
        assert!(m.allows_key("anyorg/telemetry/v1"));
        assert!(m.allows_key("ops/fabric/health/site-a"));
        assert!(!m.allows_key("ops/audit/operator/pat-operator"));
    }

    #[test]
    fn scoped_key_rules() {
        let p = scoped(&["abc123"], true);
        assert!(p.allows_key("abc123/telemetry/v1"));
        assert!(p.allows_key("volatile/abc123/stream/x"));
        assert!(p.allows_key("ops/fabric/health/site-a"));
        assert!(p.allows_key("attest/schema/x/y/z/v1"));
        assert!(!p.allows_key("ops/audit/operator/x"));
        assert!(!p.allows_key("otherorg/telemetry/v1"));
        assert!(!p.allows_key("abc123fff/telemetry/v1"), "prefix must match a whole segment");
    }

    #[test]
    fn synthetic_key_second_chance_requires_notional_grant() {
        let with = scoped(&[], true);
        let without = scoped(&[], false);
        assert!(with.allows_synthetic_key(true));
        assert!(!with.allows_synthetic_key(false));
        assert!(!without.allows_synthetic_key(true));
        assert!(!ViewerScope::Full.allows_synthetic_key(true), "Full never needs the second chance");
    }

    #[test]
    fn floor_key_scope_is_common_only() {
        let o = scoped(&[], false);
        assert!(o.allows_key("ops/mesh/health/site-a"));
        assert!(o.allows_key("attest/schema/a/b/c/v1"));
        assert!(!o.allows_key("anyorg/telemetry/v1"));
        assert!(!o.allows_key("ops/audit/operator/x"));
    }

    #[test]
    fn scoped_node_rules_incl_notional() {
        let p = scoped(&["abc123"], true);
        // Own pinned-anchor peer.
        assert!(p.allows_node(&node("abc123", NodeClass::App, &[])));
        // Own legacy peer (random ZID) publishing under the granted anchor.
        assert!(p.allows_node(&node("zid-rand", NodeClass::App, &["abc123/telemetry/v1"])));
        // Infrastructure always visible.
        assert!(p.allows_node(&node("router-1", NodeClass::Infra, &[])));
        // Notional producer visible under the notional grant...
        let mut synth = node("tarsier-alpha", NodeClass::App, &["notional/tarsier/v1"]);
        synth.synthetic = Some(true);
        assert!(p.allows_node(&synth));
        // ...but not without it.
        let p_no_notional = scoped(&["abc123"], false);
        assert!(!p_no_notional.allows_node(&synth));
        // Foreign real participant never.
        assert!(!p.allows_node(&node("other", NodeClass::App, &["other/t/v1"])));
        // Subscribing to my topics does not make a foreign node mine.
        let mut foreign = node("other", NodeClass::App, &[]);
        foreign.subscribes = vec!["abc123/telemetry/v1".to_string()];
        assert!(!p.allows_node(&foreign));
    }

    #[test]
    fn filter_snapshot_drops_edges_with_hidden_endpoints() {
        let p = scoped(&["abc123"], false);
        let nodes = vec![
            node("abc123", NodeClass::App, &["abc123/t/v1"]),
            node("other", NodeClass::App, &["other/t/v1"]),
        ];
        let mk_edge = |source: &str, topic: &str| EdgeView {
            source: source.into(),
            sink: None,
            topic: topic.into(),
            ema_eps: 0.0,
            projected_eps: 0.0,
            total_events: 1,
            last_event_ms: 0,
            schema_family: None,
            format: None,
            folded_singletons_count: None,
            folded_namespace: None,
        };
        let edges = vec![mk_edge("abc123", "abc123/t/v1"), mk_edge("other", "other/t/v1")];
        let (nodes, edges) = p.filter_snapshot(nodes, edges);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].peer_id, "abc123");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].topic, "abc123/t/v1");
    }

    #[test]
    fn filter_snapshot_keeps_synthetic_edges_under_notional_grant() {
        // The notional default must grant the WHOLE synthetic fleet —
        // nodes AND their traffic. A synthetic producer's edges survive
        // even though its topic keys match no granted anchor.
        let mk_nodes = || {
            let mut synth = node("tarsier-alpha", NodeClass::App, &["notional/tarsier/v1"]);
            synth.synthetic = Some(true);
            vec![synth, node("other", NodeClass::App, &["other/t/v1"])]
        };
        let mk_edge = |source: &str, topic: &str| EdgeView {
            source: source.into(),
            sink: None,
            topic: topic.into(),
            ema_eps: 0.0,
            projected_eps: 0.0,
            total_events: 1,
            last_event_ms: 0,
            schema_family: None,
            format: None,
            folded_singletons_count: None,
            folded_namespace: None,
        };
        let mk_edges = || {
            vec![mk_edge("tarsier-alpha", "notional/tarsier/v1"), mk_edge("other", "other/t/v1")]
        };

        let with_notional = scoped(&[], true);
        let (n, e) = with_notional.filter_snapshot(mk_nodes(), mk_edges());
        assert_eq!(n.len(), 1, "synthetic node kept, foreign real node dropped");
        assert_eq!(e.len(), 1, "synthetic producer's edge kept");
        assert_eq!(e[0].topic, "notional/tarsier/v1");

        let without_notional = scoped(&[], false);
        let (n, e) = without_notional.filter_snapshot(mk_nodes(), mk_edges());
        assert!(n.is_empty() && e.is_empty(), "no notional grant ⇒ nothing");
    }

    #[test]
    fn ws_events_gate_by_scope() {
        let o = scoped(&[], false);
        assert!(o.allows_event(&WsEvent::SampleArrived {
            topic: "ops/fabric/health/a".into(),
            ts_ms: 1
        }));
        assert!(!o.allows_event(&WsEvent::SampleArrived { topic: "org/t/v1".into(), ts_ms: 1 }));
        assert!(!o.allows_event(&WsEvent::PeerSeen { peer_id: "someorg".into(), ts_ms: 1 }));
        assert!(ViewerScope::Full
            .allows_event(&WsEvent::PeerSeen { peer_id: "someorg".into(), ts_ms: 1 }));
    }

    // ---- ADR 1074 B1: resolver-role enrichment (enriched_with) ----

    #[test]
    fn enrich_adds_resolver_org_and_producer_anchors() {
        // A participant whose claim carried no data grant, but whose
        // registry rows derive org + producer roles, gets those anchors.
        let base = scoped(&[], false);
        let roles = vec![
            "data.view.org:abc123".to_string(),
            "data.view.producer:def456".to_string(),
        ];
        let got = base.enriched_with(&roles);
        match got {
            ViewerScope::Scoped { anchors, include_notional } => {
                assert!(anchors.contains("abc123"));
                assert!(anchors.contains("def456"));
                assert!(!include_notional, "notional not implied by org/producer roles");
            }
            other => panic!("expected Scoped, got {other:?}"),
        }
    }

    #[test]
    fn enrich_unions_with_existing_claim_anchors() {
        // Claim-derived anchor survives; resolver anchor is added (union).
        let got = scoped(&["claimorg"], true).enriched_with(&["data.view.org:reg".to_string()]);
        match got {
            ViewerScope::Scoped { anchors, include_notional } => {
                assert!(anchors.contains("claimorg"), "claim anchor preserved");
                assert!(anchors.contains("reg"), "resolver anchor added");
                assert!(include_notional, "existing notional grant preserved");
            }
            other => panic!("expected Scoped, got {other:?}"),
        }
    }

    #[test]
    fn enrich_view_all_upgrades_scoped_to_dataall() {
        let got = scoped(&["x"], false).enriched_with(&["data.view.all".to_string()]);
        assert_eq!(got, ViewerScope::DataAll, "resolver data.view.all ⇒ DataAll");
    }

    #[test]
    fn enrich_notional_role_turns_on_notional() {
        let got = scoped(&[], false).enriched_with(&["data.view.notional".to_string()]);
        assert!(matches!(got, ViewerScope::Scoped { include_notional: true, .. }));
    }

    #[test]
    fn enrich_never_downgrades_full_or_dataall() {
        // A participant resolver must never narrow a platform/apex viewer.
        assert_eq!(ViewerScope::Full.enriched_with(&[]), ViewerScope::Full);
        assert_eq!(
            ViewerScope::DataAll.enriched_with(&["data.view.org:x".to_string()]),
            ViewerScope::DataAll
        );
    }

    #[test]
    fn enrich_empty_roles_is_identity_failclosed() {
        // Resolver miss/error path (caller passes no roles): base unchanged.
        let base = scoped(&["own"], false);
        assert_eq!(base.clone().enriched_with(&[]), base, "empty roles never widen or narrow");
    }

    #[test]
    fn enrich_ignores_unknown_or_empty_role_values() {
        let got = scoped(&[], false).enriched_with(&[
            "data.view.org:".to_string(),      // empty value — skipped
            "data.write.producer:x".to_string(), // not a view grant — skipped
            "garbage".to_string(),
        ]);
        assert_eq!(got, scoped(&[], false), "malformed/foreign roles are inert");
    }

    #[test]
    fn sampling_drops_are_scoped_like_any_other_key() {
        let mut drops = std::collections::HashMap::new();
        drops.insert("abc123/t/v1".to_string(), 3u64);
        drops.insert("def456/t/v1".to_string(), 5u64);
        drops.insert("ops/audit/x".to_string(), 1u64);
        let scoped = ViewerScope::Scoped {
            anchors: ["abc123".to_string()].into_iter().collect(),
            include_notional: true,
        };
        let seen = scoped.filter_sampling_drops(&drops);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen.get("abc123/t/v1"), Some(&3));
        assert!(!ViewerScope::DataAll.filter_sampling_drops(&drops).contains_key("ops/audit/x"));
        assert_eq!(ViewerScope::Full.filter_sampling_drops(&drops).len(), 3);
    }
}
