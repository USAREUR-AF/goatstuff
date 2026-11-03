//! The Zenoh fabric interface — gate ⑤ made real.
//!
//! The orchestrator holds ONE long-lived mTLS "observer" session (the observer
//! leaf, read-all) that subscribes `**`; every sample it sees feeds both the
//! attempt-verification (did the write actually land?) and the live /ws viz, so
//! the visualization only ever shows what a real ACL-gated subscriber saw.
//!
//! Each attempt opens a FRESH client session holding a chosen identity's cert
//! and PUTs. If the handshake is refused (untrusted issuer / expired cert) the
//! cert gate failed; if the handshake succeeds but the sample never reaches the
//! observer, the ACL silently denied the ingress write — the F-177 lesson.

use crate::model::WsEvent;
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::broadcast;
use zenoh::Config;

type LastSeen = Arc<Mutex<HashMap<String, (String, Instant)>>>;

pub struct Observer {
    last_seen: LastSeen,
    // Keep the session + subscriber alive for the process lifetime.
    _session: zenoh::Session,
    _sub: zenoh::pubsub::Subscriber<()>,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn client_config(
    endpoint: &str,
    root_ca: &str,
    cert: &str,
    key: &str,
) -> Result<Config> {
    let mut c = Config::default();
    c.insert_json5("mode", "\"client\"").map_err(|e| anyhow!("{e}"))?;
    c.insert_json5("connect/endpoints", &format!("[\"{endpoint}\"]"))
        .map_err(|e| anyhow!("{e}"))?;
    c.insert_json5("scouting/multicast/enabled", "false")
        .map_err(|e| anyhow!("{e}"))?;
    for (k, v) in [
        ("transport/link/tls/root_ca_certificate", root_ca),
        ("transport/link/tls/connect_certificate", cert),
        ("transport/link/tls/connect_private_key", key),
    ] {
        c.insert_json5(k, &format!("\"{v}\"")).map_err(|e| anyhow!("{e}"))?;
    }
    c.insert_json5("transport/link/tls/verify_name_on_connect", "false")
        .map_err(|e| anyhow!("{e}"))?;
    c.insert_json5("transport/link/tls/enable_mtls", "true")
        .map_err(|e| anyhow!("{e}"))?;
    Ok(c)
}

/// Open the long-lived observer session + `**` subscriber.
pub async fn start_observer(
    endpoint: &str,
    certs_dir: &str,
    tx: broadcast::Sender<WsEvent>,
) -> Result<Observer> {
    let root_ca = format!("{certs_dir}/ca-root.pem");
    let cert = format!("{certs_dir}/observer-cert.pem");
    let key = format!("{certs_dir}/observer-key.pem");
    let config = client_config(endpoint, &root_ca, &cert, &key)?;
    let session = zenoh::open(config).await.map_err(|e| anyhow!("observer open: {e}"))?;

    let last_seen: LastSeen = Arc::new(Mutex::new(HashMap::new()));
    let ls = last_seen.clone();
    let sub = session
        .declare_subscriber("**")
        .callback(move |sample| {
            let key = sample.key_expr().as_str().to_string();
            let payload = sample
                .payload()
                .try_to_string()
                .map(|c| c.into_owned())
                .unwrap_or_default();
            if let Ok(mut m) = ls.lock() {
                m.insert(key.clone(), (payload.clone(), Instant::now()));
            }
            let _ = tx.send(WsEvent::Sample {
                key,
                payload,
                at: now_ms(),
            });
        })
        .await
        .map_err(|e| anyhow!("observer sub: {e}"))?;

    Ok(Observer {
        last_seen,
        _session: session,
        _sub: sub,
    })
}

impl Observer {
    /// Did the observer see `needle` on `key` since `t0`?
    pub fn seen_since(&self, key: &str, needle: &str, t0: Instant) -> bool {
        if let Ok(m) = self.last_seen.lock() {
            if let Some((payload, at)) = m.get(key) {
                return *at >= t0 && payload.contains(needle);
            }
        }
        false
    }
}

/// Open a fresh client session with the given cert FILES and PUT once.
/// Ok() = handshake succeeded + the put was dispatched (the ACL decision is
/// observed separately via the observer). Err = the handshake itself failed
/// (untrusted issuer / expired cert / no route).
pub async fn try_write(
    endpoint: &str,
    root_ca: &str,
    cert: &str,
    key: &str,
    key_expr: &str,
    payload: &str,
) -> Result<()> {
    let config = client_config(endpoint, root_ca, cert, key)?;
    let session = tokio::time::timeout(std::time::Duration::from_secs(6), zenoh::open(config))
        .await
        .map_err(|_| anyhow!("connect timed out"))?
        .map_err(|e| anyhow!("handshake refused: {e}"))?;
    session
        .put(key_expr, payload.to_string())
        .await
        .map_err(|e| anyhow!("put: {e}"))?;
    // Give the router a moment to route (or silently drop) before we close.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let _ = session.close().await;
    Ok(())
}

/// Fire a short burst (the "noisy producer" condition) into the identity's own
/// lane — the flood is real; its CONTAINMENT is the transport layer's role,
/// enforced by identity on the live fleet (represented here, see the UI).
pub async fn burst(
    endpoint: &str,
    root_ca: &str,
    cert: &str,
    key: &str,
    key_expr: &str,
    n: usize,
) -> Result<usize> {
    let config = client_config(endpoint, root_ca, cert, key)?;
    let session = tokio::time::timeout(std::time::Duration::from_secs(6), zenoh::open(config))
        .await
        .map_err(|_| anyhow!("connect timed out"))?
        .map_err(|e| anyhow!("handshake refused: {e}"))?;
    for i in 0..n {
        let _ = session.put(key_expr, format!("{{\"burst\":{i}}}")).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    let _ = session.close().await;
    Ok(n)
}
