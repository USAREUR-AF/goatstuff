//! netvis — mesh reachability, as its own service.
//!
//! Design: `docs/design/netvis.md`.
//!
//! ## Why this is a service and not a page in microscope
//!
//! It started as a route inside microscope, which meant it had no name of its
//! own, no isitup row, and nothing linking to it — you had to already know the
//! URL existed. More importantly the two answer different questions about
//! different subjects: microscope is *one entity, every layer*; netvis is *the
//! whole goatnet's reachability*. Sharing a binary made the fence, the pin and
//! the release cadence of one the fence, pin and cadence of the other.
//!
//! What is deliberately NOT duplicated is the resolver:
//! `panoscope_core::meshgraph` computes what the policy set permits, and both
//! surfaces read it. Two implementations of "what can this reach" is how two
//! surfaces come to disagree about the same goatnet.
//!
//! ## Three levels, one matrix
//!
//! L1 group→group is the default because it is the only level that survives
//! the scope envelope (ADR 0215: up to 6000 peers fleet-wide). Peer×peer over
//! a whole goatnet is 36M cells at that size and is therefore never a level —
//! it exists only bounded by a group pair (L2), and a single pair's rules are
//! L3.
//!
//! ## Honesty
//!
//! Inherited wholesale from the resolver and not weakened here: an unread
//! source is never rendered as an empty policy set. On the NetBird plane
//! `GET /api/policies` is complete, so an absent cell really is a denial —
//! but only while every read outcome is stated, which is why the payload's
//! honesty header is non-omittable and the client refuses to draw a matrix
//! when `policies_read` is false.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use panoscope_core::meshgraph::netvis as model;
use panoscope_core::netbird::NetbirdClient;
use panoscope_core::peerowner::{OwnerResolver, PeerOwner};

mod api;
mod config;
mod doctrine;

use config::Config;

const TOKENS_CSS: &str = include_str!("../../../docs/design-system/colors_and_type.css");
const NETVIS_CSS: &str = include_str!("assets/netvis.css");
const IDENTITY_JS: &str = include_str!("assets/identity.js");
const MATRIX_JS: &str = include_str!("assets/matrix.js");

#[derive(Clone)]
struct App {
    cfg: Arc<Config>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = Arc::new(Config::from_env());
    let listen: SocketAddr = cfg.listen.parse()?;

    let app = Router::new()
        .route("/", get(page))
        // THREE VIEWS, one host, one payload builder.
        //   /          populated — what the mesh permits between machines
        //   /policy    UNPOPULATED — groups and rules as designed, independent
        //              of whether anything is enrolled. This altitude existed
        //              in the resolver with tests from the start and was
        //              unreachable: main.rs hardcoded Fabric and offered no
        //              switch, so the second of the two altitudes that were
        //              asked for was built and could not be looked at.
        //   /findings  the surface's own judgement — computed on every request
        //              and, until now, discarded before it reached a pixel.
        .route("/policy", get(page))
        .route("/findings", get(page))
        //   /armed     what switches on SILENTLY the moment someone enrols.
        //              A rule with an empty endpoint is evaluated and grants
        //              nothing — until it grants everything it always said it
        //              would, with no deploy, no review and no announcement.
        //              The ratio of armed-but-asleep to load-bearing is the
        //              number that says whether the machinery has outrun the
        //              deployment, and nothing reported it.
        .route("/armed", get(page))
        //   /assertions  THE GATE, recomputed against LIVE state on every
        //              read. The same assertions the PR gate runs against the
        //              declaration — one evaluator, so a verdict cannot mean
        //              two things depending on where it ran. This surface is
        //              the half nobody in the category has: a write-time gate
        //              fires when the POLICY changes, and the fault we care
        //              about is a MEMBERSHIP change, when nothing is written.
        .route("/assertions", get(page))
        .route("/api/reach", get(api))
        .route("/api/assertions", get(assertions))
        // Curated answers. See api.rs for why these are a superset of the
        // NetBird API rather than a mirror of it.
        .route("/api/peer/:sel/reach", get(peer_reach))
        .route("/api/pair/:a/:b", get(pair))
        .route("/api/peers", get(peer_list))
        .route("/healthz", get(healthz))
        .route("/assets/:rev/*path", get(asset))
        .with_state(App { cfg: cfg.clone() });

    tracing::info!(%listen, site = %cfg.site_label, "netvis listening");
    let l = tokio::net::TcpListener::bind(listen).await?;
    axum::serve(l, app).await?;
    Ok(())
}

/// The shell. All the drawing is in the client, fed by `/api/reach` — the same
/// separation microscope uses, and the reason a previous attempt failed: its
/// picture lived in Rust `format!` strings and became a document.
async fn page(State(app): State<App>, headers: HeaderMap) -> Response {
    match admit(&app.cfg, &headers) {
        Admission::Ok => {}
        other => return refusal_page(&app.cfg, other),
    }
    let rev = rev();
    Html(format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>netvis · {site}</title>
<link rel="stylesheet" href="/assets/{rev}/tokens.css">
<link rel="stylesheet" href="/assets/{rev}/netvis.css">
</head><body class="netvis"><div id="reach-root"></div>
<script src="/assets/{rev}/identity.js"></script>
<script src="/assets/{rev}/matrix.js"></script>
</body></html>"#,
        site = esc(&app.cfg.site_label),
        rev = rev
    ))
    .into_response()
}

/// The payload. One read, one build, one shape — see the module note on why
/// the honesty header is non-omittable.
async fn api(
    State(app): State<App>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    if let Some(r) = api_gate(&app.cfg, &headers) {
        return r;
    }
    let alt = match q.get("alt").map(String::as_str) {
        Some("policy") => model::Altitude::Policy,
        _ => model::Altitude::Fabric,
    };
    match build_doc_at(&app, alt).await {
        Ok(d) => axum::Json(d).into_response(),
        Err(r) => r,
    }
}

/// ONE read, ONE build, shared by the visualization payload and every curated
/// answer. Two builders would eventually disagree, and a surface whose picture
/// and whose API disagree about the same goatnet is worse than either alone.
/// Annotate each rule with the warrant registry's view of it (ADR 1129).
///
/// The live plane knows a rule permits `tcp 443,53,5353,7447`; only the
/// registry knows that shape is the `substrate` set, what it is for, who owns
/// it, and whether the grant expires. `expires` / `grant_owner` are the point:
/// `generate.py` strips them before the ACL is applied, so they exist NOWHERE
/// on the wire, and this is the only surface that can show them.
///
/// Costs no read and no request-time work beyond two map lookups — the
/// registry is compiled in and parsed once (ADR 1123).
fn decorate_warrant(mut d: model::Doc) -> model::Doc {
    let (vocab, intents) = warrant_registry();
    for r in &mut d.rules {
        if let Some(set) = vocab.resolve(&r.protocol, &r.ports) {
            r.port_set = Some(set.name.clone());
            r.port_set_purpose = Some(set.purpose.trim().to_string());
            r.port_set_owner = Some(set.owner.clone());
        }
        if let Some(meta) = intents.get(r.rule_name.as_str()) {
            r.expires = meta.expires.clone();
            r.grant_owner = meta.owner.clone();
        }
    }
    d
}

/// The registry, compiled in and parsed once. Not in `panoscope-core`: that
/// crate is linked by microscope, stethoscope, panoscope and flashlight, and
/// none of them renders a port-set.
fn warrant_registry() -> &'static (
    panoscope_core::portsets::Vocabulary,
    panoscope_core::portsets::IntentIndex,
) {
    static CELL: std::sync::OnceLock<(
        panoscope_core::portsets::Vocabulary,
        panoscope_core::portsets::IntentIndex,
    )> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        // A registry that fails to parse degrades to no annotation. CI
        // (ops/ci/check-warrant-registry.py) refuses a broken registry; a
        // rendering surface must not go dark over a defect it did not cause.
        (
            panoscope_core::portsets::Vocabulary::parse(include_str!(
                "../../../ops/warrant/port-sets.yaml"
            ))
            .unwrap_or_default(),
            panoscope_core::portsets::IntentIndex::parse(include_str!(
                "../../../ops/warrant/rule-intents/netbird-acl.yaml"
            )),
        )
    })
}

/// Liveness that means something. The constant "ok" this used to return was
/// true for three days in September 2026 while every management-API call
/// came back 401 and the page drew nothing: the process was up, the surface
/// was dead, and no probe could tell the difference. Now the probe is told:
/// 503 with the refusal when the API that this whole page draws from will
/// not answer. One small GET per probe (30 s), against NetBird, not the fabric.
async fn healthz(State(app): State<App>) -> Response {
    let nb = NetbirdClient::new(app.cfg.netbird_url.clone(), app.cfg.netbird_token.clone());
    match tokio::time::timeout(std::time::Duration::from_secs(5), nb.list_groups()).await {
        Ok(Ok(_)) => (StatusCode::OK, "ok").into_response(),
        Ok(Err(e)) => (StatusCode::SERVICE_UNAVAILABLE, format!("netbird management API unreadable: {e}")).into_response(),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "netbird management API unreadable: timeout after 5s".to_string()).into_response(),
    }
}

async fn build_doc(app: &App) -> Result<model::Doc, Response> {
    build_doc_at(app, model::Altitude::Fabric).await
}

/// Same read, same shape, one flag. `Altitude::Policy` omits the peer-derived
/// fields (they are `Option` + `skip_serializing_if`), so the payload is the
/// same `Doc` with five fields absent — the client keys off `alt`.
async fn build_doc_at(app: &App, alt: model::Altitude) -> Result<model::Doc, Response> {
    let nb = NetbirdClient::new(app.cfg.netbird_url.clone(), app.cfg.netbird_token.clone());
    let mut notes: Vec<String> = Vec::new();

    let groups = match nb.list_groups().await {
        Ok(g) => Some(g),
        Err(e) => { notes.push(format!("groups unread: {e}")); None }
    };
    let policies = match nb.list_policies().await {
        Ok(p) => Some(p),
        Err(e) => {
            notes.push(format!(
                "policies unread ({e}) — an absent path is an UNKNOWN, not a denial"
            ));
            None
        }
    };
    let peers = match nb.list_peers().await {
        Ok(p) => Some(p),
        Err(e) => { notes.push(format!("peers unread: {e} — counts are unknown, not zero")); None }
    };

    let doc = doctrine::load(&app.cfg.site_label);
    let generated = chrono::Utc::now().to_rfc3339();
    let declared = doc.as_ref().map(|d| d.declared_suffixes()).unwrap_or_default();
    let purposes = doc.as_ref().map(|d| d.rule_purpose.clone()).unwrap_or_default();

    let mut req = model::Request::new(&app.cfg.site_label, alt, &generated);
    req.canvas_w = 1100;
    req.site_prefix = doc.as_ref().map(|d| d.site_prefix.as_str()).unwrap_or("");
    req.groups = groups.as_deref();
    req.policies = policies.as_deref();
    req.peers = peers.as_deref();
    req.notes = &notes;
    req.declared = &declared;
    req.rule_purpose = &purposes;

    // ONE decomposition, filled here so every consumer of the doc — the
    // matrix, the identity card, /api/peers, /api/pair — reads the same
    // answer. The client deriving it from an ontology table would be the
    // second implementation, and the peer-name grammar has already shown what
    // that costs.
    let mut built = model::build(&req).map(decorate_warrant).map(|mut d| {
        let onto = doc.as_ref().map(|x| x.gear_ontology.clone()).unwrap_or_default();
        if !onto.is_empty() {
            if let Some(ps) = d.peers.clone() {
                for (i, _) in ps.iter().enumerate() {
                    let fams = api::gear_families(&d, i, &onto);
                    if !fams.is_empty() {
                        if let Some(pm) = d.peers.as_mut().and_then(|v| v.get_mut(i)) {
                            pm.gear = serde_json::to_value(&fams).unwrap_or(serde_json::Value::Null);
                        }
                    }
                }
            }
        }
        d
    });
    let _ = &mut built;
    built.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({
                "error": "netvis could not build a consistent view",
                "reason": format!("{e:?}"),
                "note": "A build fault, not a claim that nothing is permitted.",
            })),
        )
            .into_response()
    })
}

/// GET /api/assertions — what the declaration asserts about itself, checked
/// against what is live right now.
///
/// Costs no extra read: it is pure computation over the document the matrix
/// already built, which is what keeps it inside ADR 1123's rule that client
/// demand must never amplify into fabric load.
async fn assertions(State(app): State<App>, headers: HeaderMap) -> Response {
    if let Some(r) = api_gate(&app.cfg, &headers) {
        return r;
    }
    let d = match build_doc(&app).await { Ok(d) => d, Err(r) => return r };
    let site = app.cfg.site_label.trim().to_lowercase();
    let prefix = doctrine::load(&site).map(|x| x.site_prefix).unwrap_or_default();
    // A goatnet netvis serves is a persistent site, never an exercise scope,
    // so it has no host scope. Passing one would let `HOST:` resolve to
    // itself, which is the quiet self-reference the parity checker refuses.
    let model = panoscope_core::aclcheck::Model::from_doc(&d, &prefix, None);
    let tests = doctrine::declaration().map(|x| x.tests(&site)).unwrap_or_default();
    let report = panoscope_core::aclcheck::evaluate(&model, &tests);
    axum::Json(serde_json::json!({
        "provenance": api::Provenance::of(&d),
        "declared_tests": tests.len(),
        "report": report,
    }))
    .into_response()
}

/// Resolve "who is responsible for this machine" for a set of peers.
///
/// The question a viewer has once the matrix has told them WHICH machine: a
/// reachability answer they need to act on is useless without someone to ask.
/// See `panoscope_core::peerowner` for why this is a name-derived lookup and
/// why the answer carries its own provenance.
///
/// ONE batch call for the whole set, never one per peer — that is what the
/// portal's batch endpoint exists for (it assembles the access graph once).
/// Always returns a row for every name asked about, so a caller never has to
/// branch on a missing key; when the lens is off or the portal is down every
/// row is `unread`, which is a statement, not a blank.
async fn resolve_owners(
    app: &App,
    names: &[String],
) -> (std::collections::BTreeMap<String, PeerOwner>, Vec<String>) {
    match OwnerResolver::new(&app.cfg.portal_url, &app.cfg.portal_api_key) {
        Some(r) => r.resolve(names).await,
        None => {
            let mut out = std::collections::BTreeMap::new();
            for n in names {
                out.insert(
                    n.clone(),
                    PeerOwner::unread(n, "ownership lens not configured on this goatnet (NETVIS_PORTAL_URL / NETVIS_PORTAL_API_KEY unset)"),
                );
            }
            // Deliberately NOT a warning: a goatnet that has not wired the lens
            // is not in a fault state. It is a capability that is off, and the
            // note says so in the words an operator would need to turn it on.
            (out, vec!["ownership lens off — set NETVIS_PORTAL_URL + NETVIS_PORTAL_API_KEY to resolve machines to the person responsible".to_string()])
        }
    }
}

/// GET /api/peers — the roster with derived identity, so a caller can resolve
/// a human-typed name to something stable before asking a reachability
/// question. Carries the ownership lens: every row says who is responsible for
/// the machine and HOW that was established.
async fn peer_list(State(app): State<App>, headers: HeaderMap) -> Response {
    if let Some(r) = api_gate(&app.cfg, &headers) {
        return r;
    }
    let d = match build_doc(&app).await { Ok(d) => d, Err(r) => return r };
    let peers = d.peers.clone().unwrap_or_default();
    let names: Vec<String> = peers.iter().map(|p| p.name.clone()).collect();
    let (owners, owner_notes) = resolve_owners(&app, &names).await;
    // Precomputed: `json!` takes expressions, not blocks, so the fold cannot be
    // inlined as a value. Indexed by the peer's own `i`, which is what
    // `stewardship` keys on.
    // The ontology comes from the compiled-in declaration, loaded once per
    // request rather than per peer — it is the same table for every row.
    let dct = doctrine::load(&app.cfg.site_label);
    let onto = dct.as_ref().map(|d| d.gear_ontology.clone()).unwrap_or_default();
    // Which goatnet hosts each site rung, so an unappointed scope-local admin
    // reads as "authority sits over there" rather than as a gap.
    let hosts = dct.as_ref().map(|d| d.scope_host_prefix.clone()).unwrap_or_default();
    let gear: Vec<Vec<api::GearFamily>> = peers
        .iter()
        .map(|p| api::gear_families(&d, p.i, &onto))
        .collect();
    let stew: Vec<api::Stewardship> = peers
        .iter()
        .map(|p| {
            let mut st = api::stewardship(&d, p.i, &hosts);
            api::fold_admin_principals(&mut st, &owners);
            st
        })
        .collect();
    axum::Json(serde_json::json!({
        "provenance": api::Provenance::of(&d),
        "ownership_notes": owner_notes,
        "peers": peers.iter().map(|p| serde_json::json!({
            "netbird_name": p.name,
            "id": p.id,
            "ip": p.ip,
            "connected": p.connected,
            "os": p.os,
            "groups": p.groups.iter().filter_map(|g| d.nodes.get(*g)).map(|n| n.name.clone()).collect::<Vec<_>>(),
            "owner": owners.get(&p.name),
            // Who to ask when the name resolves to nobody. Mesh-derived, a
            // weaker claim than `owner`, and carried separately for that reason.
            "stewardship": stew.get(p.i),
            // ADR 1116 §3: the machine's classes as families with slots, not
            // as N unrelated suffixes. Empty for anything holding no gear
            // class — most peers are laptops and infra.
            "gear": gear.get(p.i).filter(|g| !g.is_empty()),
        })).collect::<Vec<_>>(),
    }))
    .into_response()
}

/// GET /api/peer/:sel/reach — who can this machine talk to, and why.
///
/// The question the NetBird API cannot answer in one call: it requires
/// resolving this peer's groups, every rule those groups source, and then
/// every peer in the destination groups.
async fn peer_reach(
    State(app): State<App>,
    axum::extract::Path(sel): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(r) = api_gate(&app.cfg, &headers) {
        return r;
    }
    let d = match build_doc(&app).await { Ok(d) => d, Err(r) => return r };
    let a = match api::find_peer(&d, &sel) {
        Ok(i) => i,
        Err(e) => return (StatusCode::NOT_FOUND, axum::Json(e)).into_response(),
    };
    let peers = d.peers.clone().unwrap_or_default();
    let mut out = Vec::new();
    for b in 0..peers.len() {
        if b == a { continue; }
        let ps = api::paths(&d, a, b);
        if ps.is_empty() { continue; }
        out.push(serde_json::json!({
            "peer": peers[b].name,
            "ip": peers[b].ip,
            "connected": peers[b].connected,
            "both_ends_connected": peers[a].connected && peers[b].connected,
            "any_port": ps.iter().any(|p| p["unrestricted"] == serde_json::Value::Bool(true)),
            "paths": ps,
        }));
    }
    // The subject only. Every machine it reaches is a separate question, and
    // resolving all of them here would turn one page view into a roster-wide
    // PII read for a question nobody asked.
    let (owners, owner_notes) = resolve_owners(&app, &[peers[a].name.clone()]).await;
    let hosts_a = doctrine::load(&app.cfg.site_label)
        .map(|x| x.scope_host_prefix)
        .unwrap_or_default();
    let mut st_a = api::stewardship(&d, a, &hosts_a);
    api::fold_admin_principals(&mut st_a, &owners);
    axum::Json(serde_json::json!({
        "provenance": api::Provenance::of(&d),
        "peer": peers[a].name,
        "owner": owners.get(&peers[a].name),
        "stewardship": st_a,
        "ownership_notes": owner_notes,
        "reaches_count": out.len(),
        "of_peers": peers.len().saturating_sub(1),
        "reaches": out,
    }))
    .into_response()
}

/// GET /api/pair/:a/:b — can these two talk, on what, and what permits it.
async fn pair(
    State(app): State<App>,
    axum::extract::Path((a, b)): axum::extract::Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Some(r) = api_gate(&app.cfg, &headers) {
        return r;
    }
    let d = match build_doc(&app).await { Ok(d) => d, Err(r) => return r };
    let (ia, ib) = match (api::find_peer(&d, &a), api::find_peer(&d, &b)) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(e), _) | (_, Err(e)) => return (StatusCode::NOT_FOUND, axum::Json(e)).into_response(),
    };
    let peers = d.peers.clone().unwrap_or_default();
    let fwd = api::paths(&d, ia, ib);
    let rev = api::paths(&d, ib, ia);
    // The two ends PLUS the scope admins, in ONE batch. The admins have to be
    // resolved too — otherwise the stewardship row lists devices rather than
    // people — and asking for them separately would be a second graph assembly
    // for an answer about the same pane.
    let mut want = vec![peers[ia].name.clone(), peers[ib].name.clone()];
    let dct = doctrine::load(&app.cfg.site_label);
    let hosts = dct.as_ref().map(|x| x.scope_host_prefix.clone()).unwrap_or_default();
    let (sa, sb) = (api::stewardship(&d, ia, &hosts), api::stewardship(&d, ib, &hosts));
    for st in [&sa, &sb] {
        for a in &st.admins {
            if !want.contains(&a.netbird_name) {
                want.push(a.netbird_name.clone());
            }
        }
    }
    let (owners, owner_notes) = resolve_owners(&app, &want).await;
    let onto = dct.as_ref().map(|x| x.gear_ontology.clone()).unwrap_or_default();
    let ga = Some(api::gear_families(&d, ia, &onto)).filter(|g| !g.is_empty());
    let gb = Some(api::gear_families(&d, ib, &onto)).filter(|g| !g.is_empty());
    let (mut sa, mut sb) = (sa, sb);
    api::fold_admin_principals(&mut sa, &owners);
    api::fold_admin_principals(&mut sb, &owners);
    axum::Json(serde_json::json!({
        "provenance": api::Provenance::of(&d),
        "source": peers[ia].name,
        "destination": peers[ib].name,
        // Direction is answered explicitly in both directions. NetBird rules
        // are overwhelmingly one-way, so "can they talk"
        // without a direction is a question with two different answers.
        "source_to_destination": { "permitted": !fwd.is_empty(), "paths": fwd },
        "destination_to_source": { "permitted": !rev.is_empty(), "paths": rev },
        "both_ends_connected": peers[ia].connected && peers[ib].connected,
        // The pane a viewer lands on with a specific pair in hand — level=p in
        // the URL — is exactly where "who do I ask about this" belongs. Both
        // ends, because a one-way grant is a question for the source's owner
        // and the destination's owner in different ways.
        "ownership_notes": owner_notes,
        "id_console_base": app.cfg.id_console_url,
        "source_owner": owners.get(&peers[ia].name),
        "destination_owner": owners.get(&peers[ib].name),
        "source_gear": ga,
        "destination_gear": gb,
        "source_stewardship": sa,
        "destination_stewardship": sb,
    }))
    .into_response()
}

/// Content-addressed assets. FNV-1a over every embedded byte, so a rebuild
/// changes the URL and a year-long `immutable` can never serve a stale file.
fn rev() -> &'static str {
    use std::sync::OnceLock;
    static REV: OnceLock<String> = OnceLock::new();
    REV.get_or_init(|| {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in TOKENS_CSS
            .bytes()
            .chain(NETVIS_CSS.bytes())
            .chain(IDENTITY_JS.bytes())
            .chain(MATRIX_JS.bytes())
        {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{h:016x}")
    })
}

async fn asset(axum::extract::Path((_rev, path)): axum::extract::Path<(String, String)>) -> Response {
    let hit = match path.as_str() {
        "tokens.css" => Some(("text/css; charset=utf-8", TOKENS_CSS)),
        "netvis.css" => Some(("text/css; charset=utf-8", NETVIS_CSS)),
        "identity.js" => Some(("text/javascript; charset=utf-8", IDENTITY_JS)),
        "matrix.js" => Some(("text/javascript; charset=utf-8", MATRIX_JS)),
        _ => None,
    };
    match hit {
        Some((ct, body)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, HeaderValue::from_static(ct)),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=31536000, immutable"),
                ),
                (
                    header::X_CONTENT_TYPE_OPTIONS,
                    HeaderValue::from_static("nosniff"),
                ),
            ],
            body,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "no such asset").into_response(),
    }
}

/// The header the fence reads. `X-Forwarded-Groups`, and ONLY that.
///
/// This was `x-auth-request-groups` and admitted nobody: the sidecar ships
/// `OAUTH2_PROXY_SET_XAUTHREQUEST=false` and `PASS_USER_HEADERS=true`, so the
/// X-Auth-Request family is never populated and every logged-in
/// operator got the refusal page. It read as a working fail-closed fence
/// because a fence that admits nobody and a fence that is wired to the wrong
/// header are indistinguishable from outside — see `groups_header_is_the one
/// the proxy actually sets` below, which is the test that tells them apart.
const GROUPS_HEADER: &str = "x-forwarded-groups";

/// What the fence decided, and enough to say why. A bare "denied" left the
/// operator with no way to tell a missing group from a missing header.
pub enum Admission {
    Ok,
    /// Proxy set no groups header at all — the app is exposed without its
    /// sidecar, or the sidecar is misconfigured. Not the viewer's fault.
    NoHeader,
    /// Identity arrived and carries none of the permitted groups.
    NotPermitted { presented: String },
}

/// The fence. oauth2-proxy terminates identity in front and forwards the
/// groups claim; this checks it. Default-deny when the header is absent, so a
/// misconfigured proxy fails closed rather than serving the whole ACL to
/// anyone who can reach the port.
fn admit(cfg: &Config, headers: &HeaderMap) -> Admission {
    if cfg.rbac_mode == "off" {
        return Admission::Ok;
    }
    let raw = headers
        .get(GROUPS_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if raw.is_empty() {
        return Admission::NoHeader;
    }
    let allow: Vec<&str> = cfg.rbac_groups.split(',').map(str::trim).collect();
    let ok = raw
        .split(',')
        .map(str::trim)
        .any(|g| allow.iter().any(|a| a.eq_ignore_ascii_case(g)));
    if ok {
        Admission::Ok
    } else {
        Admission::NotPermitted {
            presented: raw.to_string(),
        }
    }
}

/// The API half of the fence. Same decision as the page, JSON instead of HTML,
/// and it names the two cases separately for the same reason the page does: a
/// caller scripting against this needs to tell "I lack the group" from "this
/// service is deployed without its proxy".
fn api_gate(cfg: &Config, headers: &HeaderMap) -> Option<Response> {
    match admit(cfg, headers) {
        Admission::Ok => None,
        Admission::NoHeader => Some((
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({
                "error": "no identity reached netvis",
                "reason": format!(
                    "the sign-in proxy passed no {GROUPS_HEADER} header — a wiring \
                     fault in front of this app, not a missing permission"
                ),
                "permitted_groups": cfg.rbac_groups,
            })),
        )
            .into_response()),
        Admission::NotPermitted { presented } => Some((
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({
                "error": "not permitted",
                "permitted_groups": cfg.rbac_groups,
                "presented_groups": presented,
            })),
        )
            .into_response()),
    }
}

/// A refusal a person can act on. The first version was the string
/// "netvis is admin-gated" on a black page: it did not say which group was
/// wanted, what the viewer presented, or who to ask — so a WIRING bug (the
/// fence reading a header the proxy never sets) was indistinguishable from
/// "you lack the group", and it stayed hidden until someone screenshotted it.
fn refusal_page(cfg: &Config, why: Admission) -> Response {
    let (headline, detail) = match why {
        Admission::Ok => unreachable!("refusal_page called on an admitted request"),
        Admission::NoHeader => (
            "No identity reached netvis",
            format!(
                "The sign-in proxy did not pass a <code>{GROUPS_HEADER}</code> header. \
                 That is a wiring fault in front of this app, not a permission you are \
                 missing — reaching netvis without its proxy looks exactly like this. \
                 Nothing is served until it is fixed."
            ),
        ),
        Admission::NotPermitted { presented } => (
            "Your account is not permitted here",
            format!(
                "netvis admits <b>{}</b>. You presented <b>{}</b>.",
                esc(&cfg.rbac_groups.replace(',', ", ")),
                esc(if presented.is_empty() { "nothing" } else { &presented }),
            ),
        ),
    };
    (
        StatusCode::FORBIDDEN,
        Html(format!(
            r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>netvis · not permitted</title>
<link rel="stylesheet" href="/assets/{rev}/tokens.css">
<link rel="stylesheet" href="/assets/{rev}/netvis.css">
</head><body class="netvis"><main class="nv-refuse">
<h1>{headline}</h1><p>{detail}</p>
<p class="nv-refuse-site">netvis on <b>{site}</b></p>
</main></body></html>"#,
            rev = rev(),
            site = esc(&cfg.site_label),
        )),
    )
        .into_response()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod fence_tests {
    use super::*;

    /// The env template is the other half of this contract, and the bug lived
    /// in the gap between the two halves. Keep them in one place.
    const PROXY_ENV: &str = include_str!(
        "../../../ops/ansible/roles/netvis/templates/oauth2-proxy-netvis.env.j2"
    );

    fn cfg() -> Config {
        Config {
            listen: "0.0.0.0:9760".into(),
            site_label: "zagros".into(),
            netbird_url: String::new(),
            netbird_token: String::new(),
            rbac_mode: "enforce".into(),
            rbac_groups: "PLATFORM-ADMIN,IAM-ADMIN".into(),
            // The ownership lens OFF is the right default for a fence test:
            // these cases are about who is admitted, and a configured resolver
            // would make them depend on a portal that is not there.
            portal_url: String::new(),
            portal_api_key: String::new(),
            id_console_url: String::new(),
        }
    }

    fn admitted(cfg: &Config, headers: &HeaderMap) -> bool {
        matches!(admit(cfg, headers), Admission::Ok)
    }

    fn hdr(name: &str, value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
        h
    }

    /// THE test this crate was missing. A fence wired to a header the proxy
    /// never sets is indistinguishable from a working fail-closed fence when
    /// probed from outside: both refuse everyone. Only the sidecar config says
    /// which one you have. netvis shipped admitting NOBODY because the fence
    /// read `x-auth-request-groups` while the sidecar sets SET_XAUTHREQUEST
    /// false and PASS_USER_HEADERS true.
    #[test]
    fn groups_header_is_the_one_the_proxy_actually_sets() {
        assert!(
            PROXY_ENV.contains("OAUTH2_PROXY_PASS_USER_HEADERS=true"),
            "sidecar must pass user headers, or nothing populates {GROUPS_HEADER}"
        );
        assert!(
            PROXY_ENV.contains("OAUTH2_PROXY_SET_XAUTHREQUEST=false"),
            "the X-Auth-Request family is deliberately off; if this ever \
             flips, revisit which header the fence reads"
        );
        // PASS_USER_HEADERS populates the X-Forwarded-* family. Reading
        // anything else admits nobody.
        assert_eq!(GROUPS_HEADER, "x-forwarded-groups");
    }

    #[test]
    fn permitted_group_is_admitted() {
        assert!(admitted(
            &cfg(),
            &hdr(GROUPS_HEADER, "ZAGROS-DEVELOPER,PLATFORM-ADMIN")
        ));
    }

    #[test]
    fn unpermitted_group_is_refused_and_says_what_was_presented() {
        match admit(&cfg(), &hdr(GROUPS_HEADER, "ZAGROS-OBSERVER")) {
            Admission::NotPermitted { presented } => assert_eq!(presented, "ZAGROS-OBSERVER"),
            _ => panic!("an unpermitted group must be refused"),
        }
    }

    /// Absent header is a WIRING fault, and must not be reported as the
    /// viewer lacking a group — that conflation is what hid the bug.
    #[test]
    fn absent_header_is_distinguished_from_a_missing_group() {
        assert!(matches!(
            admit(&cfg(), &HeaderMap::new()),
            Admission::NoHeader
        ));
    }

    /// The forgeable family must never be honoured, even if something sets it.
    #[test]
    fn the_x_auth_request_family_is_never_honoured() {
        assert!(!admitted(
            &cfg(),
            &hdr("x-auth-request-groups", "PLATFORM-ADMIN")
        ));
    }
}
