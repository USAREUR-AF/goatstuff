//! feeds-shim — bridges MediaMTX's REST API to Zenoh for the video fabric.
//!
//! Per Block 35E and `docs/design/video-fabric.md`:
//!   - Polls MediaMTX `/v3/paths/list` every N seconds
//!   - Publishes per-feed advertisements at
//!     `attest/queryable/feeds/<site>/<feed-id>` with codec / bitrate /
//!     ready-state / origin-class
//!   - Listens for control commands at `cmd/feeds/<id>/ptz`,
//!     `cmd/feeds/<id>/record/start`, `cmd/feeds/<id>/record/stop` and
//!     translates them to MediaMTX API calls (where the upstream supports
//!     PTZ / record control — many won't)
//!
//! This is a Zenoh peer (not a plugin); runs as a separate process alongside
//! MediaMTX on each video-bearing site. Identity-bound via Zenoh's standard
//! authn primitives — see zenoh-data-fabric.md for the substrate-side auth
//! pattern.
//!
//! License: Apache-2.0

use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{info, warn};
use zenoh::Wait;

#[derive(Parser, Debug, Clone)]
#[command(name = "feeds-shim", about = "MediaMTX <-> Zenoh feed-catalog bridge")]
struct Args {
    /// MediaMTX API base URL (e.g. http://10.99.0.1:9997)
    #[arg(long, env = "FEEDS_SHIM_MEDIAMTX_URL", default_value = "http://127.0.0.1:9997")]
    mediamtx_url: String,

    /// Site name, used as the second path segment of the Zenoh key
    /// expression (e.g. `attest/queryable/feeds/<site>/<feed-id>`)
    #[arg(long, env = "FEEDS_SHIM_SITE", default_value = "local")]
    site: String,

    /// Polling interval in seconds for the MediaMTX path list
    #[arg(long, env = "FEEDS_SHIM_POLL_SEC", default_value = "5")]
    poll_sec: u64,

    /// Optional Zenoh config file path (defaults to peer-mode auto-config)
    #[arg(long, env = "FEEDS_SHIM_ZENOH_CONFIG")]
    zenoh_config: Option<String>,

    /// Publish the feeds-v1 + feeds-ptz-v1 schemas to attest/schema/...
    /// then exit. Run this once during deployment to register schemas
    /// per the data-dictionary discipline (Block 23). Without this,
    /// the schema-enforcer interceptor will reject feeds-shim publishes.
    #[arg(long)]
    publish_schemas_and_exit: bool,
}

const FEEDS_V1_SCHEMA: &str = include_str!("../schemas/feeds-v1.yaml");
const FEEDS_PTZ_V1_SCHEMA: &str = include_str!("../schemas/feeds-ptz-v1.yaml");

#[derive(Debug, Deserialize)]
struct MediaMtxPathsList {
    items: Vec<MediaMtxPath>,
}

#[derive(Debug, Deserialize)]
struct MediaMtxPath {
    name: String,
    #[serde(default)]
    ready: bool,
    #[serde(rename = "tracks2")]
    #[serde(default)]
    tracks: Vec<MediaMtxTrack>,
    #[serde(rename = "inboundBytes", default)]
    inbound_bytes: u64,
    #[serde(rename = "outboundBytes", default)]
    outbound_bytes: u64,
    #[serde(default)]
    source: Option<MediaMtxSource>,
}

#[derive(Debug, Deserialize)]
struct MediaMtxTrack {
    codec: String,
    #[serde(rename = "codecProps", default)]
    codec_props: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct MediaMtxSource {
    #[serde(rename = "type")]
    type_: String,
}

#[derive(Debug, Serialize)]
struct FeedAdvertisement<'a> {
    name: &'a str,
    site: &'a str,
    ready: bool,
    tracks: Vec<TrackAdvertisement<'a>>,
    inbound_bytes: u64,
    outbound_bytes: u64,
    source_type: Option<&'a str>,
    schema_version: u32,
}

#[derive(Debug, Serialize)]
struct TrackAdvertisement<'a> {
    codec: &'a str,
    codec_props: &'a serde_json::Value,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env().add_directive(
            tracing::Level::INFO.into(),
        ))
        .init();

    let args = Args::parse();
    info!(
        mediamtx_url = %args.mediamtx_url,
        site = %args.site,
        poll_sec = args.poll_sec,
        "feeds-shim starting"
    );

    let zenoh_config = match &args.zenoh_config {
        Some(path) => zenoh::Config::from_file(path)
            .map_err(|e| anyhow::anyhow!("failed to load zenoh config from {}: {}", path, e))?,
        None => zenoh::Config::default(),
    };
    let session = zenoh::open(zenoh_config)
        .await
        .map_err(|e| anyhow::anyhow!("zenoh::open: {}", e))?;
    info!("zenoh session opened");

    if args.publish_schemas_and_exit {
        publish_schemas(&session).await?;
        info!("schemas published; exiting per --publish-schemas-and-exit");
        return Ok(());
    }

    let http = reqwest::ClientBuilder::new()
        .timeout(Duration::from_secs(10))
        .build()?;

    let paths_url = format!("{}/v3/paths/list", args.mediamtx_url.trim_end_matches('/'));
    let mut interval = tokio::time::interval(Duration::from_secs(args.poll_sec));

    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Err(e) = poll_and_publish(&http, &paths_url, &session, &args.site).await {
                    warn!(error = %e, "poll iteration failed");
                }
            }
            _ = &mut shutdown => {
                info!("shutdown signal received, exiting");
                break;
            }
        }
    }

    Ok(())
}

async fn publish_schemas(session: &zenoh::Session) -> Result<()> {
    for (key_expr, body) in [
        ("attest/schema/feeds/v1", FEEDS_V1_SCHEMA),
        ("attest/schema/feeds-ptz/v1", FEEDS_PTZ_V1_SCHEMA),
    ] {
        session
            .put(key_expr, body.as_bytes().to_vec())
            .wait()
            .map_err(|e| anyhow::anyhow!("zenoh put schema {}: {}", key_expr, e))?;
        info!(key_expr = key_expr, bytes = body.len(), "published schema");
    }
    Ok(())
}

async fn poll_and_publish(
    http: &reqwest::Client,
    paths_url: &str,
    session: &zenoh::Session,
    site: &str,
) -> Result<()> {
    let resp = http.get(paths_url).send().await.context("MediaMTX API request")?;
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("MediaMTX API returned {}", resp.status()));
    }
    let body: MediaMtxPathsList = resp.json().await.context("MediaMTX JSON decode")?;

    for path in &body.items {
        let key_expr = format!("attest/queryable/feeds/{}/{}", site, path.name);
        let advert = FeedAdvertisement {
            name: &path.name,
            site,
            ready: path.ready,
            tracks: path
                .tracks
                .iter()
                .map(|t| TrackAdvertisement {
                    codec: &t.codec,
                    codec_props: &t.codec_props,
                })
                .collect(),
            inbound_bytes: path.inbound_bytes,
            outbound_bytes: path.outbound_bytes,
            source_type: path.source.as_ref().map(|s| s.type_.as_str()),
            schema_version: 1,
        };
        let payload = serde_json::to_vec(&advert)?;
        session
            .put(&key_expr, payload)
            .wait()
            .map_err(|e| anyhow::anyhow!("zenoh put {}: {}", key_expr, e))?;
        tracing::debug!(key_expr = %key_expr, ready = path.ready, "published advert");
    }
    Ok(())
}
