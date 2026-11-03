// SPDX-License-Identifier: Apache-2.0
//
// HTTP routes:
//   GET /            — server-rendered diagnosis page (HTML)
//   GET /api/diagnose — SSE stream of CheckOutcome events
//   GET /api/index    — JSON snapshot of {vendors, topics, systems}
//                       for the picker's live typeahead. Cached for
//                       30s in-process so repeat opens don't hammer
//                       the portal.
//   GET /healthz     — liveness; returns 200 OK with build info
//
// Both / and /api/diagnose parse the same DiagnoseQuery from the
// query string so a copy-paste between them works.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Serialize;
use tokio::sync::Mutex;

use panoscope_core::{AliasRegistry, Labeled};
use stethoscope_checks::context::TargetKind;
use stethoscope_checks::{CheckCtx, Registry, Target};

use crate::config::Siblings;
use crate::render;
use crate::sse::diagnose_stream;
use crate::target::{DiagnoseQuery, TargetParseError};

/// Load the slot-alias registry (ADR 1039) from the portal, degrading to an
/// empty (UUID-only) registry on any error — unconfigured portal, unreachable,
/// non-2xx. Aliases are an enhancement, never a dependency: a portal hiccup
/// must not break diagnosis. Loaded per-request (stethoscope is interactive +
/// low-rate, and the portal is a same-host sibling), mirroring goat-cli's
/// per-command load rather than a background poller.
async fn load_alias_registry(ctx: &CheckCtx) -> AliasRegistry {
    match ctx.portal.list_aliases().await {
        Ok(rows) => AliasRegistry::from_rows(rows),
        Err(_) => AliasRegistry::empty(),
    }
}

/// The real entity name (portal `display_name`, e.g. "Example Org") for a
/// slot-anchor identity, so the hero can headline the entity instead of its
/// bare UUID or slot handle (ADR 1039). Only consulted when the canonical
/// target value is anchor-shaped — per-handle goatnets (nova-a) keep the handle.
/// Degrades to `None` on any portal error (unconfigured/unreachable) + on an
/// empty `display_name`, so the hero falls back to the alias/value. Loaded
/// per-request like the alias registry (interactive, low-rate, same-host portal).
async fn load_entity_name(ctx: &CheckCtx, anchor: &str) -> Option<String> {
    if !panoscope_core::alias::is_anchor_shaped(anchor) {
        return None;
    }
    let vendors = ctx.portal.list_vendors().await.ok()?;
    vendors
        .into_iter()
        .find(|v| v.prefix == anchor)
        .map(|v| v.display_name)
        .filter(|n| !n.is_empty())
}

/// Resolve an alias to the canonical UUID anchor (ADR 1039 input-resolve), so
/// an operator can drive any target by its friendly name. A vendor target
/// (`?vendor=pat-example`) is a bare anchor/alias → `resolve_input` (alias →
/// UUID; UUID-shaped input passes through). A topic/system target
/// (`?topic=tarsier-alpha/obs/v1`, `?system=tarsier/eo`) has the slot anchor as
/// its leading segment → `resolve_key`, leaving the trailing taxonomy untouched.
///
/// Fails closed on an unknown alias against a populated registry (the F-177
/// trap — same contract as `goat pub`), which the caller renders as a friendly
/// "did you mean…". An empty/degraded registry passes everything through
/// verbatim (UUID-shaped input always resolves to itself).
fn canonicalize_target(
    reg: &AliasRegistry,
    target: Target,
) -> Result<Target, panoscope_core::alias::ResolveError> {
    match target.kind {
        TargetKind::Vendor => Ok(Target::vendor(reg.resolve_input(&target.value)?)),
        TargetKind::Topic => Ok(Target::topic(reg.resolve_key(&target.value)?)),
        TargetKind::System => Ok(Target::system(reg.resolve_key(&target.value)?)),
    }
}

/// The operator-facing rendering of a (canonical) target value — the slot
/// alias projected over the anchor (ADR 1039 output-render). A vendor anchor
/// renders whole (`render`); a topic/system key renders its leading anchor
/// segment, leaving the taxonomy verbatim (`render_key`). Falls back to the
/// truncated/verbatim value when no alias is known.
fn target_display_value(reg: &AliasRegistry, target: &Target) -> String {
    match target.kind {
        TargetKind::Vendor => reg.render(&target.value),
        TargetKind::Topic | TargetKind::System => reg.render_key(&target.value),
    }
}

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<Registry>,
    pub ctx: CheckCtx,
    pub site_label: String,
    pub index_cache: Arc<Mutex<IndexCache>>,
    /// Block 94 cross-surface deep-link contract — sibling-surface
    /// base URLs (panoscope + flashlight). Rendered into the topbar
    /// as `see in panoscope ↗` + `watch in flashlight ↗` buttons
    /// preserving the current target.
    pub siblings: Siblings,
    /// Display-only participant-noun lexicon (nova-a "vendor" / ZAGROS "org").
    /// Rendered into the picker chip + hero eyebrow; never a machine path.
    pub lexicon: crate::config::Lexicon,
}

/// In-process cache for `/api/index`. The portal is a sibling service
/// on the same mgmt host (sub-millisecond network latency), but a
/// vendor-list + per-vendor topic-list fan-out still adds up — a
/// 30-second cache means a busy operator typing keeps reusing the
/// same snapshot. Cache stays empty (`None`) until first request.
#[derive(Default)]
pub struct IndexCache {
    pub snapshot: Option<(Instant, IndexResponse)>,
}

const INDEX_CACHE_TTL: Duration = Duration::from_secs(30);

/// A picker typeahead entry for an identity (vendor/org). `id` is the canonical
/// navigate value (UUID slot anchor on slot-pool goatnets like ZAGROS, or the
/// plain handle on per-vendor goatnets like nova-a). `label` is the friendly name
/// shown in the dropdown (portal `display_name` → slot alias → none, in which
/// case the client falls back to `id`). `aka` are extra names matched but not
/// necessarily shown — the display_name, the slot handle, the topic
/// vendor-segment — so an operator finds the entity by ANY name it goes by, not
/// just its UUID (the F-199 / ADR 1039 follow-on: the index used to be
/// UUID-only, so typing a human name surfaced nothing).
#[derive(Clone, Serialize)]
pub struct VendorEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aka: Vec<String>,
}

#[derive(Clone, Serialize)]
pub struct IndexResponse {
    /// Identities as [`VendorEntry`] (id + friendly label + searchable aka),
    /// so the typeahead matches + displays the human name, not the bare UUID.
    pub vendors: Vec<VendorEntry>,
    /// Topics as the shared [`Labeled`] envelope (ADR 1039): `id` is the
    /// canonical key (what the picker navigates with — `key=` stays UUID),
    /// `alias` is the projected display key when the leading anchor has an
    /// alias, else absent. The typeahead renders `alias ?? id` and matches
    /// the needle against both, so an operator finds a topic by its friendly
    /// name OR its UUID. Systems are a different namespace (not the
    /// slot-alias table) and stay plain strings.
    pub topics: Vec<Labeled>,
    pub systems: Vec<String>,
    /// Set when the portal client is unconfigured, so the picker can
    /// degrade gracefully ("autocomplete unavailable — portal not
    /// configured") instead of looking broken.
    pub portal_disabled: bool,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(home))
        .route("/api/diagnose", get(diagnose))
        .route("/api/index", get(index))
        .route("/healthz", get(healthz))
        .with_state(state)
}

async fn home(
    State(state): State<AppState>,
    Query(q): Query<DiagnoseQuery>,
) -> Response {
    match q.into_target() {
        Ok(target) => {
            // Resolve an alias in the leading segment to the canonical anchor
            // before rendering (ADR 1039). Unknown alias → friendly error
            // page (fail closed) rather than a phantom-prefix diagnosis.
            let reg = load_alias_registry(&state.ctx).await;
            let target = match canonicalize_target(&reg, target) {
                Ok(t) => t,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Html(render::render_target_error(&e.to_string(), &state.site_label)),
                    )
                        .into_response()
                }
            };
            let display_value = target_display_value(&reg, &target);
            // The real entity name for a slot-anchor identity target (the hero
            // headline), else None → hero falls back to the alias/value.
            let entity_name = if target.kind == TargetKind::Vendor {
                load_entity_name(&state.ctx, &target.value).await
            } else {
                None
            };
            Html(render::render_page(
                &target,
                &display_value,
                entity_name.as_deref(),
                &state.registry,
                &state.site_label,
                "/api/diagnose",
                &state.siblings,
                &state.lexicon,
            ))
            .into_response()
        }
        // "No target passed" is not an error — it's the URL state for
        // "I haven't decided yet; help me pick." Render the picker
        // landing so the operator can place the stethoscope.
        Err(TargetParseError::Missing) => {
            Html(render::render_picker_landing(&state.site_label, &state.lexicon)).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Html(render::render_target_error(&e.to_string(), &state.site_label)),
        )
            .into_response(),
    }
}

async fn diagnose(
    State(state): State<AppState>,
    Query(q): Query<DiagnoseQuery>,
) -> Response {
    match q.into_target() {
        Ok(target) => {
            // Resolve the alias to the canonical anchor so checks run on the
            // real key (ADR 1039). Unknown alias → SSE error (fail closed),
            // same contract as the home page + `goat pub`.
            let alias_reg = load_alias_registry(&state.ctx).await;
            let target = match canonicalize_target(&alias_reg, target) {
                Ok(t) => t,
                Err(e) => {
                    return (StatusCode::BAD_REQUEST, format!(":error\ndata: {e}\n\n"))
                        .into_response()
                }
            };
            // Reconstruct a fresh Registry view — we hold an Arc; the
            // SSE handler owns a clone of the underlying data. (The
            // Registry itself is Clone via BTreeMap of Arcs.)
            let reg = (*state.registry).clone();
            diagnose_stream(reg, state.ctx, target).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            format!(":error\ndata: {e}\n\n"),
        )
            .into_response(),
    }
}

/// `GET /api/index` — JSON of vendors, topics, systems for the picker.
///
/// Cached in-process for 30s. Portal-disabled installs (no API key)
/// return an empty body with `portal_disabled: true` so the client
/// can label the picker honestly. Topics are returned as plain
/// `key_pattern` strings (full registry shape), not envelope objects
/// — the typeahead just needs identifiers, not the per-row metadata.
///
/// System inventory: the portal doesn't currently track a separate
/// "system" entity (it's a free-form `vendor/system` convention),
/// so we synthesize it from the topic keys — every distinct
/// `vendor/<first-segment>` becomes a system suggestion. When the
/// portal grows a real systems table this synthesis goes away.
async fn index(State(state): State<AppState>) -> Response {
    let mut cache = state.index_cache.lock().await;
    if let Some((stamp, snap)) = &cache.snapshot {
        if stamp.elapsed() < INDEX_CACHE_TTL {
            return cached_json(snap);
        }
    }

    let portal = &state.ctx.portal;
    if !portal.is_enabled() {
        let empty = IndexResponse {
            vendors: vec![],
            topics: vec![],
            systems: vec![],
            portal_disabled: true,
        };
        cache.snapshot = Some((Instant::now(), empty.clone()));
        return cached_json(&empty);
    }

    let vendors = portal.list_vendors().await.unwrap_or_default();
    // Project topic keys through the slot-alias registry (ADR 1039) so the
    // typeahead can show friendly names; degrades to UUID keys when the
    // portal alias table is empty/unreachable.
    let alias_reg = load_alias_registry(&state.ctx).await;

    // Build the identity index: each portal vendor → {id: UUID/handle,
    // label: display_name → slot alias, aka: both names} so the typeahead
    // finds it by its real name (display_name like "Example Org"), its slot
    // handle (the ADR 1039 alias), or its UUID — and SHOWS the friendly name.
    let mut vendor_entries: Vec<VendorEntry> = Vec::new();
    let mut seen_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for v in &vendors {
        let slot_alias = alias_reg.alias_of(&v.prefix);
        let display = (!v.display_name.is_empty()).then(|| v.display_name.clone());
        let label = display.clone().or_else(|| slot_alias.clone());
        let mut aka: Vec<String> = Vec::new();
        if let Some(d) = &display {
            aka.push(d.clone());
        }
        if let Some(a) = &slot_alias {
            aka.push(a.clone());
        }
        seen_ids.insert(v.prefix.clone());
        vendor_entries.push(VendorEntry { id: v.prefix.clone(), label, aka });
    }

    let mut topic_keys: Vec<String> = Vec::new();
    let mut system_keys: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    for v in &vendors {
        if let Ok(topics) = portal.list_topics(&v.prefix).await {
            for t in topics {
                // System suggestion: vendor/<first-segment-of-suffix>.
                if let Some(rest) = t.key_pattern.strip_prefix(&format!("{}/", v.prefix)) {
                    if let Some((sys, _)) = rest.split_once('/') {
                        system_keys.insert(format!("{}/{}", v.prefix, sys));
                    }
                }
                // Slot-pool rows are anchor-named (`<anchor>/<vendor>/...`):
                // surface the human vendor segment as a typeahead vendor too,
                // so the anchor-aware `?vendor=` target is reachable
                // from the picker without knowing the UUID.
                let seg = panoscope_core::vendor_segment(&t.key_pattern);
                if seg != t.key_pattern.split('/').next().unwrap_or("")
                    && !seg.is_empty()
                    && seen_ids.insert(seg.to_string())
                {
                    vendor_entries.push(VendorEntry {
                        id: seg.to_string(),
                        label: None,
                        aka: Vec::new(),
                    });
                }
                topic_keys.push(t.key_pattern);
            }
        }
    }
    vendor_entries.sort_by(|a, b| {
        // Order by the operator-facing name (label when present, else id) so
        // the typeahead's breadth-first grouping starts from friendly names.
        let ka = a.label.as_deref().unwrap_or(&a.id).to_lowercase();
        let kb = b.label.as_deref().unwrap_or(&b.id).to_lowercase();
        ka.cmp(&kb).then_with(|| a.id.cmp(&b.id))
    });
    topic_keys.sort();
    topic_keys.dedup();
    // Canonical key stays `id`; alias-projected key (if any) rides alongside.
    let topics: Vec<Labeled> = topic_keys.iter().map(|k| alias_reg.label_key(k)).collect();

    let snap = IndexResponse {
        vendors: vendor_entries,
        topics,
        systems: system_keys.into_iter().collect(),
        portal_disabled: false,
    };
    cache.snapshot = Some((Instant::now(), snap.clone()));
    cached_json(&snap)
}

fn cached_json(snap: &IndexResponse) -> Response {
    let mut resp = Json(snap).into_response();
    // Browser-side cache: 25s, so a refresh just under our 30s TTL
    // still gets a fresh snapshot.
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        "private, max-age=25".parse().unwrap(),
    );
    resp
}

async fn healthz() -> &'static str {
    concat!(
        "stethoscope ",
        env!("CARGO_PKG_VERSION"),
        " ok"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use panoscope_core::{AuditTail, AuditTailConfig, PortalClient};
    use stethoscope_checks::checks;
    use tower::ServiceExt;

    fn test_router() -> Router {
        let mut reg = Registry::new();
        checks::register_phase_b(&mut reg);
        let ctx = CheckCtx::new(
            PortalClient::new(String::new(), String::new()),
            AuditTail::new(AuditTailConfig::default()).unwrap(),
        );
        router(AppState {
            registry: Arc::new(reg),
            ctx,
            site_label: "test".into(),
            index_cache: Arc::new(Mutex::new(Default::default())),
            siblings: Siblings::default(),
            lexicon: crate::config::Lexicon::default(),
        })
    }

    async fn body_string(resp: Response) -> String {
        let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    use panoscope_core::portal::AliasRow;

    const ANCHOR: &str = "e7a9c75b532e3d434bb09fefd3ce1ec7";

    fn alias_reg() -> AliasRegistry {
        AliasRegistry::from_rows([AliasRow {
            slot_id: ANCHOR.to_string(),
            alias: "tarsier-alpha".into(),
        }])
    }

    #[test]
    fn canonicalize_resolves_topic_alias_to_anchor() {
        // `?topic=tarsier-alpha/obs/v1` → checks run on the canonical anchor.
        let t = canonicalize_target(&alias_reg(), Target::topic("tarsier-alpha/obs/v1")).unwrap();
        assert_eq!(t.value, format!("{ANCHOR}/obs/v1"));
        // A UUID-anchored topic passes through verbatim.
        let t2 =
            canonicalize_target(&alias_reg(), Target::topic(format!("{ANCHOR}/obs/v1"))).unwrap();
        assert_eq!(t2.value, format!("{ANCHOR}/obs/v1"));
    }

    #[test]
    fn canonicalize_fails_closed_on_unknown_topic_alias() {
        // Populated registry + unknown leading alias = error,
        // same contract as `goat pub`.
        assert!(canonicalize_target(&alias_reg(), Target::topic("tarsir/obs/v1")).is_err());
    }

    #[test]
    fn canonicalize_resolves_vendor_and_system_alias() {
        // Vendor alias → canonical anchor (the `?vendor=tarsier-alpha` path).
        let v = canonicalize_target(&alias_reg(), Target::vendor("tarsier-alpha")).unwrap();
        assert_eq!(v.value, ANCHOR);
        // A UUID-anchored vendor target passes through verbatim (the ZAGROS
        // `?vendor=<uuid>` path from a panoscope/portal deep-link).
        let v2 = canonicalize_target(&alias_reg(), Target::vendor(ANCHOR)).unwrap();
        assert_eq!(v2.value, ANCHOR);
        // System leading segment resolves; taxonomy stays verbatim.
        let s = canonicalize_target(&alias_reg(), Target::system("tarsier-alpha/radar")).unwrap();
        assert_eq!(s.value, format!("{ANCHOR}/radar"));
    }

    #[test]
    fn canonicalize_fails_closed_on_unknown_vendor_alias() {
        // A populated registry + an unknown, non-UUID vendor token fails closed
        // (the F-177 trap) — same contract as topics. A per-vendor goatnet (no
        // aliases) instead passes plain handles through (covered below).
        assert!(canonicalize_target(&alias_reg(), Target::vendor("tarsir")).is_err());
        // nova-a per-vendor: empty registry → plain handle passes through, never
        // a spurious fail-closed.
        let v = canonicalize_target(&AliasRegistry::empty(), Target::vendor("drs")).unwrap();
        assert_eq!(v.value, "drs");
    }

    #[test]
    fn display_value_projects_alias_for_all_kinds() {
        let reg = alias_reg();
        // Topic: leading anchor → alias, taxonomy verbatim.
        assert_eq!(
            target_display_value(&reg, &Target::topic(format!("{ANCHOR}/obs/v1"))),
            "tarsier-alpha/obs/v1"
        );
        // Vendor: the bare anchor renders as its alias (the hero-render fix —
        // was showing the raw UUID).
        assert_eq!(
            target_display_value(&reg, &Target::vendor(ANCHOR)),
            "tarsier-alpha"
        );
        // System: leading anchor → alias.
        assert_eq!(
            target_display_value(&reg, &Target::system(format!("{ANCHOR}/radar"))),
            "tarsier-alpha/radar"
        );
        // A per-vendor handle (non-anchor) renders verbatim — no false alias.
        assert_eq!(target_display_value(&reg, &Target::vendor("drs")), "drs");
        // Empty/degraded registry (portal-down): topic → leading anchor
        // truncated; vendor anchor → truncated. Never errors. Matches
        // goat-cli's degraded display exactly.
        assert_eq!(
            target_display_value(
                &AliasRegistry::empty(),
                &Target::topic(format!("{ANCHOR}/obs/v1"))
            ),
            "e7a9c75b…/obs/v1"
        );
        assert_eq!(
            target_display_value(&AliasRegistry::empty(), &Target::vendor(ANCHOR)),
            "e7a9c75b…"
        );
    }

    #[tokio::test]
    async fn healthz_returns_version() {
        let resp = test_router()
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.starts_with("stethoscope "));
        assert!(body.ends_with(" ok"));
    }

    #[tokio::test]
    async fn index_returns_empty_when_portal_disabled() {
        // test_router uses an unconfigured PortalClient. /api/index
        // must degrade to an empty snapshot with portal_disabled:true
        // (so the JS can label autocomplete honestly), not 500.
        let resp = test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/index")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains("\"portal_disabled\":true"));
        assert!(body.contains("\"vendors\":[]"));
        assert!(body.contains("\"topics\":[]"));
        assert!(body.contains("\"systems\":[]"));
    }

    #[tokio::test]
    async fn home_with_no_target_renders_picker_landing() {
        let resp = test_router()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        // No target is not an error — it's the "help me pick" state.
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains("Place the stethoscope on"));
        assert!(body.contains("sx-picker__input"));
        // Three kind chips must be present so the operator can switch.
        assert!(body.contains("data-kind=\"topic\""));
        assert!(body.contains("data-kind=\"vendor\""));
        assert!(body.contains("data-kind=\"system\""));
    }

    #[tokio::test]
    async fn home_with_target_includes_change_affordance() {
        let resp = test_router()
            .oneshot(
                Request::builder()
                    .uri("/?vendor=drs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        // The CHANGE button + popover ship with every diagnose page so
        // operators can redirect the stethoscope without round-tripping
        // back to the landing.
        assert!(body.contains("id=\"change-target-btn\""));
        assert!(body.contains("Place the stethoscope on"));
    }

    #[tokio::test]
    async fn home_with_vendor_renders_page() {
        let resp = test_router()
            .oneshot(
                Request::builder()
                    .uri("/?vendor=drs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains("VENDOR"));
        assert!(body.contains("drs"));
        assert!(body.contains("data-check-id=\"registration\""));
        assert!(body.contains("data-check-id=\"flow-rate\""));
    }

    #[tokio::test]
    async fn home_with_multiple_targets_rejected() {
        let resp = test_router()
            .oneshot(
                Request::builder()
                    .uri("/?vendor=drs&topic=drs/foo/bar/v1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_string(resp).await;
        assert!(body.contains("more than one target"));
    }

    #[tokio::test]
    async fn home_with_topic_renders_applicable_cards() {
        let resp = test_router()
            .oneshot(
                Request::builder()
                    .uri("/?topic=drs%2Ffoo%2Fbar%2Fv1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains("TOPIC"));
        // schema-body applies to topic targets only, must be present
        assert!(body.contains("data-check-id=\"schema-body\""));
    }
}
