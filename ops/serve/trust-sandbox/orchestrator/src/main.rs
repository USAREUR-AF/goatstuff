//! trust-sandbox orchestrator — the privileged broker for the zero-trust
//! identity playground. Two modes:
//!   init-ca --out <dir>   one-shot: mint the CA + zenohd server cert + observer leaf
//!   serve                 long-lived: the /api + /ws broker
//!
//! The teaching frame is complementary roles in one coherent strategy, not
//! user-vs-attacker: each gate answers a question none of the others can, and
//! the same machinery that contains a hostile identity also absorbs an ordinary
//! one that is noisy, stale, or mid-replacement — and lets identity + access be
//! created, rotated, and revoked quickly and on local authority.
//!
//! Design: docs/design/trust-sandbox.md

mod ca;
mod certinfo;
mod dns;
mod model;
mod oidc;
mod spire;
mod zfab;

use anyhow::{Context, Result};
use axum::{
    extract::{ws::WebSocketUpgrade, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use model::*;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::broadcast;

struct Cfg {
    bind: String,
    zenoh_endpoint: String,
    certs_dir: String,
    spire_socket: String,
    svid_dir: String,
    hydra_public_browser: String,
    coredns: String,
    zone_file: String,
    target_name: String,
    target_host: String,
    oidc_client_id: String,
    oidc_redirect: String,
}

struct AppState {
    cfg: Cfg,
    signer: ca::CaSigner,
    roster: Mutex<Vec<Identity>>,
    observer: zfab::Observer,
    oidc: oidc::Oidc,
    tx: broadcast::Sender<WsEvent>,
}

type Shared = Arc<AppState>;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("init-ca") => {
            let out = arg_value(&args, "--out").unwrap_or_else(|| "/certs".into());
            ca::init_ca(std::path::Path::new(&out)).context("init-ca")?;
            tracing::info!("init-ca: wrote CA + server cert + observer leaf to {out}");
            Ok(())
        }
        Some("serve") | None => serve().await,
        Some(other) => Err(anyhow::anyhow!("unknown command: {other}")),
    }
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// The URI-SAN identity grammar this sandbox stamps on the leaves it mints:
/// `urn:goat:<goatnet>:org=<org>,role=OPERATOR`. The goatnet label comes from
/// `TS_GOATNET` (default `goatnet`, which names no deployment).
fn operator_uri_san(org: &str) -> String {
    format!(
        "urn:goat:{}:org={org},role=OPERATOR",
        env_or("TS_GOATNET", "goatnet")
    )
}

fn default_roster() -> Vec<Identity> {
    vec![
        Identity {
            handle: "alice-admin".into(),
            kind: IdentityKind::Operator,
            anchor: None,
            role: Some("ADMIN".into()),
            uri_san: None,
            revoked: false,
            blurb: "A human operator with the ADMIN role — may issue, rotate, and revoke identities for others. Sign in as alice to mint.".into(),
        },
        Identity {
            handle: "omar-operator".into(),
            kind: IdentityKind::Operator,
            anchor: None,
            role: Some("OPERATOR".into()),
            uri_san: None,
            revoked: false,
            blurb: "A human operator with the OPERATOR role — can act, but not issue identities for others.".into(),
        },
        Identity {
            handle: "bea-observer".into(),
            kind: IdentityKind::Operator,
            anchor: None,
            role: Some("OBSERVER".into()),
            uri_san: None,
            revoked: false,
            blurb: "A human with the OBSERVER role — may look, not change. Cannot mint.".into(),
        },
        Identity {
            handle: "acme-sensors".into(),
            kind: IdentityKind::OrgSystem,
            anchor: Some("acme-sensors".into()),
            role: None,
            uri_san: Some(operator_uri_san("acme-sensors")),
            revoked: false,
            blurb: "An external organization's system — gets a portal/CA-issued certificate (we can't run software inside their environment). Publishes under its own anchor.".into(),
        },
        Identity {
            handle: "edge-processor".into(),
            kind: IdentityKind::Workload,
            anchor: Some("edge-processor".into()),
            role: None,
            uri_san: Some("spiffe://trust.sandbox/workload/edge-processor".into()),
            revoked: false,
            blurb: "A workload we run — gets a SPIRE-attested, auto-rotating SVID. Same access gate, different issuance path.".into(),
        },
        Identity {
            handle: "sensor-gw".into(),
            kind: IdentityKind::Backing,
            anchor: Some("sensor-gw".into()),
            role: None,
            uri_san: Some(operator_uri_san("sensor-gw")),
            revoked: false,
            blurb: "The disposable backing service behind the mesh name sensor-gw.trust.local — used for burn-&-respawn.".into(),
        },
    ]
}

fn role_of(roster: &[Identity], subject: &str) -> String {
    roster
        .iter()
        .find(|i| i.handle == subject)
        .and_then(|i| i.role.clone())
        .unwrap_or_else(|| "OBSERVER".into())
}

async fn serve() -> Result<()> {
    let cfg = Cfg {
        bind: env_or("TS_BIND", "0.0.0.0:8080"),
        zenoh_endpoint: env_or("TS_ZENOH_ENDPOINT", "tls/zenohd:7447"),
        certs_dir: env_or("TS_CERTS_DIR", "/certs"),
        spire_socket: env_or("TS_SPIRE_SOCKET", "unix:///run/spire/shared/agent.sock"),
        svid_dir: "/tmp/svid".into(),
        hydra_public_browser: env_or("TS_HYDRA_PUBLIC", "http://localhost:8100/hydra"),
        coredns: env_or("TS_COREDNS", "coredns:53"),
        zone_file: env_or("TS_ZONE_FILE", "/etc/coredns/db.trust.local"),
        target_name: env_or("TS_TARGET_NAME", "sensor-gw.trust.local"),
        target_host: env_or("TS_TARGET_HOST", "target"),
        oidc_client_id: env_or("TS_OIDC_CLIENT_ID", "trust-sandbox-spa"),
        oidc_redirect: env_or("TS_OIDC_REDIRECT", "http://localhost:8100/callback.html"),
    };

    let signer = ca::CaSigner::load(std::path::Path::new(&cfg.certs_dir))
        .context("load CA (did init-ca run?)")?;

    let (tx, _rx) = broadcast::channel::<WsEvent>(512);

    // The observer is load-bearing; retry until zenohd is reachable.
    let observer = loop {
        match zfab::start_observer(&cfg.zenoh_endpoint, &cfg.certs_dir, tx.clone()).await {
            Ok(o) => break o,
            Err(e) => {
                tracing::warn!("observer not up yet ({e}); retrying in 2s");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    };
    tracing::info!("observer session live on {}", cfg.zenoh_endpoint);

    let oidc = oidc::Oidc::new(
        &env_or("TS_HYDRA_ADMIN", "http://hydra:4445"),
        &env_or("TS_HYDRA_PUBLIC_INTERNAL", "http://hydra:4444"),
        &cfg.hydra_public_browser,
        &cfg.oidc_client_id,
    );

    let state: Shared = Arc::new(AppState {
        cfg,
        signer,
        roster: Mutex::new(default_roster()),
        observer,
        oidc,
        tx: tx.clone(),
    });

    // Periodic health heartbeat on the bus.
    {
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                let h = health_snapshot(&st).await;
                let _ = st.tx.send(h);
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });
    }

    let app = Router::new()
        .route("/api/health", get(h_health))
        .route("/api/roster", get(h_roster))
        .route("/api/config", get(h_config))
        .route("/api/spire/svid", get(h_svid))
        .route("/api/attempt", post(h_attempt))
        .route("/api/mint", post(h_mint))
        .route("/api/revoke", post(h_revoke))
        .route("/api/oidc/login", post(h_login))
        .route("/api/oidc/consent", post(h_consent))
        .route("/api/dns/resolve", post(h_resolve))
        .route("/api/dns/burn", post(h_burn))
        .route("/ws", get(h_ws))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind(&state.cfg.bind).await?;
    tracing::info!("orchestrator listening on {}", state.cfg.bind);
    axum::serve(listener, app).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Health
async fn health_snapshot(st: &Shared) -> WsEvent {
    let spire_ok = spire::available(&st.cfg.spire_socket, &st.cfg.svid_dir);
    let hydra_ok = reqwest::Client::new()
        .get(format!(
            "{}/health/ready",
            env_or("TS_HYDRA_PUBLIC_INTERNAL", "http://hydra:4444")
        ))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    WsEvent::Health {
        zenoh: true,
        spire: spire_ok,
        hydra: hydra_ok,
        at: chrono::Utc::now().timestamp_millis(),
    }
}

async fn h_health(State(st): State<Shared>) -> impl IntoResponse {
    match health_snapshot(&st).await {
        WsEvent::Health { zenoh, spire, hydra, at } => {
            Json(serde_json::json!({ "zenoh": zenoh, "spire": spire, "hydra": hydra, "at": at }))
        }
        _ => Json(serde_json::json!({})),
    }
}

async fn h_roster(State(st): State<Shared>) -> impl IntoResponse {
    let r = st.roster.lock().unwrap().clone();
    Json(r)
}

async fn h_config(State(st): State<Shared>) -> impl IntoResponse {
    Json(serde_json::json!({
        "issuer": st.cfg.hydra_public_browser,
        "client_id": st.cfg.oidc_client_id,
        "redirect": st.cfg.oidc_redirect,
        "target_name": st.cfg.target_name,
    }))
}

async fn h_svid(State(st): State<Shared>) -> impl IntoResponse {
    match spire::fetch_svid(&st.cfg.spire_socket, &st.cfg.svid_dir) {
        Ok(s) => {
            let ttl = s.info.not_after_unix - chrono::Utc::now().timestamp();
            Json(serde_json::json!({
                "ok": true,
                "spiffe_id": s.info.uri_sans.first().cloned().unwrap_or_default(),
                "cn": s.info.cn,
                "serial": s.info.serial,
                "not_after_unix": s.info.not_after_unix,
                "ttl_seconds": ttl.max(0),
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// The attempt — the heart of the knockout experiment.
fn gate(g: &str, state: GateState, role: &str, detail: &str, live: bool) -> GateVerdict {
    GateVerdict {
        gate: g.into(),
        state,
        role: role.into(),
        detail: detail.into(),
        live,
    }
}

fn nonce() -> String {
    format!(
        "n{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    )
}

async fn h_attempt(State(st): State<Shared>, Json(req): Json<AttemptReq>) -> impl IntoResponse {
    let identity = {
        let r = st.roster.lock().unwrap();
        match r.iter().find(|i| i.handle == req.identity).cloned() {
            Some(i) => i,
            None => {
                return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error":"unknown identity"}))).into_response()
            }
        }
    };
    let resp = run_attempt(&st, &identity, &req).await;
    let _ = st.tx.send(WsEvent::Attempt {
        identity: resp.identity.clone(),
        summary: resp.summary.clone(),
        silently_denied: resp.silently_denied,
        at: chrono::Utc::now().timestamp_millis(),
    });
    Json(resp).into_response()
}

async fn run_attempt(st: &Shared, id: &Identity, req: &AttemptReq) -> AttemptResp {
    let cond = req.condition;
    let anchor = id.anchor.clone().unwrap_or_else(|| id.handle.clone());
    let root_ca = format!("{}/ca-root.pem", st.cfg.certs_dir);

    // ---- target key -------------------------------------------------------
    let other_lane = if anchor == "acme-sensors" { "sensor-gw" } else { "acme-sensors" };
    let key = match (cond, &req.key) {
        (Condition::WrongLane, _) => format!("{other_lane}/telemetry"),
        (_, Some(k)) => k.clone(),
        _ => format!("{anchor}/telemetry"),
    };
    let nonce = nonce();
    let payload = req
        .payload
        .clone()
        .unwrap_or_else(|| format!("{{\"reading\":21.4,\"id\":\"{nonce}\"}}"));

    let mut gates: Vec<GateVerdict> = Vec::new();

    // ---- revoked / unenrolled short-circuits (NetBird gate) ---------------
    if id.revoked {
        gates.push(gate("netbird", GateState::Fail, "Are you on the mesh at all?",
            "This identity was revoked — the mesh-first cut removed it from the network. There is no CRL; revocation lives at the network layer (the real model).", false));
        return finish(st, id, cond, key, gates, "denied — identity revoked at the network layer", "never reached the fabric", false,
            "Revoked identities are cut at the network, not by a certificate list — fast, local, and immediate.").await;
    }
    if cond == Condition::Unenrolled {
        gates.push(gate("netbird", GateState::Fail, "Are you on the mesh at all?",
            "Not enrolled on the mesh — no L3 route, so packets never arrive. Being off the network isn't a trust decision; the traffic simply doesn't happen.", false));
        gates.push(gate("cert", GateState::Na, "What signed identity do you present?", "Never reached — no transport.", false));
        gates.push(gate("spire", GateState::Na, "An attested, auto-rotating workload?", "Never reached.", false));
        gates.push(gate("ory", GateState::Na, "Which human, and what role?", "Never reached.", false));
        gates.push(gate("acl", GateState::Na, "Does your identity permit THIS write?", "Never reached — the write never left.", false));
        return finish(st, id, cond, key, gates, "no network route", "never reached the fabric", false,
            "Reaching the fabric is the first gate — and the one a browser can't truly exercise (shown, not faked).").await;
    }

    // NetBird gate (represented) — reaching the door is not permission.
    gates.push(gate("netbird", GateState::Pass, "Are you on the mesh at all?",
        "On the mesh — packets can reach the fabric. (Represented: a browser can't do a WireGuard handshake.) Being inside the network is NOT trust.", false));

    // ---- choose cert material per identity kind + condition ---------------
    let tmp = std::env::temp_dir().join(format!("ts-{}-{nonce}", id.handle));
    let _ = std::fs::create_dir_all(&tmp);
    let cert_path;
    let key_path;
    let is_workload = id.kind == IdentityKind::Workload;

    if cond == Condition::Forged {
        // Self-signed leaf — chains to no trusted issuer.
        match ca::self_signed_leaf(&anchor) {
            Ok((c, k)) => {
                cert_path = tmp.join("c.pem");
                key_path = tmp.join("k.pem");
                let _ = std::fs::write(&cert_path, c);
                let _ = std::fs::write(&key_path, k);
            }
            Err(e) => return error_resp(st, id, cond, key, format!("mint forged: {e}")).await,
        }
        gates.push(gate("ory", GateState::Na, "Which human, and what role?",
            "Not in this path — a machine write involves no human/Ory. The failure here is the certificate: it chains to no trusted issuer (see the handshake).", false));
    } else if is_workload && cond != Condition::Expired {
        // Real SPIRE SVID.
        match spire::fetch_svid(&st.cfg.spire_socket, &st.cfg.svid_dir) {
            Ok(s) => {
                cert_path = s.cert_path;
                key_path = s.key_path;
            }
            Err(e) => {
                gates.push(gate("spire", GateState::Fail, "An attested, auto-rotating workload?",
                    &format!("SPIRE didn't issue an SVID: {e}"), true));
                gates.push(gate("acl", GateState::Na, "Does your identity permit THIS write?", "No identity to present.", false));
                return finish(st, id, cond, key, gates, "no attested identity", "handshake never attempted", false,
                    "Without a current attested identity, gate ③ stops you before the access decision.").await;
            }
        }
        gates.push(gate("ory", GateState::Na, "Which human, and what role?",
            "Workload identity — there is no human in this path; SPIRE attests the workload itself.", false));
    } else {
        // CA-minted leaf (org / backing / operator), fresh or expired.
        let expired = cond == Condition::Expired;
        match st.signer.mint(&anchor, id.uri_san.as_deref(), expired) {
            Ok(m) => {
                cert_path = tmp.join("c.pem");
                key_path = tmp.join("k.pem");
                let _ = std::fs::write(&cert_path, m.cert_pem);
                let _ = std::fs::write(&key_path, m.key_pem);
            }
            Err(e) => return error_resp(st, id, cond, key, format!("mint leaf: {e}")).await,
        }
        gates.push(gate("ory", GateState::Na, "Which human, and what role?",
            "Not in this path — a machine write involves no human and no Ory. (Ory authenticates humans to web tools and carries their role; an admin's role can authorize the mint that issued this cert, but that is separate from the write.)", false));
    }

    // ---- cert / spire validity gate (LIVE — from the handshake) -----------
    let t0 = Instant::now();
    let write = zfab::try_write(
        &st.cfg.zenoh_endpoint,
        &root_ca,
        &cert_path.to_string_lossy(),
        &key_path.to_string_lossy(),
        &key,
        &payload,
    )
    .await;

    match write {
        Err(e) => {
            // Handshake refused — forged / expired / chain failure.
            let (g, role, detail) = if is_workload {
                ("spire", "An attested, auto-rotating workload?",
                 format!("The attested credential was refused at the TLS handshake (expired or untrusted): {e}"))
            } else {
                ("cert", "What signed identity do you present?",
                 format!("The certificate was refused at the TLS handshake (forged or expired): {e}"))
            };
            gates.push(gate(g, GateState::Fail, role, &detail, true));
            gates.push(gate("acl", GateState::Na, "Does your identity permit THIS write?",
                "Never reached — no valid identity was presented, so there was nothing to authorize.", false));
            let summary = if cond == Condition::Forged {
                "denied — an identity rooted in no trusted issuer can't even complete the handshake"
            } else if cond == Condition::Expired {
                "denied — a lapsed credential is refused at the handshake; stale identities stop working on their own"
            } else {
                "denied — the credential was refused at the handshake"
            };
            return finish(st, id, cond, key, gates, "handshake refused", "the write never left the publisher", false, summary).await;
        }
        Ok(()) => {
            if is_workload {
                let svid = spire::fetch_svid(&st.cfg.spire_socket, &st.cfg.svid_dir).ok();
                let ttl = svid.map(|s| (s.info.not_after_unix - chrono::Utc::now().timestamp()).max(0)).unwrap_or(0);
                gates.push(gate("spire", GateState::Pass, "An attested, auto-rotating workload?",
                    &format!("A freshly attested SVID was accepted — it auto-rotates (≈{ttl}s of life left). Nothing is permanent; it re-proves itself continuously."), true));
                gates.push(gate("cert", GateState::Na, "What signed identity do you present?",
                    "Identity came from SPIRE, not the portal/CA path — see gate ③.", false));
            } else {
                gates.push(gate("cert", GateState::Pass, "What signed identity do you present?",
                    "A valid, CA-rooted certificate was accepted at the handshake — identity is a signed thing you present, not a name or an address.", true));
                gates.push(gate("spire", GateState::Na, "An attested, auto-rotating workload?",
                    "Portal/CA-issued, not SPIRE-attested — the other issuance path. Both end at the same gate.", false));
            }
        }
    }

    // ---- ACL gate (LIVE — did the observer actually see it land?) ---------
    // Determine the verdict from the CLEAN nonce write BEFORE any burst (a burst
    // on the same key would overwrite the observer's last-seen and hide the nonce).
    tokio::time::sleep(std::time::Duration::from_millis(550)).await;
    let landed = st.observer.seen_since(&key, &nonce, t0);

    // ---- the noisy condition: a real burst into the OWN lane (after verdict) -
    if cond == Condition::Noisy {
        let _ = zfab::burst(
            &st.cfg.zenoh_endpoint, &root_ca,
            &cert_path.to_string_lossy(), &key_path.to_string_lossy(),
            &format!("{anchor}/telemetry"), 25,
        ).await;
    }
    let _ = std::fs::remove_dir_all(&tmp);

    if landed {
        gates.push(gate("acl", GateState::Pass, "Does your identity permit THIS write?",
            &format!("The write to {key} landed on the live subscriber — this identity is permitted in its own lane. Default-deny everywhere else."), true));
        let summary = if cond == Condition::Noisy {
            "allowed — a flooder is a legitimate identity in its own lane, so the per-write check passes. What contains it is that every byte is attributable: the flood shows up against this identity on the health/throughput surface (goat top), and that credential can be revoked to cut it off. Automatic rate-throttle is on the roadmap; monitor-and-revoke is live."
        } else {
            "allowed — every gate passed, so the write landed where this identity is permitted"
        };
        finish(st, id, cond, key, gates, "allow", "the write landed on the live subscriber", false, summary).await
    } else {
        let detail = if cond == Condition::WrongLane {
            format!("Writing to {key} is outside this identity's lane — the ACL denied the ingress, and (the lesson) gave the publisher NO error. The write just never arrived.")
        } else {
            format!("The write to {key} was accepted at the handshake but dropped at the ACL with no error to the publisher — silent denial.")
        };
        gates.push(gate("acl", GateState::Fail, "Does your identity permit THIS write?", &detail, true));
        let summary = if cond == Condition::WrongLane {
            "silently denied — a valid identity writing outside its lane is refused per-resource, quietly. Each identity is confined to exactly what it's permitted."
        } else {
            "silently denied at the access layer — the write never arrived, and nothing told the sender"
        };
        finish(st, id, cond, key, gates, "deny (silent)", "the write never arrived on the subscriber", true, summary).await
    }
}

#[allow(clippy::too_many_arguments)]
async fn finish(
    _st: &Shared,
    id: &Identity,
    cond: Condition,
    key: String,
    gates: Vec<GateVerdict>,
    expected: &str,
    observed: &str,
    silently_denied: bool,
    summary: &str,
) -> AttemptResp {
    AttemptResp {
        identity: id.handle.clone(),
        condition: cond,
        key,
        gates,
        expected: expected.into(),
        observed: observed.into(),
        silently_denied,
        summary: summary.into(),
    }
}

async fn error_resp(st: &Shared, id: &Identity, cond: Condition, key: String, err: String) -> AttemptResp {
    finish(st, id, cond, key, vec![], "error", &err, false, &format!("internal error: {err}")).await
}

// ---------------------------------------------------------------------------
// Mint / rotate / revoke — autonomy + dynamism, gated on ADMIN.
async fn h_mint(State(st): State<Shared>, Json(req): Json<MintReq>) -> impl IntoResponse {
    let claims = match st.oidc.verify(&req.jwt).await {
        Ok(c) => c,
        Err(e) => return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"ok":false,"message":format!("not authenticated: {e}")}))).into_response(),
    };
    if !claims.groups.iter().any(|g| g == "ADMIN") {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({
            "ok": false,
            "message": format!("{} is signed in as {:?} — only an ADMIN role may authorize issuance. Ory authenticates the human + role; the CA does the minting. This is the human-authority gate.", claims.sub, claims.groups)
        }))).into_response();
    }
    let uri = operator_uri_san(&req.anchor);
    match st.signer.mint(&req.anchor, Some(&uri), false) {
        Ok(m) => {
            let _ = st.tx.send(WsEvent::Lifecycle {
                action: if req.rotate { "rotate".into() } else { "mint".into() },
                anchor: req.anchor.clone(),
                at: chrono::Utc::now().timestamp_millis(),
            });
            // Clear a revoked flag if rotating an existing identity back in.
            if let Ok(mut r) = st.roster.lock() {
                if let Some(idn) = r.iter_mut().find(|i| i.anchor.as_deref() == Some(req.anchor.as_str())) {
                    idn.revoked = false;
                }
            }
            Json(MintResp {
                ok: true,
                anchor: req.anchor.clone(),
                rotated: req.rotate,
                cn: m.cn,
                uri_san: m.uri_san,
                serial: m.serial,
                not_after: m.not_after,
                message: format!(
                    "{} a fresh leaf for {} — instant, on {}'s ADMIN authority, no access-rule rebuild. {}",
                    if req.rotate { "Rotated" } else { "Minted" },
                    req.anchor,
                    claims.sub,
                    if req.rotate { "The old leaf still verifies until the mesh-first cut (there's no CRL) — rotation is a re-key, not a revoke." } else { "Access for this anchor was pre-provisioned; the identity is now live." }
                ),
            }).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"ok":false,"message":e.to_string()}))).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct RevokeReq { anchor: String, jwt: String }

async fn h_revoke(State(st): State<Shared>, Json(req): Json<RevokeReq>) -> impl IntoResponse {
    let claims = match st.oidc.verify(&req.jwt).await {
        Ok(c) => c,
        Err(e) => return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"ok":false,"message":format!("not authenticated: {e}")}))).into_response(),
    };
    if !claims.groups.iter().any(|g| g == "ADMIN") {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"ok":false,"message":"only an ADMIN may revoke"}))).into_response();
    }
    let mut found = false;
    if let Ok(mut r) = st.roster.lock() {
        if let Some(idn) = r.iter_mut().find(|i| i.anchor.as_deref() == Some(req.anchor.as_str())) {
            idn.revoked = true;
            found = true;
        }
    }
    if found {
        let _ = st.tx.send(WsEvent::Lifecycle { action: "revoke".into(), anchor: req.anchor.clone(), at: chrono::Utc::now().timestamp_millis() });
        Json(serde_json::json!({
            "ok": true,
            "message": format!("Revoked {} — a mesh-first cut at the network layer, effective immediately. No certificate list to distribute; the next attempt won't even reach the fabric.", req.anchor)
        })).into_response()
    } else {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({"ok":false,"message":"unknown anchor"}))).into_response()
    }
}

// ---------------------------------------------------------------------------
// OIDC login / consent (the orchestrator is Hydra's login+consent app).
#[derive(serde::Deserialize)]
struct LoginReq { challenge: String, persona: String }
#[derive(serde::Deserialize)]
struct ConsentReq { challenge: String }

async fn h_login(State(st): State<Shared>, Json(req): Json<LoginReq>) -> impl IntoResponse {
    match st.oidc.accept_login(&req.challenge, &req.persona).await {
        Ok(redirect) => Json(serde_json::json!({"redirect_to": redirect})).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}

async fn h_consent(State(st): State<Shared>, Json(req): Json<ConsentReq>) -> impl IntoResponse {
    let subject = match st.oidc.consent_subject(&req.challenge).await {
        Ok(s) => s,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    };
    let role = role_of(&st.roster.lock().unwrap(), &subject);
    match st.oidc.accept_consent(&req.challenge, &role).await {
        Ok(redirect) => Json(serde_json::json!({"redirect_to": redirect, "role": role})).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}

// ---------------------------------------------------------------------------
// Private DNS — resolve + burn-&-respawn.
async fn h_resolve(State(st): State<Shared>) -> impl IntoResponse {
    match dns::resolve(&st.cfg.coredns, &st.cfg.target_name) {
        Ok(addrs) => Json(serde_json::json!({
            "ok": true, "name": st.cfg.target_name, "addresses": addrs,
            "note": "This name resolved to an address and nothing else — it confers no trust. The endpoint still has to pass every gate."
        })).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"ok":false,"error":e.to_string()}))).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct BurnReq {
    /// respawn with a bad/unauthorized identity (wrong anchor) instead of a fresh good one.
    #[serde(default)]
    bad: bool,
}

async fn h_burn(State(st): State<Shared>, Json(req): Json<BurnReq>) -> impl IntoResponse {
    // 1. Re-point the mesh name to the backing host's real (docker-resolved)
    //    address. The address is real + reachable; the cert is the identity that
    //    changes underneath.
    let fresh_ip = (st.cfg.target_host.clone() + ":80")
        .to_socket_addrs_first()
        .unwrap_or_else(|| "10.99.0.2".into());
    let serial = dns::repoint(&st.cfg.zone_file, &fresh_ip).unwrap_or(0);

    // 2. Respawn the backing identity: a fresh good leaf (CN=sensor-gw) or a
    //    deliberately bad one (CN=rogue-gw → no ACL grant for sensor-gw/**).
    let cn = if req.bad { "rogue-gw" } else { "sensor-gw" };
    let nonce = nonce();
    let root_ca = format!("{}/ca-root.pem", st.cfg.certs_dir);
    let tmp = std::env::temp_dir().join(format!("ts-burn-{nonce}"));
    let _ = std::fs::create_dir_all(&tmp);
    let leaf = match st.signer.mint(cn, Some(&operator_uri_san(cn)), false) {
        Ok(m) => m,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"ok":false,"error":e.to_string()}))).into_response(),
    };
    let cert_path = tmp.join("c.pem");
    let key_path = tmp.join("k.pem");
    let _ = std::fs::write(&cert_path, leaf.cert_pem);
    let _ = std::fs::write(&key_path, leaf.key_pem);

    // 3. Reconnect BY THE SAME NAME and write to sensor-gw/telemetry.
    let key = "sensor-gw/telemetry".to_string();
    let payload = format!("{{\"respawn\":true,\"id\":\"{nonce}\"}}");
    let t0 = Instant::now();
    let wrote = zfab::try_write(&st.cfg.zenoh_endpoint, &root_ca,
        &cert_path.to_string_lossy(), &key_path.to_string_lossy(), &key, &payload).await;
    tokio::time::sleep(std::time::Duration::from_millis(550)).await;
    let landed = wrote.is_ok() && st.observer.seen_since(&key, &nonce, t0);
    let _ = std::fs::remove_dir_all(&tmp);

    let _ = st.tx.send(WsEvent::Lifecycle { action: if req.bad {"respawn-bad".into()} else {"respawn".into()}, anchor: "sensor-gw".into(), at: chrono::Utc::now().timestamp_millis() });

    let addrs = dns::resolve(&st.cfg.coredns, &st.cfg.target_name).unwrap_or_default();
    let summary = if req.bad {
        "Respawned with an unauthorized identity. The name resolved exactly the same — but the connection was refused. The name never conferred trust."
    } else if landed {
        "Respawned with a fresh identity and re-pointed the name. The consumer reconnected to the SAME name and it just worked — trust re-earned cryptographically, nobody re-addressed, everyone else kept functioning."
    } else {
        "Respawned, but the fresh write didn't land — check the fabric."
    };
    Json(serde_json::json!({
        "ok": true,
        "bad": req.bad,
        "name": st.cfg.target_name,
        "addresses": addrs,
        "serial": serial,
        "cn": cn,
        "landed": landed,
        "summary": summary
    })).into_response()
}

// small helper: first resolved socket addr's IP as a string
trait FirstAddr { fn to_socket_addrs_first(&self) -> Option<String>; }
impl FirstAddr for String {
    fn to_socket_addrs_first(&self) -> Option<String> {
        use std::net::ToSocketAddrs;
        self.to_socket_addrs().ok()?.next().map(|s| s.ip().to_string())
    }
}

// ---------------------------------------------------------------------------
// WebSocket — push real observer + lifecycle events to browsers.
async fn h_ws(State(st): State<Shared>, ws: WebSocketUpgrade) -> impl IntoResponse {
    let mut rx = st.tx.subscribe();
    ws.on_upgrade(move |mut socket| async move {
        use axum::extract::ws::Message;
        while let Ok(ev) = rx.recv().await {
            if let Ok(txt) = serde_json::to_string(&ev) {
                if socket.send(Message::Text(txt)).await.is_err() {
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod oidc_tests;
