// SPDX-License-Identifier: Apache-2.0
//
// goat-ntp-health-pub — per-host NTP-health attestation republisher.
//
// ADR 1036 (env-health time domain) + ADR 1037 (internal-sync foundation; this
// feed is the attestation / enforcement plane) + F-184 (the publisher resolved
// but never built).
//
// Shape (a normal mTLS Zenoh client peer; templated on goat-fabric-health):
//   - Reads the local ntp-health-agent's SIGNED verdict over HTTP
//     (http://127.0.0.1:9753/health — ntp-attest-v1 JSON).
//   - Republishes it VERBATIM (it never re-signs; the agent owns the key) on
//     the bus in the control class (C0 — Block + high priority) so the
//     attestation signal is never starved by the load it travels beside:
//       ops/time/attest/<host_id>   <- the signed NtpState JSON
//   - On a ~30s heartbeat (universal-ntp-hygiene §5.1). Persists nothing.
//
// The key is under the canonical operator namespace `ops/` (ADR 0844 / 1035),
// NOT Zenoh's reserved `@/` adminspace: `@/` is router-introspection (GET/
// queryable), not a pub/sub data topic, and the `@/ntp-health` design predates
// ADR 1035. The rolled per-site viewing
// reading lives separately at ops/time/health/<site> (env-health time domain).

use anyhow::{Context, Result};
use clap::Parser;
use std::time::Duration;
use tracing::{info, warn};
use zenoh::qos::{CongestionControl, Priority};

#[derive(Parser, Debug)]
#[command(
    name = "goat-ntp-health-pub",
    about = "Per-host NTP-health attestation republisher (ADR 1036/1037, F-184)"
)]
struct Args {
    /// Zenoh router endpoint, e.g. `tls/goat-zenoh-router:7447`.
    #[arg(long, env = "NTP_HEALTH_PUB_ENDPOINT")]
    endpoint: String,

    /// Host identifier — the `<host_id>` segment of ops/time/attest/<host_id>.
    /// Defaults to the OS hostname; MUST match the ntp-health-agent's host_id
    /// (which also defaults to the OS hostname) so the key names the signer.
    #[arg(long, env = "NTP_HEALTH_PUB_HOST_ID")]
    host_id: Option<String>,

    /// The ntp-health-agent verdict endpoint (signed JSON).
    #[arg(
        long,
        env = "NTP_HEALTH_AGENT_URL",
        default_value = "http://127.0.0.1:9753/health"
    )]
    agent_url: String,

    /// Leaf X.509 cert (PEM) for mTLS. Required for a `tls/` endpoint.
    #[arg(long, env = "NTP_HEALTH_PUB_CERT")]
    cert: Option<String>,
    /// Matching private key (PEM).
    #[arg(long, env = "NTP_HEALTH_PUB_KEY_FILE")]
    key: Option<String>,
    /// CA root cert (PEM) used to verify the router.
    #[arg(long, env = "NTP_HEALTH_PUB_ROOT_CA")]
    root_ca: Option<String>,

    /// Republish interval (ms). ~30s heartbeat per universal-ntp-hygiene §5.1.
    #[arg(long, env = "NTP_HEALTH_PUB_INTERVAL_MS", default_value_t = 30_000)]
    interval_ms: u64,
}

/// The Zenoh key the per-host signed attestation is published on. Operator
/// namespace `ops/` (ADR 1035), never the `@/` adminspace.
fn attest_key(host_id: &str) -> String {
    format!("ops/time/attest/{host_id}")
}

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
}

/// Read the agent's signed verdict and republish it verbatim. Returns the byte
/// count republished. The agent returns 200 when healthy and 503 otherwise —
/// both carry a valid signed verdict body, so both are forwarded; only a
/// transport / read failure (or an empty body) is an error.
async fn publish_round(
    session: &zenoh::Session,
    client: &reqwest::Client,
    agent_url: &str,
    key: &str,
) -> Result<usize> {
    let resp = client
        .get(agent_url)
        .send()
        .await
        .context("GET ntp-health-agent verdict")?;
    let status = resp.status();
    let body = resp.bytes().await.context("read agent verdict body")?;
    if body.is_empty() {
        anyhow::bail!("agent returned empty body (status {status})");
    }
    let n = body.len();
    session
        .put(key, body.to_vec())
        .congestion_control(CongestionControl::Block)
        .priority(Priority::InteractiveHigh)
        .await
        .map_err(|e| anyhow::anyhow!("zenoh put {key}: {e}"))?;
    Ok(n)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let host_id = match &args.host_id {
        Some(h) => h.clone(),
        None => hostname::get()
            .context("read OS hostname")?
            .to_string_lossy()
            .into_owned(),
    };
    let key = attest_key(&host_id);

    let session = open_session(&args).await.context("open zenoh session")?;
    info!(
        zid = %session.zid(),
        endpoint = args.endpoint.as_str(),
        key = key.as_str(),
        agent = args.agent_url.as_str(),
        "session open; republishing ntp-health attestation"
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("build http client")?;

    // Publish immediately on start, then on each heartbeat. SIGTERM/SIGINT
    // shut the loop down cleanly so the container exits promptly.
    let mut ticker = tokio::time::interval(Duration::from_millis(args.interval_ms));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await; // consume the immediate first tick (we publish below)
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("install SIGTERM handler")?;

    loop {
        match publish_round(&session, &client, &args.agent_url, &key).await {
            Ok(n) => info!(bytes = n, key = key.as_str(), "republished attestation"),
            Err(e) => warn!(error = %e, "publish round failed (will retry next heartbeat)"),
        }
        tokio::select! {
            _ = ticker.tick() => {}
            _ = sigterm.recv() => { info!("SIGTERM received; shutting down"); break; }
            _ = tokio::signal::ctrl_c() => { info!("SIGINT received; shutting down"); break; }
        }
    }

    let _ = session.close().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_under_ops_namespace_not_adminspace() {
        let k = attest_key("mgmt-01");
        assert_eq!(k, "ops/time/attest/mgmt-01");
        // The load-bearing contract: never the `@/` adminspace (ADR 1035 + the
        // F-184 routing note), and under the canonical `ops/` operator prefix.
        assert!(k.starts_with("ops/time/attest/"));
        assert!(!k.starts_with("@/"));
    }
}
