// SPDX-License-Identifier: Apache-2.0
//
// goat-trashgoat — operator-controlled chaos publisher.
//
// Subscribes to ops/trashgoat/control/** for spec-state-change
// events the portal publishes (per docs/design/trashgoat.md
// §"Control wire contract"), maintains an in-memory spec table, and
// runs active specs per their declared payload + cadence under the
// trashgoat/<slug>/... namespace.
//
// Design boundaries (see docs/design/trashgoat.md):
//   - Specs are declarative shapes; the runner interprets them. No
//     arbitrary code execution.
//   - The runner is single-vendor — its identity is the cert-bound
//     `trashgoat` URI-SAN. It does not impersonate other vendors.
//   - Generated payloads are ephemeral. Cleanse issues z_delete
//     against every key a spec has touched; the runner doesn't keep
//     them otherwise.
//   - Per-spec sequence + history live in-process. Restart loses
//     them; the portal's spec table + the runner's cold-start
//     `session.get("ops/trashgoat/control/**")` are the durability
//     story (standard pub/sub late-joiner pattern).

mod cadence;
mod control;
mod payload;
mod runner;

use anyhow::{Context, Result};
use clap::Parser;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

#[derive(Parser, Debug, Clone)]
#[command(
    name = "goat-trashgoat",
    about = "Operator-controlled chaos publisher. Block 92 per docs/design/trashgoat.md."
)]
pub struct Args {
    /// Zenoh router endpoint, e.g. `tls/zenoh-router:7447`.
    #[arg(long, env = "TRASHGOAT_ENDPOINT")]
    pub endpoint: String,

    /// Path to the trashgoat leaf X.509 certificate (PEM). Required
    /// when `endpoint` starts with `tls/`.
    #[arg(long, env = "TRASHGOAT_CERT")]
    pub cert: Option<String>,

    /// Path to the matching private key (PEM). Required when
    /// `endpoint` starts with `tls/`.
    #[arg(long, env = "TRASHGOAT_KEY")]
    pub key: Option<String>,

    /// Path to the CA root certificate (PEM) used to verify the
    /// router's server cert. Required when `endpoint` starts with
    /// `tls/`.
    #[arg(long, env = "TRASHGOAT_ROOT_CA")]
    pub root_ca: Option<String>,

    /// Control-topic prefix this runner subscribes to. Default
    /// matches the portal's contract (ops/trashgoat/control/**).
    /// Exposed as a flag for test rigs that want to drive the runner
    /// on a sandbox prefix.
    #[arg(
        long,
        env = "TRASHGOAT_CONTROL_KEY",
        default_value = "ops/trashgoat/control/**"
    )]
    pub control_key: String,

    /// Maximum publish rate ceiling (Hz). The runner refuses cadences
    /// that would exceed this. Per design doc §"Out of scope for v1"
    /// the runner is free to ignore cadence_spec values it considers
    /// abusive; this is the implementation of that. Operator can
    /// raise via env if a specific experiment legitimately needs more.
    #[arg(long, env = "TRASHGOAT_MAX_HZ", default_value_t = 100.0)]
    pub max_hz: f64,

    /// Upper bound on number of keys the runner remembers per spec
    /// for cleanse-time `z_delete`. Templated topic_pattern with
    /// `{uuid}` substitution can in principle produce unbounded keys;
    /// this caps the bookkeeping memory at a reasonable per-spec
    /// budget (cleanse drops the oldest keys above the cap with a
    /// warning).
    #[arg(long, env = "TRASHGOAT_MAX_KEYS_PER_SPEC", default_value_t = 10000)]
    pub max_keys_per_spec: usize,

    /// Logging filter (RUST_LOG-style). Defaults verbose enough to
    /// see one line per state-change but quiet enough that a 100 Hz
    /// publish loop doesn't spam.
    #[arg(long, env = "RUST_LOG")]
    pub log: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    init_tracing(&args);

    let session = open_session(&args).await.context("open zenoh session")?;
    info!(
        zid = %session.zid(),
        endpoint = args.endpoint.as_str(),
        control_key = args.control_key.as_str(),
        max_hz = args.max_hz,
        "session open"
    );

    let runner_state = Arc::new(Mutex::new(runner::RunnerState::new(
        args.max_hz,
        args.max_keys_per_spec,
    )));

    // Cold-start: snapshot whatever the portal currently has on the
    // control topic via session.get(), then subscribe for live
    // updates. The portal's contract is that every state-change
    // PUT lands on ops/trashgoat/control/<id>/v1 — replay = grab
    // every key matching that pattern and feed it through the same
    // dispatcher that handles live samples.
    if let Err(e) = control::cold_start_snapshot(&session, &args.control_key, &runner_state, &session).await {
        warn!(error = %e, "cold-start snapshot failed; continuing with empty spec table");
    }

    // Live control-event subscriber.
    let subscriber = session
        .declare_subscriber(args.control_key.clone())
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "declare_subscriber({}) failed: {e}",
                args.control_key.as_str()
            )
        })?;
    let runner_state_for_sub = runner_state.clone();
    let session_for_sub = session.clone();
    // Admin-path spawn — one long-lived task that drains the
    // control-topic subscriber for the whole life of the runner.
    // ADR 0709 D7 layer 3's spawn-local rule targets goat-relay's
    // shard-pinned hot path; trashgoat is admin-path with no
    // per-thread shard model. Deviation visible per clippy.toml.
    #[allow(clippy::disallowed_methods)]
    let _control_loop = tokio::spawn(async move {
        loop {
            match subscriber.recv_async().await {
                Ok(sample) => {
                    let key_expr = sample.key_expr().to_string();
                    let bytes = sample.payload().to_bytes();
                    if let Err(e) = control::dispatch_control_event(
                        &session_for_sub,
                        &runner_state_for_sub,
                        &key_expr,
                        bytes.as_ref(),
                    )
                    .await
                    {
                        warn!(error = %e, key_expr = %key_expr, "control event dispatch failed");
                    }
                }
                Err(e) => {
                    warn!(error = %e, "control subscriber recv error; loop ending");
                    break;
                }
            }
        }
    });

    // Wait for SIGINT to shut down cleanly.
    if let Err(e) = tokio::signal::ctrl_c().await {
        error!(error = %e, "ctrl_c handler failed");
    }
    info!("shutdown signal received");

    // Stop all active spec publishers so they don't keep firing past
    // the session close.
    runner_state.lock().await.shutdown_all().await;

    Ok(())
}

fn init_tracing(args: &Args) {
    let filter = match args.log.as_ref() {
        Some(f) if !f.is_empty() => tracing_subscriber::EnvFilter::new(f.as_str()),
        _ => tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

async fn open_session(args: &Args) -> Result<zenoh::Session> {
    let mut config = zenoh::Config::default();
    config
        .insert_json5("mode", "\"client\"")
        .map_err(|e| anyhow::anyhow!("config mode: {e}"))?;
    config
        .insert_json5(
            "connect/endpoints",
            &format!("[\"{}\"]", args.endpoint),
        )
        .map_err(|e| anyhow::anyhow!("config connect/endpoints: {e}"))?;
    if args.endpoint.starts_with("tls/") {
        let cert = args
            .cert
            .as_ref()
            .context("--cert required for tls/ endpoint")?;
        let key = args
            .key
            .as_ref()
            .context("--key required for tls/ endpoint")?;
        let root_ca = args
            .root_ca
            .as_ref()
            .context("--root-ca required for tls/ endpoint")?;
        // Same TLS shape as synth-vendor + audit-subscriber +
        // panoscope: TLS is selected by the endpoint scheme; hostname
        // verification off because the router cert SAN binds the
        // mesh IP, not the docker DNS name we connect through.
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
