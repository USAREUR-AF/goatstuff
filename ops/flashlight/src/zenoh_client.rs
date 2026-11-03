// SPDX-License-Identifier: Apache-2.0
//
// Zenoh client — opens a session against the configured router and
// subscribes on the narrowed key-expression set from a
// `SubscriptionPlan`. Each incoming sample is broadcast on a
// `tokio::sync::broadcast::Sender<SampleEvent>` for downstream
// consumers (the SSE handler in PR #3 of the Phase C series).
//
// mTLS configuration is delegated to
// `panoscope_core::apply_tls_to_zenoh_config` — the same helper
// panoscope and stethoscope use, so all three inspection siblings
// resolve mTLS identically.
//
// Mode `client` (not `peer`) — flashlight is a passive observer that
// only subscribes; client mode forces a real connection to the
// configured endpoint and makes the router aware of our subscriber,
// matching panoscope's verified-live behavior.

use std::sync::{Arc, RwLock};

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use tokio::sync::broadcast;
use tracing::{info, warn};

use std::time::Duration;

use panoscope_core::topology::Topology;
use panoscope_core::{
    apply_tls_to_zenoh_config, declare_catchup_subscriber, now_ms, CatchupConfig, TlsConfig,
};

use crate::store::{SampleFormat, SampleStore};
use crate::subscription::SubscriptionPlan;

/// Lossy broadcast capacity. Slow SSE consumers miss events when the
/// ring wraps; that's intentional — Tier 1 is a live tail, not a
/// guaranteed log. Replay (`?since=`) comes off the MinIO audit-tail
/// in a later PR.
pub const BROADCAST_CAPACITY: usize = 1024;

/// One sample observed on the subscribed key-expression set. Fields
/// are deliberately minimal — the live SSE wire carries key, timestamp,
/// payload length, source ZID, a monotonic `seq`, and a coarse `format`
/// hint, but NOT the payload bytes. Bytes stay server-side in the
/// `SampleStore` and flow only when the operator clicks a row (which
/// fetches `GET /api/sample/{seq}`). This honors
/// docs/design/minimalism-under-network-constraints.md — bytes don't
/// hit the wire until asked for.
#[derive(Debug, Clone, Serialize)]
pub struct SampleEvent {
    /// Monotonic ingest sequence number — the key the browser uses to
    /// fetch this sample's retained payload from `/api/sample/{seq}`.
    pub seq: u64,
    /// Full Zenoh key expression the sample arrived on.
    pub key: String,
    /// Timestamp the flashlight backend observed the sample (ms since epoch).
    pub ts_ms: u64,
    /// Publisher ZID (hex string) when Zenoh provided one on the wire,
    /// `None` otherwise. In zenoh 1.x most publishers omit this; the
    /// SSE handler will lean on the topic-prefix attribution path the
    /// panoscope-core peer registry already uses.
    ///
    /// **ASSERTED BY THE PUBLISHER. VERIFIED BY NOTHING.** This value is
    /// `ext_sinfo` off the wire. A forwarding hop rewrites only `wire_expr`
    /// and `ext_nodeid`, so it crosses a federation boundary verbatim — and
    /// nothing anywhere in zenoh compares it to the transport peer that
    /// actually delivered the sample. A peered party can therefore claim any
    /// ZID here, including one belonging to our own routers.
    ///
    /// Do not render this as a verified origin, and do not use it to
    /// attribute data that arrived across a trust boundary. The two facts
    /// that *are* authenticated are the ingress key prefix and the peering
    /// certificate's Common Name.
    /// See `docs/design/cross-sovereign-federation.md` §6 and ADR 1152.
    pub src_zid: Option<String>,
    /// Length of the payload in bytes — useful for the inline-rate
    /// sparkline + payload-size column in the Tier 1 event table.
    pub payload_len: usize,
    /// Coarse format hint — drives the format-icon column in the Tier 1
    /// event table. Now encoding-authoritative when the publisher set a
    /// wire `Encoding` (protobuf/cbor are knowable only this way), else
    /// content-classified.
    pub format: SampleFormat,
    /// The stringified wire `Encoding` when the publisher set a meaningful
    /// one (e.g. `application/protobuf;TrackUpdate`), `None` for the
    /// default `zenoh/bytes`. Lets the client show the declared media type
    /// + protobuf schema name without a portal round-trip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
}

/// Runtime config — env-var driven, defaults sized for nova-a trial.
#[derive(Debug, Clone)]
pub struct ZenohConfig {
    /// Endpoint locator (e.g. `tls/zenoh.nova.netbird.nova.example:7447`).
    pub endpoint: String,
    pub tls: TlsConfig,
}

impl ZenohConfig {
    /// Read the standard `FLASHLIGHT_ZENOH_*` env vars. When
    /// `FLASHLIGHT_ZENOH_ENDPOINT` is unset, returns `None` so the
    /// binary stays usable in `cargo run` smoke without a router on
    /// the loopback.
    pub fn from_env() -> Result<Option<Self>> {
        let Ok(endpoint) = std::env::var("FLASHLIGHT_ZENOH_ENDPOINT") else {
            return Ok(None);
        };
        let tls = TlsConfig {
            enabled: env_flag("FLASHLIGHT_ZENOH_MTLS_ENABLED"),
            root_ca_certificate: std::env::var("FLASHLIGHT_ZENOH_TLS_ROOT_CA").unwrap_or_default(),
            connect_certificate: std::env::var("FLASHLIGHT_ZENOH_TLS_CLIENT_CERT")
                .unwrap_or_default(),
            connect_private_key: std::env::var("FLASHLIGHT_ZENOH_TLS_CLIENT_KEY")
                .unwrap_or_default(),
        };
        tls.validate().context("validate FLASHLIGHT_ZENOH_TLS_* envs")?;
        Ok(Some(Self { endpoint, tls }))
    }
}

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref().map(|s| s.to_ascii_lowercase()),
        Ok(v) if matches!(v.as_str(), "true" | "1" | "yes")
    )
}

/// A running live subscription: the session, the per-key-expression
/// ingest tasks, and the broadcast sender they feed.
///
/// Returned rather than detached so a supervisor can observe the
/// subscription dying and rebuild it. The previous shape spawned the
/// ingest tasks and returned only the sender, which left no way to
/// notice that ingest had stopped — the session could go silent while
/// the sender (and therefore `/healthz`) stayed as healthy as ever.
pub struct LiveSubscription {
    tx: broadcast::Sender<SampleEvent>,
    session: Arc<zenoh::Session>,
    tasks: tokio::task::JoinSet<()>,
}

// Hand-written rather than derived: neither the zenoh session nor the
// JoinSet is Debug, and the useful thing to print is the shape anyway.
impl std::fmt::Debug for LiveSubscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveSubscription")
            .field("ingest_tasks", &self.tasks.len())
            .field("receivers", &self.tx.receiver_count())
            .finish()
    }
}

impl LiveSubscription {
    /// The broadcast sender SSE handlers subscribe to.
    pub fn sender(&self) -> broadcast::Sender<SampleEvent> {
        self.tx.clone()
    }

    /// The zenoh session, for liveness interrogation by the supervisor.
    pub fn session(&self) -> Arc<zenoh::Session> {
        self.session.clone()
    }

    /// Resolves when any ingest task finishes. An ingest task only ends
    /// when its subscriber's channel closes, so this resolving at all
    /// means the subscription is no longer delivering.
    pub async fn ingest_ended(&mut self) {
        if self.tasks.is_empty() {
            // No tasks to outlive; treat as ended immediately rather
            // than parking forever on an empty JoinSet.
            return;
        }
        let _ = self.tasks.join_next().await;
    }

    /// Abort every ingest task and close the session, so the next
    /// connect starts from a clean transport rather than layering a
    /// second session onto a half-dead one.
    pub async fn shutdown(mut self) {
        self.tasks.shutdown().await;
        if let Err(e) = self.session.close().await {
            warn!(error = %e, "closing zenoh session after subscription loss");
        }
    }
}

/// Open a Zenoh session, declare a subscriber for each key-expression
/// in the plan, and forward arriving samples onto the broadcast
/// channel. Returns a [`LiveSubscription`] so the caller can both hand
/// the sender to request handlers and supervise the ingest tasks.
///
/// Empty `plan.key_exprs` is treated as "no subscription"; the caller
/// should render the help page instead of calling this.
pub async fn run(
    cfg: &ZenohConfig,
    plan: &SubscriptionPlan,
    store: Arc<SampleStore>,
    topology: Arc<RwLock<Topology>>,
) -> Result<LiveSubscription> {
    if plan.key_exprs.is_empty() {
        return Err(anyhow!(
            "refusing to open a Zenoh session for an empty subscription plan — \
             the apply-side should be rendering the help page instead"
        ));
    }

    let (tx, _rx) = broadcast::channel::<SampleEvent>(BROADCAST_CAPACITY);

    let mut config = zenoh::Config::default();
    config
        .insert_json5("mode", "\"client\"")
        .map_err(|e| anyhow!("invalid zenoh mode config: {e}"))?;
    config
        .insert_json5("connect/endpoints", &format!("[\"{}\"]", cfg.endpoint))
        .map_err(|e| anyhow!("invalid zenoh endpoint config: {e}"))?;

    apply_tls_to_zenoh_config(&mut config, &cfg.tls)
        .context("applying Zenoh mTLS material from FLASHLIGHT_ZENOH_TLS_* envs")?;

    let session = zenoh::open(config)
        .await
        .map_err(|e| anyhow!("zenoh::open failed: {e}"))?;
    info!(zid = %session.zid(), endpoint = %cfg.endpoint, "zenoh session open");

    let session = Arc::new(session);

    let mut tasks = tokio::task::JoinSet::new();
    for ke in &plan.key_exprs {
        let session = session.clone();
        let tx = tx.clone();
        let store = store.clone();
        let topology = topology.clone();
        let key_expr = ke.clone();
        tasks.spawn(async move {
            match subscribe_loop(session, &key_expr, tx, store, topology).await {
                // Both arms are subscription loss. The error arm was the
                // only one that logged before, so the far more common
                // quiet ending — the subscriber's channel closing under a
                // dead session — left no trace at all.
                Ok(()) => warn!(key_expr, "subscribe loop ended: subscriber channel closed"),
                Err(e) => warn!(error = %e, key_expr, "subscribe loop exited with an error"),
            }
        });
    }

    Ok(LiveSubscription { tx, session, tasks })
}

async fn subscribe_loop(
    session: Arc<zenoh::Session>,
    key_expr: &str,
    tx: broadcast::Sender<SampleEvent>,
    store: Arc<SampleStore>,
    topology: Arc<RwLock<Topology>>,
) -> Result<()> {
    // Catch-up subscriber (panoscope-core advanced_sub; gap 2+4 of
    // docs/design/zenoh-capability-adoption.md). Recovery-only config:
    // flashlight is a live tail (replay comes off the audit-tail, not
    // here), so we heal samples dropped DURING a session via background
    // re-query but pull no on-connect history backlog. Against today's
    // plain publishers this re-pulls the latest retained value on the
    // recovery timer; true sequence-numbered gap-filling activates once
    // publishers adopt AdvancedPublisher (gap 5). The returned
    // AdvancedSubscriber derefs to its receiver, so the recv loop below is
    // unchanged.
    let catchup = CatchupConfig::recovery_only(Duration::from_secs(5));
    let subscriber = declare_catchup_subscriber(&session, key_expr, &catchup)
        .await
        .map_err(|e| anyhow!("declare_catchup_subscriber on `{key_expr}` failed: {e}"))?;
    info!(key_expr, "catch-up subscriber declared (recovery-only)");

    while let Ok(sample) = subscriber.recv_async().await {
        let key = sample.key_expr().as_str().to_string();
        let ts_ms = now_ms();
        // ASSERTED origin, never verified — see the `src_zid` field docs.
        // `source_info` is publisher-supplied and survives a federation hop
        // unchanged; no code compares it to the transport peer.
        let src_zid = sample
            .source_info()
            .map(|si| si.source_id().zid().to_string());
        // Materialize the payload bytes once: retain a bounded preview
        // in the store (keyed by the assigned seq) and forward only the
        // length + format hint on the live wire.
        let bytes = sample.payload().to_bytes();
        let payload_len = bytes.len();
        // Self-describing wire encoding (gap 1 consumer half,
        // docs/design/zenoh-capability-adoption.md). The publishers we own
        // set this from their one declared format; read it off the wire so
        // the format hint + decode path don't have to guess. Normalize the
        // default/unset `zenoh/bytes` to None so content classification still
        // kicks in for publishers that set nothing.
        let encoding = {
            let enc = sample.encoding().to_string();
            if enc.is_empty() || enc == "zenoh/bytes" {
                None
            } else {
                Some(enc)
            }
        };
        // Hydrate the peer topology from schema-declarations (published
        // on `attest/schema/**`, which the broad subscription sees) so
        // the SSE handler can resolve role/site/format post-filters.
        // Non-decl keys are a cheap prefix-check no-op.
        if crate::schema_decl::is_schema_decl_key(&key) {
            if let Ok(mut topo) = topology.write() {
                crate::schema_decl::ingest_if_schema_decl(&mut topo, &key, &bytes, ts_ms);
            }
        }
        let (seq, format) = store.ingest(key.clone(), ts_ms, &bytes, encoding.as_deref());
        let event = SampleEvent {
            seq,
            key,
            ts_ms,
            src_zid,
            payload_len,
            format,
            encoding,
        };
        // Lossy send: ignore the "no receivers" + "lagged" errors.
        // Receivers handle catch-up via `RecvError::Lagged` in their
        // own consumer loops.
        let _ = tx.send(event);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_flag_true_variants() {
        std::env::set_var("FLASHLIGHT_TEST_FLAG_T1", "true");
        std::env::set_var("FLASHLIGHT_TEST_FLAG_T2", "TRUE");
        std::env::set_var("FLASHLIGHT_TEST_FLAG_T3", "yes");
        std::env::set_var("FLASHLIGHT_TEST_FLAG_T4", "1");
        assert!(env_flag("FLASHLIGHT_TEST_FLAG_T1"));
        assert!(env_flag("FLASHLIGHT_TEST_FLAG_T2"));
        assert!(env_flag("FLASHLIGHT_TEST_FLAG_T3"));
        assert!(env_flag("FLASHLIGHT_TEST_FLAG_T4"));
    }

    #[test]
    fn env_flag_false_variants() {
        std::env::set_var("FLASHLIGHT_TEST_FLAG_F1", "false");
        std::env::set_var("FLASHLIGHT_TEST_FLAG_F2", "0");
        std::env::set_var("FLASHLIGHT_TEST_FLAG_F3", "no");
        std::env::set_var("FLASHLIGHT_TEST_FLAG_F4", "");
        assert!(!env_flag("FLASHLIGHT_TEST_FLAG_F1"));
        assert!(!env_flag("FLASHLIGHT_TEST_FLAG_F2"));
        assert!(!env_flag("FLASHLIGHT_TEST_FLAG_F3"));
        assert!(!env_flag("FLASHLIGHT_TEST_FLAG_F4"));
        assert!(!env_flag("FLASHLIGHT_TEST_FLAG_DOES_NOT_EXIST"));
    }

    #[test]
    fn zenoh_config_from_env_returns_none_when_endpoint_unset() {
        std::env::remove_var("FLASHLIGHT_ZENOH_ENDPOINT");
        let cfg = ZenohConfig::from_env().unwrap();
        assert!(cfg.is_none());
    }

    #[tokio::test]
    async fn run_refuses_empty_plan() {
        let cfg = ZenohConfig {
            endpoint: "tcp/127.0.0.1:7447".into(),
            tls: TlsConfig::default(),
        };
        let empty_plan = SubscriptionPlan::default();
        let store = Arc::new(SampleStore::new(8, 4096));
        let topology = Arc::new(RwLock::new(Topology::default()));
        let err = run(&cfg, &empty_plan, store, topology).await.unwrap_err();
        assert!(err.to_string().contains("empty subscription plan"));
    }

    #[test]
    fn sample_event_serializes_to_json() {
        let ev = SampleEvent {
            seq: 7,
            key: "vendor-a/observations/v1".into(),
            ts_ms: 1_700_000_000_000,
            src_zid: Some("abc123".into()),
            payload_len: 412,
            format: SampleFormat::Protobuf,
            encoding: Some("application/protobuf;TrackUpdate".into()),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"seq\":7"));
        assert!(json.contains("\"key\":\"vendor-a/observations/v1\""));
        assert!(json.contains("\"ts_ms\":1700000000000"));
        assert!(json.contains("\"src_zid\":\"abc123\""));
        assert!(json.contains("\"payload_len\":412"));
        assert!(json.contains("\"format\":\"protobuf\""));
        assert!(json.contains("\"encoding\":\"application/protobuf;TrackUpdate\""));
    }

    #[test]
    fn sample_event_omits_or_nulls_src_zid_when_absent() {
        let ev = SampleEvent {
            seq: 0,
            key: "foo".into(),
            ts_ms: 0,
            src_zid: None,
            payload_len: 0,
            format: SampleFormat::Empty,
            encoding: None,
        };
        let json = serde_json::to_string(&ev).unwrap();
        // Serde's default Option<T> serializes None as `null`.
        assert!(json.contains("\"src_zid\":null"));
        // encoding uses skip_serializing_if so an absent encoding drops out
        // of the wire entirely (no `"encoding":null` noise on every sample).
        assert!(!json.contains("\"encoding\""));
    }
}
