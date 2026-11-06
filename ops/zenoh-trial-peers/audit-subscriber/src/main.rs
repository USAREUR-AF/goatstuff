// SPDX-License-Identifier: Apache-2.0
//
// goat-audit-subscriber — external Zenoh audit subscriber.
//
// Replaces the in-process audit-logger plugin per:
//   - docs/design/zenoh-audit-external-subscriber.md
//   - ADR 0858 (zenoh image upstream pivot enabled by this move)
//   - ADR 0811 (audit log integrity → MinIO Object-Lock COMPLIANCE 1y)
//
// Shape:
//   - Opens an mTLS Zenoh session as a client peer of the audited router.
//   - Declares a subscriber on the configured key-expression (default **).
//   - Forwards each sample through a bounded mpsc channel to a writer
//     task that batches events into NDJSON objects and ships them to a
//     MinIO bucket (S3-compatible).
//   - Exposes Prometheus /metrics on a separate HTTP port.
//
// Event schema is `AuditEvent` below — matches the design doc shape
// (vendor, key_expr, ts, src_zid, payload_hash, payload_len,
// payload_bytes?, audit_subscriber_id). Payload bytes are captured
// inline up to --max-inline-payload; oversize publishes carry the hash
// + length only.

use anyhow::{Context, Result};
use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use aws_sdk_s3::config::Region;
use aws_sdk_s3::primitives::ByteStream;
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use chrono::{DateTime, SecondsFormat, Utc};
use clap::Parser;
use prometheus::{
    register_counter_vec_with_registry, register_gauge_vec_with_registry,
    register_int_counter_with_registry, register_int_gauge_with_registry, CounterVec, Encoder,
    GaugeVec, IntCounter, IntGauge, Registry, TextEncoder,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use zenoh::key_expr::KeyExpr;

#[derive(Parser, Debug)]
#[command(
    name = "goat-audit-subscriber",
    about = "External Zenoh audit subscriber. Block 82S per docs/design/zenoh-audit-external-subscriber.md."
)]
struct Args {
    /// Zenoh router endpoint, e.g. `tls/zenoh-router:7447`.
    #[arg(long, env = "AUDIT_SUBSCRIBER_ENDPOINT")]
    endpoint: String,

    /// Key-expression to subscribe to. Default `**` captures every
    /// sample the router routes to this client; narrow per-prefix when
    /// the trial fabric ACL gets tighter.
    #[arg(long, env = "AUDIT_SUBSCRIBER_KEY", default_value = "**")]
    subscribe_key: String,

    /// Comma-separated key-expressions to EXCLUDE from the audit /
    /// persistence path. Samples whose key intersects any of these are
    /// counted in `goat_audit_excluded_total` and dropped before hashing
    /// + upload — they never reach the MinIO bucket. The subscription
    /// stays `**` (future-proof against new top-level classes); the
    /// filter is applied client-side. Default `volatile/**` per ADR 1032:
    /// the non-persisted class (high-throughput telemetry + bounded
    /// large-file transfer) must not fill the locked audit store. Set to
    /// an empty string to capture everything (legacy behaviour).
    #[arg(long, env = "AUDIT_SUBSCRIBER_EXCLUDE_KEYS", default_value = "volatile/**")]
    exclude_keys: String,

    /// Stable identifier for this subscriber's writes (e.g. `site-a`).
    /// Becomes a path component in MinIO object keys + a label on
    /// emitted metrics.
    #[arg(long, env = "AUDIT_SUBSCRIBER_ID")]
    audit_subscriber_id: String,

    /// Identity model of the audited goatnet (ADR 1025). `per-vendor`
    /// (the first key segment is a human vendor prefix) or
    /// `per-uuid-slot-pool` (the first segment is an opaque slot
    /// anchor uuid). Recorded verbatim on every audit event as
    /// `identity_model` so a downstream reader knows whether the
    /// `anchor`/`vendor` field is a human name or a uuid to be joined
    /// against the slot registry's assigned handle. Default `per-vendor`
    /// for compatibility.
    #[arg(
        long,
        env = "AUDIT_SUBSCRIBER_IDENTITY_MODEL",
        default_value = "per-vendor"
    )]
    identity_model: String,

    /// Path to the per-instance leaf X.509 certificate (PEM) for mTLS.
    /// Required when `endpoint` starts with `tls/`.
    #[arg(long, env = "AUDIT_SUBSCRIBER_CERT")]
    cert: Option<String>,

    /// Path to the matching private key (PEM). Required when
    /// `endpoint` starts with `tls/`.
    #[arg(long, env = "AUDIT_SUBSCRIBER_KEY_FILE")]
    key: Option<String>,

    /// Path to the CA root certificate (PEM) used to verify the
    /// router's server cert. Required when `endpoint` starts with
    /// `tls/`.
    #[arg(long, env = "AUDIT_SUBSCRIBER_ROOT_CA")]
    root_ca: Option<String>,

    /// Bind address for the Prometheus /metrics endpoint.
    #[arg(
        long,
        env = "AUDIT_SUBSCRIBER_METRICS_BIND",
        default_value = "0.0.0.0:9303"
    )]
    metrics_bind: SocketAddr,

    /// MinIO / S3 endpoint URL (e.g. `https://minio.site-a.local:9000`).
    /// When set, audit events are uploaded as NDJSON batches; when
    /// unset, the subscriber runs in observe-only mode (metrics still
    /// emit, no durable write — useful for first-deploy verification).
    #[arg(long, env = "AUDIT_SUBSCRIBER_S3_ENDPOINT")]
    s3_endpoint: Option<String>,

    /// MinIO / S3 bucket name. Per ADR 0811 the production target is a
    /// bucket with Object-Lock COMPLIANCE 1y.
    #[arg(
        long,
        env = "AUDIT_SUBSCRIBER_S3_BUCKET",
        default_value = "audit-integrity"
    )]
    s3_bucket: String,

    /// MinIO / S3 region. MinIO ignores the value but the SDK requires
    /// one to construct the client.
    #[arg(
        long,
        env = "AUDIT_SUBSCRIBER_S3_REGION",
        default_value = "us-east-1"
    )]
    s3_region: String,

    /// Environment variable name holding the S3 access key. The value
    /// is read from the env at startup so the secret never appears in
    /// process listings.
    #[arg(
        long,
        env = "AUDIT_SUBSCRIBER_S3_ACCESS_KEY_ENV",
        default_value = "AUDIT_SUBSCRIBER_S3_ACCESS_KEY"
    )]
    s3_access_key_env: String,

    /// Environment variable name holding the S3 secret key.
    #[arg(
        long,
        env = "AUDIT_SUBSCRIBER_S3_SECRET_KEY_ENV",
        default_value = "AUDIT_SUBSCRIBER_S3_SECRET_KEY"
    )]
    s3_secret_key_env: String,

    /// Cap on inline payload bytes captured per event. Publishes
    /// larger than this carry hash + length only. Default 16 KiB per
    /// the design-doc open question (revisit when we have trial-fabric
    /// payload-size distribution evidence).
    #[arg(
        long,
        env = "AUDIT_SUBSCRIBER_MAX_INLINE_PAYLOAD",
        default_value_t = 16384
    )]
    max_inline_payload: usize,

    /// Maximum events per uploaded NDJSON batch.
    ///
    /// This is a BURST VALVE, not the normal flush trigger. It must sit well
    /// above `rate x batch_interval_ms` or it binds first and the interval
    /// becomes decorative — that is exactly what 256 did against a 5 s
    /// interval at tens of events/s. 10000 leaves headroom over that
    /// rate and bounds writer memory at about 6.5 MB held.
    #[arg(long, env = "AUDIT_SUBSCRIBER_BATCH_EVENTS", default_value_t = 10_000)]
    batch_events: usize,

    /// Maximum batch flush interval. The writer flushes when either
    /// `batch_events` accumulates OR this much time elapses since the
    /// batch started, whichever fires first.
    ///
    /// ONE FLUSH IS ONE S3 OBJECT, and MinIO's scanner reads one `xl.meta`
    /// per object on every pass — so this value, not the byte volume, sets
    /// the store's scan cost. A 5 s cadence accumulates hundreds of thousands of
    /// objects and millions of inodes on the host root filesystem. At 60 s the
    /// same bytes land in about a twelfth as many objects.
    ///
    /// IT IS ALSO THE DATA-LOSS WINDOW. Between flushes the batch is in this
    /// process's memory and nowhere else — the local NDJSON tail is appended
    /// inside `flush_batch`, not continuously — so a crash loses up to this
    /// much audit outright, not merely un-WORMed. 60 s was chosen by the
    /// operator with that stated.
    #[arg(long, env = "AUDIT_SUBSCRIBER_BATCH_INTERVAL_MS", default_value_t = 60_000)]
    batch_interval_ms: u64,

    /// Optional local directory to ALSO append each NDJSON batch to (a
    /// best-effort tail — the durable MinIO write is unaffected on failure).
    /// Off-box cyber-logging (goat-logship, ADR 1078) tails this to surface
    /// fabric writes as OCSF; the MinIO WORM store stays the system-of-record.
    #[arg(long, env = "AUDIT_SUBSCRIBER_LOCAL_NDJSON_DIR")]
    local_ndjson_dir: Option<String>,

    /// Bounded channel depth between subscriber and writer. Sized for
    /// burst tolerance; if the writer falls behind by more than this,
    /// `goat_audit_drop_total` increments and the publish is dropped
    /// from the audit log (fail-loud, not fail-silent).
    #[arg(long, env = "AUDIT_SUBSCRIBER_CHANNEL_DEPTH", default_value_t = 4096)]
    channel_depth: usize,
}

/// The two identity models an audited goatnet can run (ADR 1025).
const VALID_IDENTITY_MODELS: [&str; 2] = ["per-vendor", "per-uuid-slot-pool"];

/// Backoff between zenoh session-open attempts. Connection refused at
/// boot (router not up yet) is the common case; 5s keeps the audit gap
/// small without hammering the router.
const SESSION_OPEN_RETRY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize)]
struct AuditEvent {
    /// Enforced write-anchor — the first segment of the key-expression,
    /// which is the identity the router's ACL bound the write to. Shape-
    /// neutral: a human vendor prefix under `per-vendor`, an opaque slot-
    /// anchor uuid under `per-uuid-slot-pool` (ADR 1025). New consumers
    /// read this together with `identity_model`.
    anchor: String,
    /// DEPRECATED back-compat alias of `anchor` — identical value. The
    /// in-portal audit tailer keys on `vendor`, so it stays emitted; new
    /// consumers should prefer `anchor` + `identity_model`.
    vendor: String,
    /// Identity model that produced this record (`per-vendor` |
    /// `per-uuid-slot-pool`) — tells a reader how to interpret `anchor`.
    identity_model: String,
    key_expr: String,
    /// ISO-8601 UTC, millisecond precision.
    ts: String,
    /// Source zenoh-id of the publisher (when zenoh exposes it; empty
    /// string when not available).
    ///
    /// **ASSERTED BY THE PUBLISHER, VERIFIED BY NOTHING.**
    ///
    /// This is `ext_sinfo` as it appeared on the wire. A forwarding hop rewrites
    /// only `wire_expr` and `ext_nodeid`, so it crosses a federation boundary
    /// unchanged, and no code compares it to the transport peer that delivered
    /// the sample. A peered party can claim any ZID, including one of ours.
    ///
    /// The audit trail therefore records a CLAIM here, not a fact. For anything
    /// that crossed a trust boundary, attribute on the ingress key prefix and the
    /// peering certificate CN — the only authenticated facts available.
    /// See `docs/design/cross-sovereign-federation.md` §6 and ADR 1152.
    src_zid: String,
    /// `sha256:<hex>` over the full payload bytes.
    payload_hash: String,
    payload_len: usize,
    /// Base64-encoded payload, present only when len ≤
    /// `max_inline_payload`. Null otherwise — the hash + len still
    /// pin the value structurally.
    #[serde(skip_serializing_if = "Option::is_none")]
    payload_bytes_b64: Option<String>,
    audit_subscriber_id: String,
}

#[derive(Clone)]
struct Metrics {
    samples_total: CounterVec,
    sample_bytes: CounterVec,
    excluded_total: CounterVec,
    write_errors_total: IntCounter,
    drop_total: IntCounter,
    batches_uploaded_total: IntCounter,
    batch_size_events: GaugeVec,
    session_state: IntGauge,
    registry: Registry,
}

impl Metrics {
    fn new(audit_subscriber_id: &str) -> Result<Self> {
        let registry = Registry::new();
        let labels = [("audit_subscriber_id", audit_subscriber_id)];
        let const_labels: std::collections::HashMap<String, String> = labels
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();

        let samples_total = register_counter_vec_with_registry!(
            prometheus::Opts::new(
                "goat_audit_samples_total",
                "Audit samples observed by this subscriber, by vendor + key-pattern."
            )
            .const_labels(const_labels.clone()),
            &["vendor", "key_pattern"],
            registry
        )?;
        let sample_bytes = register_counter_vec_with_registry!(
            prometheus::Opts::new(
                "goat_audit_sample_bytes_total",
                "Total payload bytes observed, by vendor."
            )
            .const_labels(const_labels.clone()),
            &["vendor"],
            registry
        )?;
        let excluded_total = register_counter_vec_with_registry!(
            prometheus::Opts::new(
                "goat_audit_excluded_total",
                "Samples deliberately dropped before the persistence path \
                 because their key matched an --exclude-key pattern (e.g. \
                 the non-persisted volatile/** class, ADR 1032). Makes the \
                 audit exemption observable rather than silent."
            )
            .const_labels(const_labels.clone()),
            &["exclude_pattern"],
            registry
        )?;
        let write_errors_total = register_int_counter_with_registry!(
            prometheus::Opts::new(
                "goat_audit_write_errors_total",
                "S3 PutObject failures (post-retry)."
            )
            .const_labels(const_labels.clone()),
            registry
        )?;
        let drop_total = register_int_counter_with_registry!(
            prometheus::Opts::new(
                "goat_audit_drop_total",
                "Samples dropped because the writer channel was full."
            )
            .const_labels(const_labels.clone()),
            registry
        )?;
        let batches_uploaded_total = register_int_counter_with_registry!(
            prometheus::Opts::new(
                "goat_audit_batches_uploaded_total",
                "NDJSON batches successfully uploaded to S3."
            )
            .const_labels(const_labels.clone()),
            registry
        )?;
        let batch_size_events = register_gauge_vec_with_registry!(
            prometheus::Opts::new(
                "goat_audit_batch_size_events",
                "Event count of the most recently uploaded batch."
            )
            .const_labels(const_labels.clone()),
            &["outcome"],
            registry
        )?;
        let session_state = register_int_gauge_with_registry!(
            prometheus::Opts::new(
                "goat_audit_subscriber_session_state",
                "1 when the zenoh session declared its subscriber, 0 otherwise."
            )
            .const_labels(const_labels.clone()),
            registry
        )?;

        Ok(Self {
            samples_total,
            sample_bytes,
            excluded_total,
            write_errors_total,
            drop_total,
            batches_uploaded_total,
            batch_size_events,
            session_state,
            registry,
        })
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

    let args = Args::parse();

    // Validate the identity model up front (ADR 1025) — a typo here would
    // mislabel every audit record's identity_model, so fail loud at startup.
    if !VALID_IDENTITY_MODELS.contains(&args.identity_model.as_str()) {
        anyhow::bail!(
            "--identity-model {:?} invalid; must be one of {:?}",
            args.identity_model,
            VALID_IDENTITY_MODELS
        );
    }

    let metrics = Metrics::new(&args.audit_subscriber_id)?;

    // Serve /metrics. Must be ready before zenoh open so Prometheus
    // scraping picks up the `session_state=0` pre-connect signal.
    let metrics_for_http = metrics.clone();
    let metrics_bind = args.metrics_bind;
    tokio::spawn(async move {
        if let Err(e) = serve_metrics(metrics_for_http, metrics_bind).await {
            error!(error = %e, "metrics server exited");
        }
    });

    let (tx, rx) = mpsc::channel::<AuditEvent>(args.channel_depth);

    // Writer task — owns the S3 client (or none, for observe-only mode).
    let writer_metrics = metrics.clone();
    let writer_args = WriterArgs {
        s3_endpoint: args.s3_endpoint.clone(),
        s3_bucket: args.s3_bucket.clone(),
        s3_region: args.s3_region.clone(),
        s3_access_key: read_env_opt(&args.s3_access_key_env),
        s3_secret_key: read_env_opt(&args.s3_secret_key_env),
        audit_subscriber_id: args.audit_subscriber_id.clone(),
        batch_events: args.batch_events,
        batch_interval: Duration::from_millis(args.batch_interval_ms),
        local_ndjson_dir: args.local_ndjson_dir.clone(),
    };
    let writer_handle = tokio::spawn(async move {
        if let Err(e) = run_writer(writer_args, writer_metrics, rx).await {
            error!(error = %e, "writer task exited");
        }
    });

    // Parse the audit exclude filter (ADR 1032). Done up front so a
    // malformed key-expression fails the process at startup rather than
    // silently capturing the class we meant to drop.
    let exclude_kes = parse_exclude_keys(&args.exclude_keys)?;
    if exclude_kes.is_empty() {
        info!("audit exclude filter: none (capturing all routed keys)");
    } else {
        let patterns: Vec<&str> = exclude_kes.iter().map(|k| k.as_str()).collect();
        info!(?patterns, "audit exclude filter active — matching samples dropped before persistence");
    }

    // Subscriber loop — opens the zenoh session and forwards samples.
    let subscriber_metrics = metrics.clone();
    let subscriber_args = SubscriberArgs {
        endpoint: args.endpoint.clone(),
        subscribe_key: args.subscribe_key.clone(),
        cert: args.cert.clone(),
        key: args.key.clone(),
        root_ca: args.root_ca.clone(),
        audit_subscriber_id: args.audit_subscriber_id.clone(),
        identity_model: args.identity_model.clone(),
        max_inline_payload: args.max_inline_payload,
        exclude_kes,
    };
    let mut subscriber_handle = tokio::spawn(async move {
        if let Err(e) = run_subscriber(subscriber_args, subscriber_metrics, tx).await {
            error!(error = %e, "subscriber task exited");
        }
    });

    // The subscriber task ending for ANY reason means the process is no
    // longer auditing — exit non-zero so the container restart policy
    // brings up a fresh process, instead of idling as a zombie whose
    // metrics listener keeps the container looking healthy. A router that comes
    // up after us otherwise means one failed session open and a silent audit gap.
    tokio::select! {
        r = tokio::signal::ctrl_c() => {
            r.context("install ctrl_c handler")?;
            info!("shutdown signal received");
            subscriber_handle.abort();
            drop(subscriber_handle);
            // Writer keeps draining until the channel closes — that happens
            // when `tx` is dropped, which happens when the subscriber task is
            // aborted and its tx clone falls out of scope.
            let _ = tokio::time::timeout(Duration::from_secs(10), writer_handle).await;
            Ok(())
        }
        _ = &mut subscriber_handle => {
            // Give the writer a moment to flush any batched events before
            // dying — the channel is closed (subscriber's tx dropped).
            let _ = tokio::time::timeout(Duration::from_secs(10), writer_handle).await;
            anyhow::bail!("subscriber task ended — exiting so the container restarts");
        }
    }
}

fn read_env_opt(var_name: &str) -> Option<String> {
    std::env::var(var_name).ok().filter(|v| !v.is_empty())
}

/// Parse the comma-separated `--exclude-keys` string into owned
/// key-expressions. Empty / whitespace-only entries are ignored, so
/// `""` yields no filter (capture everything). A malformed entry is a
/// hard startup error — better to fail loud than silently capture a
/// class an operator meant to exclude.
fn parse_exclude_keys(raw: &str) -> Result<Vec<KeyExpr<'static>>> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            KeyExpr::new(s.to_string())
                .map(|ke| ke.into_owned())
                .map_err(|e| anyhow::anyhow!("invalid --exclude-keys entry {s:?}: {e}"))
        })
        .collect()
}

async fn serve_metrics(metrics: Metrics, bind: SocketAddr) -> Result<()> {
    let registry = Arc::new(metrics.registry);
    let app = Router::new().route(
        "/metrics",
        get(move || {
            let registry = registry.clone();
            async move {
                let metric_families = registry.gather();
                let mut buf = Vec::new();
                let encoder = TextEncoder::new();
                if let Err(e) = encoder.encode(&metric_families, &mut buf) {
                    return (StatusCode::INTERNAL_SERVER_ERROR, format!("encode: {e}"));
                }
                let body = String::from_utf8(buf).unwrap_or_default();
                (StatusCode::OK, body)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind(bind).await?;
    info!(%bind, "metrics listening");
    axum::serve(listener, app).await?;
    Ok(())
}

struct SubscriberArgs {
    endpoint: String,
    subscribe_key: String,
    cert: Option<String>,
    key: Option<String>,
    root_ca: Option<String>,
    audit_subscriber_id: String,
    identity_model: String,
    max_inline_payload: usize,
    exclude_kes: Vec<KeyExpr<'static>>,
}

async fn run_subscriber(
    args: SubscriberArgs,
    metrics: Metrics,
    tx: mpsc::Sender<AuditEvent>,
) -> Result<()> {
    // Retry the initial open forever: at host boot the router routinely
    // comes up after us, and a one-shot open turned that ordering into a
    // dead subscriber. `session_state` stays 0 while retrying, so the
    // outage window is visible on the existing metric. Once open, zenoh
    // handles link re-establishment itself.
    let session = loop {
        match open_session(&args).await {
            Ok(s) => break s,
            Err(e) => {
                warn!(
                    error = %e,
                    retry_in_s = SESSION_OPEN_RETRY.as_secs(),
                    "zenoh session open failed; retrying"
                );
                tokio::time::sleep(SESSION_OPEN_RETRY).await;
            }
        }
    };
    info!(
        zid = %session.zid(),
        endpoint = args.endpoint.as_str(),
        subscribe_key = args.subscribe_key.as_str(),
        "session open",
    );

    let subscriber = session
        .declare_subscriber(args.subscribe_key.clone())
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "declare_subscriber({}) failed: {e}",
                args.subscribe_key.as_str()
            )
        })?;
    metrics.session_state.set(1);

    loop {
        let sample = match subscriber.recv_async().await {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "subscriber recv error; loop ending");
                metrics.session_state.set(0);
                break;
            }
        };

        // Audit exclude filter (ADR 1032): drop the non-persisted classes
        // (default volatile/**) before any hashing / channel / upload
        // work, so the high-throughput firehose never reaches the locked
        // MinIO bucket. Counted per matched pattern so the exemption is
        // observable rather than silent.
        if let Some(pat) = args
            .exclude_kes
            .iter()
            .find(|ke| ke.intersects(sample.key_expr()))
        {
            metrics
                .excluded_total
                .with_label_values(&[pat.as_str()])
                .inc();
            continue;
        }

        let key_expr = sample.key_expr().to_string();
        // The enforced write-anchor is the first key segment in BOTH identity
        // models (ADR 1025): a vendor prefix under per-vendor, an opaque slot
        // uuid under per-uuid-slot-pool. The `vendor` field/label keeps this
        // value for back-compat; `identity_model` disambiguates downstream.
        let anchor = anchor_of_key(&key_expr).to_string();
        let key_pattern = key_pattern_label(&key_expr);
        let payload_bytes = sample.payload().to_bytes().to_vec();
        let payload_len = payload_bytes.len();
        let mut hasher = Sha256::new();
        hasher.update(&payload_bytes);
        let payload_hash = format!("sha256:{}", hex::encode(hasher.finalize()));
        let payload_bytes_b64 = if payload_len <= args.max_inline_payload {
            Some(BASE64_STANDARD.encode(&payload_bytes))
        } else {
            None
        };
        let event = AuditEvent {
            anchor: anchor.clone(),
            vendor: anchor.clone(),
            identity_model: args.identity_model.clone(),
            key_expr: key_expr.clone(),
            ts: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            src_zid: String::new(),
            payload_hash,
            payload_len,
            payload_bytes_b64,
            audit_subscriber_id: args.audit_subscriber_id.clone(),
        };

        metrics
            .samples_total
            .with_label_values(&[&anchor, &key_pattern])
            .inc();
        metrics
            .sample_bytes
            .with_label_values(&[&anchor])
            .inc_by(payload_len as f64);

        match tx.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                metrics.drop_total.inc();
                warn!(
                    key_expr = key_expr.as_str(),
                    "writer channel full; dropping sample"
                );
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                warn!("writer channel closed; subscriber stopping");
                break;
            }
        }
    }
    Ok(())
}

/// The enforced write-anchor of a key-expression: its first `/`-segment.
/// This is the ACL-bound identity in BOTH identity models (ADR 1025) — a
/// vendor prefix under `per-vendor`, a slot-anchor uuid under
/// `per-uuid-slot-pool`. The taxonomy (ADR 0844) nests verbatim below it,
/// so this single rule serves both shapes.
fn anchor_of_key(key_expr: &str) -> &str {
    key_expr.split('/').next().unwrap_or("")
}

/// Reduce a key-expression to a stable label suitable for a Prometheus
/// `key_pattern` dimension. Replaces all but the first two path
/// segments with `*` so cardinality stays bounded.
fn key_pattern_label(key_expr: &str) -> String {
    let segments: Vec<&str> = key_expr.split('/').collect();
    match segments.len() {
        0 => String::new(),
        1 => segments[0].to_string(),
        _ => format!("{}/{}/*", segments[0], segments[1]),
    }
}

async fn open_session(args: &SubscriberArgs) -> Result<zenoh::Session> {
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
        let cert = args.cert.as_ref().context("--cert required for tls/ endpoint")?;
        let key = args.key.as_ref().context("--key required for tls/ endpoint")?;
        let root_ca = args
            .root_ca
            .as_ref()
            .context("--root-ca required for tls/ endpoint")?;
        // Same shape as synth-vendor + panoscope: TLS is selected by
        // the endpoint scheme; turn off hostname verification because
        // the router cert SAN binds the mesh IP, not the docker DNS
        // name we connect through.
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

struct WriterArgs {
    s3_endpoint: Option<String>,
    s3_bucket: String,
    s3_region: String,
    s3_access_key: Option<String>,
    s3_secret_key: Option<String>,
    audit_subscriber_id: String,
    batch_events: usize,
    batch_interval: Duration,
    local_ndjson_dir: Option<String>,
}

async fn run_writer(
    args: WriterArgs,
    metrics: Metrics,
    mut rx: mpsc::Receiver<AuditEvent>,
) -> Result<()> {
    let client = build_s3_client(&args).await?;
    if client.is_none() {
        info!("observe-only mode (no S3 endpoint configured); events logged but not uploaded");
    }

    let mut batch: Vec<AuditEvent> = Vec::with_capacity(args.batch_events);
    let mut deadline = tokio::time::Instant::now() + args.batch_interval;

    loop {
        let now = tokio::time::Instant::now();
        let sleep = tokio::time::sleep_until(deadline);
        tokio::pin!(sleep);

        tokio::select! {
            maybe_event = rx.recv() => {
                match maybe_event {
                    Some(event) => {
                        batch.push(event);
                        if batch.is_empty() {
                            deadline = tokio::time::Instant::now() + args.batch_interval;
                        }
                        if batch.len() >= args.batch_events {
                            flush_batch(&client, &args, &metrics, &mut batch).await;
                            deadline = tokio::time::Instant::now() + args.batch_interval;
                        }
                    }
                    None => {
                        // Channel closed — final flush.
                        if !batch.is_empty() {
                            flush_batch(&client, &args, &metrics, &mut batch).await;
                        }
                        break;
                    }
                }
            }
            _ = &mut sleep => {
                if !batch.is_empty() {
                    flush_batch(&client, &args, &metrics, &mut batch).await;
                }
                deadline = now + args.batch_interval;
            }
        }
    }
    Ok(())
}

async fn build_s3_client(args: &WriterArgs) -> Result<Option<aws_sdk_s3::Client>> {
    let endpoint = match &args.s3_endpoint {
        Some(e) => e,
        None => return Ok(None),
    };
    let access = args
        .s3_access_key
        .clone()
        .context("S3 endpoint set but access key env var unset / empty")?;
    let secret = args
        .s3_secret_key
        .clone()
        .context("S3 endpoint set but secret key env var unset / empty")?;
    let creds = Credentials::new(access, secret, None, None, "audit-subscriber");
    let region = Region::new(args.s3_region.clone());
    let conf = aws_config::defaults(BehaviorVersion::latest())
        .region(region)
        .credentials_provider(creds)
        .endpoint_url(endpoint)
        .load()
        .await;
    let s3_conf = aws_sdk_s3::config::Builder::from(&conf)
        .force_path_style(true)
        .build();
    Ok(Some(aws_sdk_s3::Client::from_conf(s3_conf)))
}

async fn flush_batch(
    client: &Option<aws_sdk_s3::Client>,
    args: &WriterArgs,
    metrics: &Metrics,
    batch: &mut Vec<AuditEvent>,
) {
    if batch.is_empty() {
        return;
    }
    let count = batch.len();
    let now: DateTime<Utc> = Utc::now();
    let key = format!(
        "{}/{:04}/{:02}/{:02}/{:02}/{}-{}.ndjson",
        args.audit_subscriber_id,
        now.format("%Y").to_string().parse::<u32>().unwrap_or(0),
        now.format("%m").to_string().parse::<u32>().unwrap_or(0),
        now.format("%d").to_string().parse::<u32>().unwrap_or(0),
        now.format("%H").to_string().parse::<u32>().unwrap_or(0),
        now.timestamp_millis(),
        count
    );

    let mut buf = Vec::with_capacity(count * 256);
    for event in batch.drain(..) {
        match serde_json::to_vec(&event) {
            Ok(bytes) => {
                buf.extend_from_slice(&bytes);
                buf.push(b'\n');
            }
            Err(e) => {
                warn!(error = %e, "serde_json failed for event; skipping");
            }
        }
    }

    // Best-effort local tail for off-box cyber-logging (goat-logship, ADR 1078).
    // A local-write failure must NEVER affect the durable MinIO path below.
    if let Some(dir) = args.local_ndjson_dir.as_deref() {
        let local_path = format!("{}/{}-{}.ndjson", dir, args.audit_subscriber_id, now.format("%Y-%m-%d"));
        match std::fs::OpenOptions::new().create(true).append(true).open(&local_path) {
            Ok(mut f) => {
                use std::io::Write;
                if let Err(e) = f.write_all(&buf) {
                    warn!(error = %e, path = local_path.as_str(), "local ndjson tail append failed (durable path unaffected)");
                }
            }
            Err(e) => {
                warn!(error = %e, path = local_path.as_str(), "local ndjson tail open failed (durable path unaffected)");
            }
        }
    }

    let Some(client) = client else {
        info!(events = count, bytes = buf.len(), "observe-only flush");
        metrics
            .batch_size_events
            .with_label_values(&["observe_only"])
            .set(count as f64);
        return;
    };

    let body = ByteStream::from(buf);
    match client
        .put_object()
        .bucket(&args.s3_bucket)
        .key(&key)
        .body(body)
        .content_type("application/x-ndjson")
        .send()
        .await
    {
        Ok(_) => {
            metrics.batches_uploaded_total.inc();
            metrics
                .batch_size_events
                .with_label_values(&["uploaded"])
                .set(count as f64);
            info!(bucket = args.s3_bucket.as_str(), key = key.as_str(), events = count, "batch uploaded");
        }
        Err(e) => {
            metrics.write_errors_total.inc();
            metrics
                .batch_size_events
                .with_label_values(&["error"])
                .set(count as f64);
            error!(error = %e, bucket = args.s3_bucket.as_str(), key = key.as_str(), events = count, "S3 put_object failed");
        }
    }
}

#[cfg(test)]
mod s3_tls_tests;

#[cfg(test)]
mod tests {
    use super::*;

    // The anchor is the first segment in BOTH identity models — this is what
    // makes the audit path dual-shape without branching on the model.
    #[test]
    fn anchor_is_first_segment_in_both_shapes() {
        // per-vendor: human prefix.
        assert_eq!(anchor_of_key("vendor-b/sensorgrid/track/v1"), "vendor-b");
        // per-uuid-slot-pool: opaque slot anchor uuid + nested taxonomy.
        assert_eq!(
            anchor_of_key("61dc1e84ca2c7cfea827728a0f4bfa4d/data/track/site/v1"),
            "61dc1e84ca2c7cfea827728a0f4bfa4d"
        );
        assert_eq!(anchor_of_key("solo"), "solo");
        assert_eq!(anchor_of_key(""), "");
    }

    #[test]
    fn key_pattern_label_bounds_cardinality() {
        assert_eq!(key_pattern_label("v/s/a/b/c"), "v/s/*");
        assert_eq!(key_pattern_label("v/s"), "v/s/*");
        assert_eq!(key_pattern_label("v"), "v");
        assert_eq!(key_pattern_label(""), "");
    }

    // The volatile exclusion must hold for a uuid anchor exactly as for a
    // vendor prefix: volatile/<anchor>/** is dropped, the persisted
    // <anchor>/** namespace is kept (ADR 1032 + ADR 1025).
    #[test]
    fn volatile_exclusion_matches_uuid_anchor_but_not_persisted() {
        let kes = parse_exclude_keys("volatile/**").unwrap();
        let vol =
            KeyExpr::new("volatile/61dc1e84ca2c7cfea827728a0f4bfa4d/stream/x").unwrap();
        assert!(
            kes.iter().any(|e| e.intersects(&vol)),
            "volatile/<uuid>/** must be excluded from audit"
        );
        let persisted =
            KeyExpr::new("61dc1e84ca2c7cfea827728a0f4bfa4d/data/track/v1").unwrap();
        assert!(
            !kes.iter().any(|e| e.intersects(&persisted)),
            "persisted <uuid>/** must NOT be excluded"
        );
    }

    #[test]
    fn identity_model_set_is_the_two_adr_1025_shapes() {
        assert!(VALID_IDENTITY_MODELS.contains(&"per-vendor"));
        assert!(VALID_IDENTITY_MODELS.contains(&"per-uuid-slot-pool"));
        assert!(!VALID_IDENTITY_MODELS.contains(&"per-org"));
    }

    // The audit record keeps `vendor` as a value-identical alias of `anchor`
    // so the portal tailer (which reads json:"vendor") is unaffected, while
    // new consumers can read `anchor` + `identity_model`.
    #[test]
    fn event_vendor_aliases_anchor_and_serializes_model() {
        let anchor = anchor_of_key("61dc1e84ca2c7cfea827728a0f4bfa4d/data/x/v1").to_string();
        let ev = AuditEvent {
            anchor: anchor.clone(),
            vendor: anchor.clone(),
            identity_model: "per-uuid-slot-pool".to_string(),
            key_expr: "61dc1e84ca2c7cfea827728a0f4bfa4d/data/x/v1".to_string(),
            ts: "2026-06-08T00:00:00.000Z".to_string(),
            src_zid: String::new(),
            payload_hash: "sha256:00".to_string(),
            payload_len: 0,
            payload_bytes_b64: None,
            audit_subscriber_id: "zagros".to_string(),
        };
        let j: serde_json::Value = serde_json::from_str(&serde_json::to_string(&ev).unwrap()).unwrap();
        assert_eq!(j["anchor"], j["vendor"]);
        assert_eq!(j["identity_model"], "per-uuid-slot-pool");
    }
}
