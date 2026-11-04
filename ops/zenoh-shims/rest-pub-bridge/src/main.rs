//! rest-pub-bridge — bridges HTTP-PUT publishes to a native mTLS Zenoh
//! session.
//!
//! ## Why this exists
//!
//! Every native Zenoh peer in this repo (synth-vendor, audit-subscriber,
//! panoscope, the portal) opens a TLS session and presents a per-vendor
//! X.509 leaf; the router's `access_control` matches the leaf Common Name
//! to a write rule (`CN=<vendor>-trial-peer` → `put` on `<vendor>/**`).
//!
//! A publisher that *can't* open a native session has no wire identity. The
//! demogame game server is the motivating case: it's Go, and there is
//! no maintained Go client for Zenoh 1.x, so it publishes via plain HTTP
//! PUT to the router's REST plugin — a surface with no client-cert
//! requirement. After the ADR 1020 ingress/egress ACL split, the router
//! grants publish *only* to identified per-vendor subjects, so those
//! identity-less REST PUTs are denied and the data stops flowing.
//!
//! This bridge restores the missing identity without touching the Go
//! publisher. It mimics the REST plugin's PUT contract exactly
//! (`PUT /<keyexpr>` with the value as the body), and re-emits each sample
//! on a native mTLS session keyed by the vendor leaf it mounts. The Go
//! publisher just points `ZENOH_REST_ENDPOINT` at this bridge instead of
//! the router. Reusable by any non-Rust publisher in the same bind.
//!
//! ## Wire contract (mirrors the Zenoh REST plugin)
//!
//! ```text
//! PUT /<keyexpr>        body = value bytes      → publish to <keyexpr>
//! POST /<keyexpr>       (accepted for parity)
//! GET /health          → 200 "ok"              (liveness for smoketests)
//! ```
//!
//! The URL path (minus the leading `/`) is the Zenoh key-expression. The
//! request body is the sample value; the request Content-Type, when
//! present, becomes the Zenoh encoding so downstream raw views render it.
//!
//! ## Defense in depth
//!
//! `--allow-prefix` (repeatable / comma-separated) restricts which
//! key-expressions the bridge will forward. The bridge holds a real vendor
//! cert, so a compromised or buggy upstream must not be able to ask it to
//! publish outside the vendor's namespace. The router ACL already scopes
//! the cert to `<vendor>/**`; this is the belt to that suspenders.
//!
//! License: Apache-2.0

use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::IntoResponse,
    routing::get,
    Router,
};
use clap::Parser;
use tracing::{error, info, warn};
use zenoh::bytes::Encoding;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "rest-pub-bridge",
    about = "REST-PUT to native-mTLS-Zenoh publish bridge for non-Rust publishers"
)]
struct Args {
    /// Address the HTTP listener binds (the upstream publisher PUTs here).
    #[arg(long, env = "REST_PUB_BRIDGE_LISTEN", default_value = "0.0.0.0:8000")]
    listen: String,

    /// Zenoh router endpoint. `tls/...` selects native mTLS (the live
    /// the router listens mTLS-only on 7447); `tcp/...` is plaintext
    /// for local testing only.
    #[arg(
        long,
        env = "REST_PUB_BRIDGE_ENDPOINT",
        default_value = "tls/goat-zenoh-router:7447"
    )]
    endpoint: String,

    /// Client leaf certificate (PEM). Required for a `tls/` endpoint. Its
    /// Common Name is the ACL identity axis (`CN=<vendor>-trial-peer`).
    #[arg(long, env = "REST_PUB_BRIDGE_CERT")]
    cert: Option<String>,

    /// Client private key (PEM). Required for a `tls/` endpoint.
    #[arg(long, env = "REST_PUB_BRIDGE_KEY")]
    key: Option<String>,

    /// Trust root the router's server cert must chain to (PEM). Required
    /// for a `tls/` endpoint.
    #[arg(long, env = "REST_PUB_BRIDGE_ROOT_CA")]
    root_ca: Option<String>,

    /// Comma-separated key-expression prefixes the bridge will forward.
    /// Empty = forward any key (no restriction). A publish whose key does
    /// not start with one of these is rejected with 403 before it reaches
    /// the router. Example: `demogame/`.
    #[arg(long, env = "REST_PUB_BRIDGE_ALLOW_PREFIX", default_value = "")]
    allow_prefix: String,
}

#[derive(Clone)]
struct AppState {
    session: Arc<zenoh::Session>,
    allow_prefixes: Arc<Vec<String>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let args = Args::parse();

    let allow_prefixes: Vec<String> = args
        .allow_prefix
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    info!(
        listen = %args.listen,
        endpoint = %args.endpoint,
        allow_prefixes = ?allow_prefixes,
        "rest-pub-bridge starting"
    );

    let session = open_session(&args).await?;
    info!("zenoh session opened");

    let state = AppState {
        session: Arc::new(session),
        allow_prefixes: Arc::new(allow_prefixes),
    };

    let app = Router::new()
        .route("/health", get(health))
        .fallback(publish)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&args.listen)
        .await
        .with_context(|| format!("binding {}", args.listen))?;
    info!(listen = %args.listen, "http listener bound");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            info!("shutdown signal received");
        })
        .await
        .context("axum serve")?;

    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

/// Catch-all publish handler. The URL path is the key-expression; the body
/// is the value. Mirrors the Zenoh REST plugin's PUT/POST semantics.
async fn publish(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    if method != Method::PUT && method != Method::POST {
        return (StatusCode::METHOD_NOT_ALLOWED, "use PUT or POST").into_response();
    }

    // URL path → key-expression. Strip the leading '/' (Zenoh keys never
    // start with '/'). Reject an empty key.
    let key = uri.path().trim_start_matches('/');
    if key.is_empty() {
        return (StatusCode::BAD_REQUEST, "empty key-expression").into_response();
    }

    // Defense-in-depth: refuse keys outside the configured namespace(s).
    if !state.allow_prefixes.is_empty()
        && !state.allow_prefixes.iter().any(|p| key.starts_with(p))
    {
        warn!(key, "publish rejected: key not under an allowed prefix");
        return (
            StatusCode::FORBIDDEN,
            "key-expression not under an allowed prefix",
        )
            .into_response();
    }

    // Carry the request Content-Type through as the Zenoh encoding so raw
    // sample views render the payload correctly.
    let encoding = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(Encoding::from)
        .unwrap_or(Encoding::APPLICATION_OCTET_STREAM);

    match state
        .session
        .put(key, body.to_vec())
        .encoding(encoding)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            // A denied write (router ACL) surfaces here as a session error.
            // Log it loudly — silent drops are the failure mode this whole
            // bridge exists to eliminate.
            error!(key, error = %format!("{e}"), "zenoh put failed");
            (StatusCode::BAD_GATEWAY, format!("zenoh put failed: {e}")).into_response()
        }
    }
}

/// Open a Zenoh client session. mTLS material is injected when the endpoint
/// uses the `tls/` scheme. Mirrors the synth-vendor / audit-subscriber
/// idiom verbatim (docs/design/zenoh-trial-fabric-synth-vendors.md).
async fn open_session(args: &Args) -> Result<zenoh::Session> {
    let mut config = zenoh::Config::default();
    config
        .insert_json5("mode", "\"client\"")
        .map_err(|e| anyhow::anyhow!("config mode: {e}"))?;
    config
        .insert_json5("connect/endpoints", &format!("[\"{}\"]", args.endpoint))
        .map_err(|e| anyhow::anyhow!("config connect/endpoints: {e}"))?;
    if args.endpoint.starts_with("tls/") {
        let cert = args.cert.as_ref().context("--cert required for tls/ endpoint")?;
        let key = args.key.as_ref().context("--key required for tls/ endpoint")?;
        let root_ca = args
            .root_ca
            .as_ref()
            .context("--root-ca required for tls/ endpoint")?;
        // Zenoh 1.9.0's transport.link.tls schema has no `enabled` field —
        // TLS is selected by the endpoint scheme (`tls/...`). Hostname
        // verification is off: the router cert SAN binds the host's mesh
        // IP, not the docker DNS name we connect through. enable_mtls makes
        // the client present its leaf (without it the handshake fails with
        // CertificateRequired). Same block as synth-vendor + panoscope-core.
        let tls_block = format!(
            r#"{{ "root_ca_certificate": "{root_ca}", "connect_certificate": "{cert}", "connect_private_key": "{key}", "enable_mtls": true, "verify_name_on_connect": false }}"#
        );
        config
            .insert_json5("transport/link/tls", &tls_block)
            .map_err(|e| anyhow::anyhow!("config transport/link/tls: {e}"))?;
    }
    zenoh::open(config)
        .await
        .map_err(|e| anyhow::anyhow!("zenoh::open failed: {e}"))
        .context("opening zenoh session")
}
