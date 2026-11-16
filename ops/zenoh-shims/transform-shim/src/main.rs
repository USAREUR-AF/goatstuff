//! transform-shim binary. The behaviour is in the library (`src/lib.rs`);
//! this file reads the arguments, opens the fabric session and runs it.
//!
//! License: Apache-2.0

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use goat_transformers::Distribution;
use sha2::{Digest, Sha256};
use tracing::info;
use transform_shim::{load_bundle, run, Config, RunOptions, Table};

#[derive(Parser, Debug, Clone)]
#[command(
    name = "transform-shim",
    about = "Fabric peer that converts messages between formats from a declarative route table"
)]
struct Args {
    /// The route table (JSON). See config.example.json.
    #[arg(long, env = "TRANSFORM_SHIM_CONFIG")]
    config: PathBuf,

    /// The pack bundle directory: MANIFEST.json, model/, packs/.
    #[arg(
        long,
        env = "TRANSFORM_SHIM_BUNDLE",
        default_value = "/usr/share/transform-shim/bundle"
    )]
    bundle: PathBuf,

    /// The most restricted marking the engine loads: `public` refuses every
    /// pack and model module marked `controlled`.
    #[arg(
        long,
        env = "TRANSFORM_SHIM_MAX_DISTRIBUTION",
        default_value = "public"
    )]
    max_distribution: String,

    /// Zenoh router endpoint. `tls/...` selects native mTLS; `tcp/...` is
    /// plaintext, for a local test only.
    #[arg(long, env = "TRANSFORM_SHIM_ENDPOINT")]
    endpoint: String,

    /// Client leaf certificate (PEM). Required for a `tls/` endpoint.
    #[arg(long, env = "TRANSFORM_SHIM_CERT")]
    cert: Option<String>,

    /// Client private key (PEM). Required for a `tls/` endpoint.
    #[arg(long, env = "TRANSFORM_SHIM_KEY")]
    key: Option<String>,

    /// Trust root the router's certificate must chain to (PEM). Required
    /// for a `tls/` endpoint.
    #[arg(long, env = "TRANSFORM_SHIM_ROOT_CA")]
    root_ca: Option<String>,

    /// Samples held between the subscribers and the converter. A sample that
    /// arrives when the queue is full is dropped and counted.
    #[arg(long, env = "TRANSFORM_SHIM_QUEUE_DEPTH", default_value_t = 1024)]
    queue_depth: usize,

    /// Seconds between heartbeats.
    #[arg(long, env = "TRANSFORM_SHIM_HEARTBEAT_S", default_value_t = 30)]
    heartbeat_s: u64,

    /// Load the bundle and check the route table, then exit.
    #[arg(long)]
    check: bool,
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

    let max = Distribution::parse(&args.max_distribution).ok_or_else(|| {
        anyhow!(
            "--max-distribution must be public or controlled, got `{}`",
            args.max_distribution
        )
    })?;
    if args.queue_depth == 0 || args.heartbeat_s == 0 {
        bail!("--queue-depth and --heartbeat-s must be at least 1");
    }
    let config_text = std::fs::read_to_string(&args.config)
        .with_context(|| format!("reading {}", args.config.display()))?;
    let cfg = Config::from_json(&config_text)?;
    let formats = load_bundle(&args.bundle, max)?;
    info!(bundle = %args.bundle.display(), library_rev = %formats.library_rev,
          max_distribution = %args.max_distribution,
          formats = ?formats.engine.formats(), "pack bundle loaded");
    let table = Arc::new(Table::build(&cfg, formats)?);
    if args.check {
        println!("ok: {} route(s)", table.routes.len());
        return Ok(());
    }

    let session = open_session(&args).await?;
    info!(endpoint = %args.endpoint, "zenoh session opened");
    run(
        session,
        cfg,
        table,
        RunOptions {
            queue_depth: args.queue_depth,
            heartbeat: Duration::from_secs(args.heartbeat_s),
            config_digest: hex::encode(Sha256::digest(config_text.as_bytes())),
        },
        async {
            let _ = tokio::signal::ctrl_c().await;
        },
    )
    .await
}

/// Open a Zenoh client session. The same block as rest-pub-bridge and
/// synth-vendor: mTLS material is used when the endpoint scheme is `tls/`.
async fn open_session(args: &Args) -> Result<zenoh::Session> {
    let mut config = zenoh::Config::default();
    config
        .insert_json5("mode", "\"client\"")
        .map_err(|e| anyhow!("config mode: {e}"))?;
    config
        .insert_json5("connect/endpoints", &format!("[\"{}\"]", args.endpoint))
        .map_err(|e| anyhow!("config connect/endpoints: {e}"))?;
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
        let tls_block = format!(
            r#"{{ "root_ca_certificate": "{root_ca}", "connect_certificate": "{cert}", "connect_private_key": "{key}", "enable_mtls": true, "verify_name_on_connect": false }}"#
        );
        config
            .insert_json5("transport/link/tls", &tls_block)
            .map_err(|e| anyhow!("config transport/link/tls: {e}"))?;
    }
    zenoh::open(config)
        .await
        .map_err(|e| anyhow!("zenoh::open failed: {e}"))
        .context("opening zenoh session")
}
