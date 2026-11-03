// SPDX-License-Identifier: Apache-2.0
//! goat-topology — the zenoh routing-topology surface (ADR 1160).
//!
//! ```text
//!   goat-topology publish   on each router host: read MY router, publish it
//!   goat-topology serve     on the mgmt-host:    subscribe, render, grade
//! ```
//!
//! The split is not stylistic. The router adminspace binds to its host's
//! loopback, so only a co-located process can read it; and a surface that
//! queried each router per page view would be exactly the shape ADR 1123
//! forbids. Self-publish + passive roll-up is also what survives a partition:
//! a cut-off site keeps publishing and the surface shows its last snapshot with
//! an honest age (ADR 1036).

mod adminspace;
mod assets;
mod findings;
mod gwconfig;
mod serve;
mod store;
mod view;
mod wire;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use wire::{GatewayMode, RouterTopologyV1};

#[derive(Parser, Debug)]
#[command(
    name = "goat-topology",
    about = "Zenoh routing-topology surface (ADR 1160)."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Run on a zenoh router host: read this router, publish what it says.
    Publish(PublishArgs),
    /// Run on the mgmt-host: subscribe, hold in memory, serve the surface.
    Serve(ServeArgs),
    /// Print what this host's router looks like, as JSON, and exit. Publishes
    /// nothing and opens no zenoh session — so it is safe to run against a
    /// production router, and it is how an operator answers "what does
    /// topology think my config says?" without waiting for a publish cycle.
    Inspect(InspectArgs),
}

#[derive(Parser, Debug)]
struct InspectArgs {
    #[arg(long, env = "TOPOLOGY_ADMIN_ADDR", default_value = "127.0.0.1:8000")]
    admin_addr: String,
    #[arg(long, env = "TOPOLOGY_CONFIG_PATH")]
    config_path: Option<String>,
    /// Read the declared config only; do not touch the adminspace. Lets the
    /// config parser be exercised off-host, against a captured file.
    #[arg(long, default_value_t = false)]
    config_only: bool,
}

#[derive(Parser, Debug)]
struct PublishArgs {
    /// Zenoh router endpoint to PUBLISH through, e.g. `tls/goat-zenoh-router:7447`.
    #[arg(long, env = "TOPOLOGY_ENDPOINT")]
    endpoint: String,
    /// `<site>` segment of ops/fabric/topology/<site>/<node>.
    #[arg(long, env = "TOPOLOGY_SITE")]
    site: String,
    /// `<node>` segment — this host. Must be unique within the site.
    #[arg(long, env = "TOPOLOGY_NODE")]
    node: String,

    /// The local router's adminspace REST address. Loopback by default: the
    /// router publishes 8000/tcp to 127.0.0.1, so no netns entry is needed.
    #[arg(long, env = "TOPOLOGY_ADMIN_ADDR", default_value = "127.0.0.1:8000")]
    admin_addr: String,
    /// The router's DECLARED config file. Only its `gateway` block is read;
    /// the ACL that dominates this file is never parsed, logged or published.
    #[arg(long, env = "TOPOLOGY_CONFIG_PATH")]
    config_path: Option<String>,

    #[arg(long, env = "TOPOLOGY_INTERVAL_SECS", default_value_t = 30)]
    interval_secs: u64,

    /// Leaf X.509 cert (PEM) for mTLS. Required for a `tls/` endpoint.
    #[arg(long, env = "TOPOLOGY_CERT")]
    cert: Option<String>,
    #[arg(long, env = "TOPOLOGY_KEY_FILE")]
    key: Option<String>,
    #[arg(long, env = "TOPOLOGY_ROOT_CA")]
    root_ca: Option<String>,
}

#[derive(Parser, Debug)]
struct ServeArgs {
    #[arg(long, env = "TOPOLOGY_ENDPOINT")]
    endpoint: String,
    /// Restrict to one site's routers. Omit to roll up every site the
    /// subscription can see.
    #[arg(long, env = "TOPOLOGY_SITE")]
    site: Option<String>,
    #[arg(long, env = "TOPOLOGY_BIND", default_value = "0.0.0.0:8080")]
    bind: String,

    #[arg(long, env = "TOPOLOGY_CERT")]
    cert: Option<String>,
    #[arg(long, env = "TOPOLOGY_KEY_FILE")]
    key: Option<String>,
    #[arg(long, env = "TOPOLOGY_ROOT_CA")]
    root_ca: Option<String>,
}

/// Shared zenoh client config. Mirrors goat-fabric-health so both meters fail
/// the same way against the same router.
struct Tls<'a> {
    endpoint: &'a str,
    cert: Option<&'a String>,
    key: Option<&'a String>,
    root_ca: Option<&'a String>,
}

async fn open_session(t: Tls<'_>) -> Result<zenoh::Session> {
    let mut config = zenoh::Config::default();
    config
        .insert_json5("mode", "\"client\"")
        .map_err(|e| anyhow::anyhow!("config mode: {e}"))?;
    config
        .insert_json5("connect/endpoints", &format!("[\"{}\"]", t.endpoint))
        .map_err(|e| anyhow::anyhow!("config connect/endpoints: {e}"))?;
    if t.endpoint.starts_with("tls/") {
        let cert = t.cert.context("--cert required for a tls/ endpoint")?;
        let key = t.key.context("--key required for a tls/ endpoint")?;
        let root_ca = t
            .root_ca
            .context("--root-ca required for a tls/ endpoint")?;
        let block = format!(
            r#"{{ "root_ca_certificate": "{root_ca}", "connect_certificate": "{cert}", "connect_private_key": "{key}", "enable_mtls": true, "verify_name_on_connect": false }}"#
        );
        config
            .insert_json5("transport/link/tls", &block)
            .map_err(|e| anyhow::anyhow!("config transport/link/tls: {e}"))?;
    }
    zenoh::open(config)
        .await
        .map_err(|e| anyhow::anyhow!("open zenoh session: {e}"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Build one snapshot of this router.
///
/// Never returns `Err` for a read failure: a snapshot that says "I could not
/// read my own adminspace" is more useful than silence, because silence is
/// indistinguishable from a dead publisher and would be graded `unknown`
/// without saying why.
async fn snapshot(a: &PublishArgs) -> RouterTopologyV1 {
    let mut snap = RouterTopologyV1 {
        site: a.site.clone(),
        node: a.node.clone(),
        ts_unix_ms: now_ms(),
        interval_secs: a.interval_secs as u32,
        ..Default::default()
    };

    // --- declared: the gateway block of the config file --------------------
    match a.config_path.as_deref() {
        Some(p) => {
            let d = gwconfig::read(p);
            snap.gateway_mode = d.mode as i32;
            snap.custom_subregions = d.subregions;
            snap.gateway_read_error = d.error;
        }
        None => {
            snap.gateway_mode = GatewayMode::Unreadable as i32;
            snap.gateway_read_error =
                "no --config-path given, so the declared gateway posture is unknown; \
                 it is NOT assumed to be auto"
                    .into();
        }
    }

    // --- observed: the adminspace ------------------------------------------
    if a.admin_addr.is_empty() {
        snap.observe_error = "adminspace not read (--config-only)".into();
        return snap;
    }
    match adminspace::fetch(&a.admin_addr, 10).await {
        Err(e) => snap.observe_error = format!("{e:#}"),
        Ok(raw) => match adminspace::split_response(&raw) {
            Err(e) => snap.observe_error = format!("{e:#}"),
            Ok((status, _)) if status != 200 => {
                snap.observe_error = format!("adminspace returned HTTP {status}");
            }
            Ok((_, body)) => match adminspace::parse(body) {
                Err(e) => snap.observe_error = format!("{e:#}"),
                Ok(obs) => adminspace::apply(&mut snap, obs),
            },
        },
    }
    snap
}

async fn run_publish(a: PublishArgs) -> Result<()> {
    let session = open_session(Tls {
        endpoint: &a.endpoint,
        cert: a.cert.as_ref(),
        key: a.key.as_ref(),
        root_ca: a.root_ca.as_ref(),
    })
    .await?;

    let key = format!("ops/fabric/topology/{}/{}", a.site, a.node);
    tracing::info!(key = %key, admin = %a.admin_addr, "publishing router topology");

    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(a.interval_secs));
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let snap = snapshot(&a).await;
                if !snap.observe_error.is_empty() {
                    tracing::warn!(err = %snap.observe_error, "adminspace read failed; publishing the failure");
                }
                if let Err(e) = session.put(&key, snap.encode_vec()).await {
                    tracing::warn!("publish {key}: {e}");
                }
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutting down");
                return Ok(());
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    match Cli::parse().cmd {
        Cmd::Publish(a) => run_publish(a).await,
        Cmd::Inspect(a) => {
            let snap = snapshot(&PublishArgs {
                endpoint: String::new(),
                site: "-".into(),
                node: "-".into(),
                admin_addr: if a.config_only {
                    String::new()
                } else {
                    a.admin_addr
                },
                config_path: a.config_path,
                interval_secs: 0,
                cert: None,
                key: None,
                root_ca: None,
            })
            .await;
            println!("{}", serde_json::to_string_pretty(&snap)?);
            Ok(())
        }
        Cmd::Serve(a) => {
            let session = open_session(Tls {
                endpoint: &a.endpoint,
                cert: a.cert.as_ref(),
                key: a.key.as_ref(),
                root_ca: a.root_ca.as_ref(),
            })
            .await?;
            serve::run(session, a.site, &a.bind).await
        }
    }
}
