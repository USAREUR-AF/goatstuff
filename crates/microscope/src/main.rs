//! microscope — admin-gated per-entity behavior lens (ADR 1090).
//! Phase B: JSON API. Phase C: server-rendered pages + the in-app RBAC
//! check + the view-audit trail. Read-only by construction: no route
//! mutates any system the adapters read.
//!
//! RBAC (panoscope `/who` precedent, F-244): the oauth2-proxy sidecar is
//! the outer fence; when `MICROSCOPE_RBAC_MODE=enforce` the app is its own
//! PEP too — it reads `X-Forwarded-Groups` ONLY (never the forgeable
//! `X-Auth-Request-*` family), admits `MICROSCOPE_RBAC_WHO_GROUPS`, and
//! fails closed. `off` (default) ships inert for pre-fence bring-up.

mod adapters;
mod auth;
mod config;
mod event;
mod graph;
mod mesh;
mod render;
mod spine;
mod suggest;
mod timefmt;
mod view;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Json};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use std::io::Write as _;
use std::sync::Arc;

use config::Config;

#[derive(Clone)]
struct App {
    cfg: Arc<Config>,
    /// Typeahead candidate snapshot. Shared across requests because the
    /// roster call behind it costs seconds, not milliseconds.
    sug: Arc<suggest::Cache>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "microscope=info,tower_http=info".into()),
        )
        .init();

    let cfg = Config::from_env();
    let listen = cfg.listen.clone();
    let suggest_ttl = cfg.suggest_ttl_secs;
    let app_state = App {
        cfg: Arc::new(cfg),
        sug: Arc::new(suggest::Cache::new(suggest_ttl)),
    };

    // Prewarm the typeahead so the first operator to type does not pay the
    // roster call's seconds. Failure is not fatal — the endpoint rebuilds on
    // demand and reports what it could not read.
    {
        let warm = app_state.clone();
        tokio::spawn(async move {
            let snap = warm.sug.get(&warm.cfg).await;
            tracing::info!(
                candidates = snap.entries.len(),
                roster_read = snap.roster_read,
                "typeahead snapshot prewarmed"
            );
        });
    }

    let router = Router::new()
        .route("/", get(home))
        .route("/entity", get(entity_page))
        .route("/entity/body", get(entity_body_route))
        .route("/graph", get(graph_page))
        // Content-addressed asset URLs: the `{rev}` segment is an FNV-1a hash
        // of the embedded bytes, so `immutable` caching can never pin a stale
        // file across a rebuild. The segment is not validated — a stale rev
        // still serves the CURRENT bytes, which is the safe direction; the
        // alternative (404 on a stale rev) breaks a page mid-deploy for no
        // gain. Wildcard tail so `fonts/<name>.woff2` reaches the handler
        // intact (axum 0.7 path syntax).
        .route("/healthz", get(healthz))
        .route("/api/meta", get(meta))
        .route("/api/entity", get(entity_api))
        .route("/api/suggest", get(suggest_api))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(app_state);

    tracing::info!(%listen, "microscope listening");
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

async fn healthz() -> &'static str {
    "ok"
}

/// Block 94 sibling-discovery meta.
async fn meta(State(app): State<App>) -> impl IntoResponse {
    Json(serde_json::json!({
        "service": "microscope",
        "version": env!("CARGO_PKG_VERSION"),
        "adr": "1090",
        "siblings": app.cfg.siblings,
    }))
}

#[derive(Deserialize)]
struct EntityQuery {
    id: Option<String>,
    /// Timeline window start. Absent = newest page.
    offset: Option<usize>,
    /// `full=1` renders the whole page server-side with no deferred fetch —
    /// the noscript path, and the one an operator can always fall back to.
    full: Option<String>,
    /// Which MACHINE the reachability graph is drawn for. A person has no
    /// single position in the mesh, so the graph refuses to pick one for them.
    peer: Option<String>,
    /// `all=1` draws every machine the entity is connected to. Opt-in.
    all: Option<String>,
}

/// The resolved caller. `via` records HOW they authenticated (`web` = the
/// oauth2-proxy forwarded-header path, `cli` = a validated operator bearer)
/// so the view-audit shows the admission mode.
struct Viewer {
    email: String,
    groups: Vec<String>,
    via: &'static str,
}

fn viewer_from(headers: &HeaderMap) -> Viewer {
    let h = |k: &str| {
        headers
            .get(k)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    Viewer {
        email: h("x-forwarded-email"),
        groups: h("x-forwarded-groups")
            .split(',')
            .map(|g| g.trim().to_string())
            .filter(|g| !g.is_empty())
            .collect(),
        via: "web",
    }
}

/// In-app admission check for the WEB path. `off` → open (outer fence still
/// applies in deploy); `enforce` → fail-closed group match, case-sensitive
///.
fn admitted(cfg: &Config, viewer: &Viewer) -> bool {
    if cfg.rbac_mode != "enforce" {
        return true;
    }
    viewer.groups.iter().any(|g| cfg.rbac_who_groups.contains(g))
}

/// Resolve the caller and whether they're admitted. Two mutually exclusive
/// paths: a bearer token (CLI — validated against the portal, `X-Forwarded-*`
/// deliberately ignored so a bypass-route request can't forge groups) or the
/// oauth2-proxy forwarded headers (web). When `enforce` is off the surface is
/// open either way (bring-up); a bearer is still resolved for the audit actor.
async fn authenticate(cfg: &Config, headers: &HeaderMap) -> (Viewer, bool) {
    if let Some(bearer) = auth::bearer_token(headers) {
        let actor = auth::resolve_operator(&cfg.portal_url, &bearer).await;
        let admit = cfg.rbac_mode != "enforce" || actor.is_some();
        let viewer = Viewer {
            email: actor.clone().unwrap_or_default(),
            groups: if actor.is_some() {
                vec!["operator-key".into()]
            } else {
                vec![]
            },
            via: "cli",
        };
        return (viewer, admit);
    }
    let viewer = viewer_from(headers);
    let admit = admitted(cfg, &viewer);
    (viewer, admit)
}

/// The view-audit trail: every look is recorded — who examined whom, when,
/// via which surface, admitted or refused. Append-only JSONL. Folding this
/// into the F-207 OperatorActionV1 stream is the named follow-on in the
/// design doc; the trail exists from the first deploy either way.
fn audit_look(cfg: &Config, viewer: &Viewer, selector: &str, surface: &str, outcome: &str) {
    if cfg.view_audit_path.is_empty() {
        return;
    }
    let rec = serde_json::json!({
        "ts": chrono_free_now(),
        "actor": viewer.email,
        "groups": viewer.groups,
        "via": viewer.via,
        "selector": selector,
        "surface": surface,
        "outcome": outcome,
    });
    let path = cfg.view_audit_path.clone();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{rec}");
    } else {
        tracing::warn!(%path, "view-audit append failed");
    }
}

/// RFC3339 UTC now without a chrono dependency.
pub(crate) fn chrono_free_now() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    let (days, rem) = (secs / 86400, secs % 86400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // civil-from-days (Howard Hinnant's algorithm)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

async fn home(State(app): State<App>, headers: HeaderMap) -> impl IntoResponse {
    let (viewer, admit) = authenticate(&app.cfg, &headers).await;
    if !admit {
        audit_look(&app.cfg, &viewer, "", viewer.via, "refused");
        return (
            StatusCode::FORBIDDEN,
            Html(render::forbidden(&app.cfg.site_label, &viewer.groups.join(","))),
        )
            .into_response();
    }
    Html(render::home(&app.cfg.site_label)).into_response()
}

async fn entity_page(
    State(app): State<App>,
    Query(q): Query<EntityQuery>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let selector = q.id.unwrap_or_default();
    let (viewer, admit) = authenticate(&app.cfg, &headers).await;
    if !admit {
        audit_look(&app.cfg, &viewer, &selector, viewer.via, "refused");
        return (
            StatusCode::FORBIDDEN,
            Html(render::forbidden(&app.cfg.site_label, &viewer.groups.join(","))),
        )
            .into_response();
    }
    if selector.trim().is_empty() {
        return Html(render::home(&app.cfg.site_label)).into_response();
    }
    audit_look(&app.cfg, &viewer, selector.trim(), viewer.via, "ok");
    // Fast path: resolve identity ONLY, render, return. The adapters run when
    // the browser asks for /entity/body.
    if q.full.as_deref() != Some("1") && q.offset.is_none() {
        let spine = spine::resolve(&app.cfg, selector.trim()).await;
        return Html(render::entity_shell(&app.cfg.site_label, &spine)).into_response();
    }
    let view = view::build(&app.cfg, selector.trim()).await;
    if q.full.as_deref() == Some("1") || q.offset.is_some() {
        return Html(render::entity_paged(
            &app.cfg.site_label,
            &view,
            q.offset.unwrap_or(0),
            render::DEFAULT_PAGE,
        ))
        .into_response();
    }
    Html(render::entity_paged(&app.cfg.site_label, &view, 0, render::DEFAULT_PAGE)).into_response()
}

/// The reachability graph for one entity — same gate, same audit as the
/// timeline. A second VIEW of the same subject is the same disclosure, so it
/// must not be reachable on weaker terms than the page it sits beside.
/// The deferred half of the entity page — timeline + sources. Same gate, same
/// audit: a second VIEW of the same subject is the same disclosure.
async fn entity_body_route(
    State(app): State<App>,
    Query(q): Query<EntityQuery>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let selector = q.id.unwrap_or_default();
    let (viewer, admit) = authenticate(&app.cfg, &headers).await;
    if !admit {
        audit_look(&app.cfg, &viewer, &selector, viewer.via, "refused");
        return (StatusCode::FORBIDDEN, Html(String::from("<p>forbidden</p>"))).into_response();
    }
    let view = view::build(&app.cfg, selector.trim()).await;
    let off = q.offset.unwrap_or(0);
    Html(render::entity_body_public(&view, off, render::DEFAULT_PAGE)).into_response()
}

async fn graph_page(
    State(app): State<App>,
    Query(q): Query<EntityQuery>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let selector = q.id.unwrap_or_default();
    let (viewer, admit) = authenticate(&app.cfg, &headers).await;
    if !admit {
        audit_look(&app.cfg, &viewer, &selector, viewer.via, "refused");
        return (
            StatusCode::FORBIDDEN,
            Html(render::forbidden(&app.cfg.site_label, &viewer.groups.join(","))),
        )
            .into_response();
    }
    if selector.trim().is_empty() {
        return Html(render::home(&app.cfg.site_label)).into_response();
    }
    audit_look(&app.cfg, &viewer, selector.trim(), viewer.via, "ok");
    let spine = spine::resolve(&app.cfg, selector.trim()).await;
    // Minimal by default. One machine if named; the whole estate only when
    // explicitly asked for; otherwise a picker.
    let focus = match (q.peer.as_deref().filter(|s| !s.is_empty()), q.all.as_deref()) {
        (Some(p), _) => graph::Focus::Peer(p),
        (None, Some("1")) => graph::Focus::All,
        _ => match spine.netbird_peers.as_slice() {
            // Exactly one own device: no ambiguity, so drawing it is not a guess.
            [only] => graph::Focus::Peer(only.as_str()),
            _ => graph::Focus::None,
        },
    };
    let g = graph::build(&app.cfg, &spine, focus).await;
    Html(render::graph_page(&app.cfg.site_label, selector.trim(), &g, &spine, q.peer.as_deref(), q.all.as_deref() == Some("1"))).into_response()
}

async fn entity_api(
    State(app): State<App>,
    Query(q): Query<EntityQuery>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let selector = q.id.unwrap_or_default();
    let (viewer, admit) = authenticate(&app.cfg, &headers).await;
    if !admit {
        audit_look(&app.cfg, &viewer, &selector, "api", "refused");
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "identity-administration groups or a valid operator key required"})),
        )
            .into_response();
    }
    let selector = selector.trim().to_string();
    if selector.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "id required (email | handle | anchor | alias)"})),
        )
            .into_response();
    }
    audit_look(&app.cfg, &viewer, &selector, "api", "ok");
    let view = view::build(&app.cfg, &selector).await;
    Json(view).into_response()
}


#[derive(Deserialize)]
struct SuggestQuery {
    q: Option<String>,
    limit: Option<usize>,
}

/// Typeahead for the search box: which selectors on this deployment contain
/// what the reader has typed so far — at the start, in the middle, or at the
/// end of the string.
///
/// SAME GATE AS THE PAGES. The candidate list is a roster of who exists,
/// which is a disclosure in its own right, so an un-admitted caller gets the
/// same 403 the entity page gives and the refusal is audited.
///
/// AUDITED, BUT AS ITS OWN SURFACE. Each query lands in the view-audit as
/// `surface=suggest` with the typed prefix as the selector. It is deliberately
/// NOT recorded as a look at a person: nobody was examined, and collapsing the
/// two would let a real look hide among keystrokes. The client debounces, so a
/// search costs a few rows rather than one per character.
///
/// The response separates "nothing matched" from "we could not read the
/// roster" (`roster_read` + `notes`); the UI renders the second as unknown,
/// never as absence.
async fn suggest_api(
    State(app): State<App>,
    Query(q): Query<SuggestQuery>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let query = q.q.unwrap_or_default();
    let (viewer, admit) = authenticate(&app.cfg, &headers).await;
    if !admit {
        audit_look(&app.cfg, &viewer, query.trim(), "suggest", "refused");
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "identity-administration groups or a valid operator key required"})),
        )
            .into_response();
    }
    let query = query.trim().to_string();
    if query.is_empty() {
        return Json(serde_json::json!({
            "q": query,
            "matches": [],
            "roster_read": true,
            "notes": [],
        }))
        .into_response();
    }
    audit_look(&app.cfg, &viewer, &query, "suggest", "ok");
    let snap = app.sug.get(&app.cfg).await;
    let limit = q.limit.unwrap_or(10).clamp(1, 25);
    let matches = suggest::rank(&snap.entries, &query, limit);
    Json(serde_json::json!({
        "q": query,
        "matches": matches,
        "roster_read": snap.roster_read,
        "candidates": snap.entries.len(),
        "notes": snap.notes,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct NetvisQuery {
    /// `policy` (default) | `fabric`. Every other view-state parameter in the
    /// contract (`focus`, `open`, `family`, `find`, `q`, `filter`, `hub`,
    /// `diff`, `z`, `x`, `y`) is client-side and deliberately not parsed here
    /// — the server's job is to serve the same bytes for any of them, so a
    /// deep link is a pure client concern and cannot 400.
    alt: Option<String>,
}





#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_now_shape() {
        let s = chrono_free_now();
        assert_eq!(s.len(), 20, "{s}");
        assert!(s.ends_with('Z') && s.contains('T'));
    }

    #[test]
    fn enforce_fails_closed_on_missing_groups() {
        let mut cfg = Config::default();
        cfg.rbac_mode = "enforce".into();
        cfg.rbac_who_groups = vec!["PLATFORM-ADMIN".into(), "IAM-ADMIN".into()];
        let nobody = Viewer { email: String::new(), groups: vec![], via: "web" };
        assert!(!admitted(&cfg, &nobody));
        let data_all = Viewer { email: "x@y".into(), groups: vec!["data.view.all".into()], via: "web" };
        assert!(!admitted(&cfg, &data_all));
        let admin = Viewer { email: "x@y".into(), groups: vec!["PLATFORM-ADMIN".into()], via: "web" };
        assert!(admitted(&cfg, &admin));
        // Case-sensitive: lowercase does not admit.
        let lower = Viewer { email: "x@y".into(), groups: vec!["admins".into()], via: "web" };
        assert!(!admitted(&cfg, &lower));
    }
}
