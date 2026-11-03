//! goat-crypto-posture — evidence-based crypto-posture publisher (ADR 1079).
//!
//! Every published field is observed evidence with a freshness stamp; nothing
//! is a hardcoded badge. See proto/crypto_posture_v1.proto for the contract.

use std::io::Write as _;
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use prost::Message;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore};
use tracing::{info, warn};

// ---- wire types (prost-derive on hand-written structs; canonical schema in
// proto/crypto_posture_v1.proto) ----
#[derive(Clone, PartialEq, Message)]
struct CryptoPostureV1 {
    #[prost(string, tag = "1")]
    site: String,
    #[prost(int64, tag = "2")]
    ts_unix_ms: i64,
    #[prost(bool, tag = "3")]
    kx_ok: bool,
    #[prost(string, tag = "4")]
    kx_group: String,
    #[prost(bool, tag = "5")]
    kx_group_fips: bool,
    #[prost(bool, tag = "6")]
    kx_pq: bool,
    #[prost(int64, tag = "7")]
    kx_probed_ms: i64,
    #[prost(string, tag = "8")]
    kx_error: String,
    #[prost(string, tag = "9")]
    router_version: String,
    #[prost(bool, tag = "10")]
    attest_present: bool,
    #[prost(bool, tag = "11")]
    attest_fips_marker: bool,
    #[prost(string, tag = "12")]
    attest_digest: String,
    #[prost(int64, tag = "13")]
    attest_at_ms: i64,
    #[prost(string, tag = "14")]
    attest_module: String,
    #[prost(string, tag = "15")]
    color: String,
    #[prost(string, tag = "16")]
    headline: String,
}

/// Host-side FIPS self-attestation file, written by the deploy (ansible role)
/// right after it asserts the router's boot marker. Absent on a non-FIPS
/// router — its absence is itself honest evidence ("no FIPS attestation").
#[derive(serde::Deserialize, Default)]
struct FipsAttest {
    #[serde(default)]
    fips_marker: bool,
    #[serde(default)]
    image_digest: String,
    #[serde(default)]
    attested_at_ms: i64,
    #[serde(default)]
    module: String,
}

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Evidence-based crypto-posture publisher for a Zenoh router (ADR 1079)."
)]
struct Args {
    /// Router host:port for the raw-TLS probe, e.g. `zagros-sandbox-zenoh-router:7447`.
    #[arg(long, env = "CRYPTO_POSTURE_PROBE_ADDR")]
    probe_addr: String,
    /// TLS SNI the router cert is issued for (defaults to the probe host).
    #[arg(long, env = "CRYPTO_POSTURE_SNI")]
    sni: Option<String>,
    /// Zenoh publish endpoint, e.g. `tls/zagros-sandbox-zenoh-router:7447`.
    #[arg(long, env = "CRYPTO_POSTURE_ENDPOINT")]
    endpoint: String,
    /// Site segment of `ops/fabric/crypto/<site>`.
    #[arg(long, env = "CRYPTO_POSTURE_SITE")]
    site: String,
    /// mTLS leaf cert (PEM) — used for BOTH the probe and the publish client.
    #[arg(long, env = "CRYPTO_POSTURE_CERT")]
    cert: String,
    /// mTLS private key (PEM).
    #[arg(long, env = "CRYPTO_POSTURE_KEY_FILE")]
    key: String,
    /// CA root (PEM) to verify the router.
    #[arg(long, env = "CRYPTO_POSTURE_ROOT_CA")]
    root_ca: String,
    /// REST admin base for the router version, e.g. `http://zagros-sandbox-zenoh-router:8000`.
    #[arg(long, env = "CRYPTO_POSTURE_REST_URL")]
    rest_url: Option<String>,
    /// Host-side FIPS attestation file (written by the deploy). Optional.
    #[arg(long, env = "CRYPTO_POSTURE_ATTEST_FILE")]
    attest_file: Option<String>,
    /// Publish interval (ms).
    #[arg(long, env = "CRYPTO_POSTURE_INTERVAL_MS", default_value_t = 30000)]
    interval_ms: u64,
    /// Max age (ms) before the host attestation is treated as stale (grey).
    #[arg(long, env = "CRYPTO_POSTURE_ATTEST_MAX_AGE_MS", default_value_t = 86_400_000)]
    attest_max_age_ms: i64,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn load_certs(pem: &str) -> Result<Vec<CertificateDer<'static>>> {
    let bytes = std::fs::read(pem).with_context(|| format!("read cert {pem}"))?;
    let certs = rustls_pemfile::certs(&mut bytes.as_slice())
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("parse cert chain")?;
    if certs.is_empty() {
        return Err(anyhow!("no certificates in {pem}"));
    }
    Ok(certs)
}

fn load_key(pem: &str) -> Result<PrivateKeyDer<'static>> {
    let bytes = std::fs::read(pem).with_context(|| format!("read key {pem}"))?;
    rustls_pemfile::private_key(&mut bytes.as_slice())
        .context("parse key")?
        .ok_or_else(|| anyhow!("no private key in {pem}"))
}

fn load_roots(pem: &str) -> Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    for c in load_certs(pem)? {
        roots.add(c).context("add root")?;
    }
    Ok(roots)
}

/// A single live raw-TLS handshake with a PQ-capable client. Returns the
/// negotiated group name, whether it is fips=true per the provider, and the
/// router's provider-independent identity. This is the load-bearing evidence.
struct KxResult {
    group: String,
    group_fips: bool,
    pq: bool,
}

fn probe_kx(args: &Args) -> Result<KxResult> {
    let roots = load_roots(&args.root_ca)?;
    let certs = load_certs(&args.cert)?;
    let key = load_key(&args.key)?;
    // Explicit aws-lc-rs provider (prefer-post-quantum) so a PQ router
    // negotiates X25519MLKEM768; the probe does not touch the process default.
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("tls versions")?
        .with_root_certificates(roots)
        .with_client_auth_cert(certs, key)
        .context("client auth")?;
    let sni = args
        .sni
        .clone()
        .unwrap_or_else(|| args.probe_addr.split(':').next().unwrap_or("").to_string());
    let server_name = ServerName::try_from(sni.clone())
        .map_err(|_| anyhow!("bad SNI {sni}"))?
        .to_owned();
    let tcp = TcpStream::connect(&args.probe_addr)
        .with_context(|| format!("connect {}", args.probe_addr))?;
    tcp.set_nodelay(true).ok();
    tcp.set_read_timeout(Some(Duration::from_secs(15))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(15))).ok();
    let mut conn = ClientConnection::new(Arc::new(config), server_name).context("client conn")?;
    let mut stream = tcp;
    while conn.is_handshaking() {
        conn.complete_io(&mut stream).context("handshake")?;
    }
    stream.flush().ok();
    let g = conn
        .negotiated_key_exchange_group()
        .ok_or_else(|| anyhow!("no negotiated group"))?;
    let name = format!("{:?}", g.name());
    Ok(KxResult {
        group_fips: g.fips(),
        pq: name.contains("MLKEM") || name.contains("Kyber"),
        group: name,
    })
}

/// Router self-reported version from the REST admin space — live evidence the
/// patched binary (`-modified`) is the running one. Best-effort; empty on miss.
fn probe_router_version(rest_url: &str) -> String {
    let url = format!("{}/@/local/router", rest_url.trim_end_matches('/'));
    let Ok(resp) = ureq::get(&url).timeout(Duration::from_secs(8)).call() else {
        return String::new();
    };
    let Ok(body) = resp.into_string() else {
        return String::new();
    };
    // The admin tree is a JSON array of router entries; pull the first version.
    match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(v) => v
            .as_array()
            .and_then(|a| a.first())
            .and_then(|e| e.pointer("/version").or_else(|| e.get("version")))
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        Err(_) => String::new(),
    }
}

fn read_attest(path: &Option<String>) -> Option<FipsAttest> {
    let p = path.as_ref()?;
    let bytes = std::fs::read(p).ok()?;
    serde_json::from_slice::<FipsAttest>(&bytes).ok()
}

/// Color the posture from the evidence — honest, never green on no data.
fn assess(
    kx: &Option<KxResult>,
    attest: &Option<FipsAttest>,
    attest_max_age_ms: i64,
    now: i64,
) -> (String, String) {
    let Some(kx) = kx else {
        return (
            "unknown".into(),
            "crypto posture unknown — TLS probe failed".into(),
        );
    };
    let attest_fresh = attest
        .as_ref()
        .map(|a| a.fips_marker && a.attested_at_ms > 0 && (now - a.attested_at_ms) <= attest_max_age_ms)
        .unwrap_or(false);
    if kx.pq && attest_fresh {
        let module = attest
            .as_ref()
            .map(|a| a.module.clone())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "validated FIPS module".into());
        (
            "fips-pq".into(),
            format!("post-quantum ({}) on {}", kx.group, module),
        )
    } else if kx.pq {
        (
            "pq".into(),
            format!("post-quantum ({}) — no FIPS attestation", kx.group),
        )
    } else {
        (
            "classical".into(),
            format!("classical key exchange ({})", kx.group),
        )
    }
}

async fn publish_round(session: &zenoh::Session, args: &Args) -> Result<()> {
    let now = now_ms();
    let kx = match probe_kx(args) {
        Ok(k) => {
            info!(group = %k.group, fips = k.group_fips, pq = k.pq, "kx probe ok");
            Some(k)
        }
        Err(e) => {
            warn!(error = %e, "kx probe failed");
            None
        }
    };
    let attest = read_attest(&args.attest_file);
    let router_version = args
        .rest_url
        .as_deref()
        .map(probe_router_version)
        .unwrap_or_default();
    let (color, headline) = assess(&kx, &attest, args.attest_max_age_ms, now);

    let msg = CryptoPostureV1 {
        site: args.site.clone(),
        ts_unix_ms: now,
        kx_ok: kx.is_some(),
        kx_group: kx.as_ref().map(|k| k.group.clone()).unwrap_or_default(),
        kx_group_fips: kx.as_ref().map(|k| k.group_fips).unwrap_or(false),
        kx_pq: kx.as_ref().map(|k| k.pq).unwrap_or(false),
        kx_probed_ms: now,
        kx_error: String::new(),
        router_version,
        attest_present: attest.is_some(),
        attest_fips_marker: attest.as_ref().map(|a| a.fips_marker).unwrap_or(false),
        attest_digest: attest.as_ref().map(|a| a.image_digest.clone()).unwrap_or_default(),
        attest_at_ms: attest.as_ref().map(|a| a.attested_at_ms).unwrap_or(0),
        attest_module: attest.as_ref().map(|a| a.module.clone()).unwrap_or_default(),
        color: color.clone(),
        headline: headline.clone(),
    };
    let key = format!("ops/fabric/crypto/{}", args.site);
    session
        .put(&key, msg.encode_to_vec())
        .await
        .map_err(|e| anyhow!("publish {key}: {e}"))?;
    info!(color = %color, headline = %headline, "published crypto posture");
    Ok(())
}

/// Build the zenoh publish session (mTLS client to the router). Mirrors the
/// working fabric-health/audit-subscriber client config: a single
/// `transport/link/tls` block with `enable_mtls: true` (so the client presents
/// its leaf — the router requires it) and `verify_name_on_connect: false`
/// (the router cert SAN is a container name, not a resolvable host).
async fn open_session(args: &Args) -> Result<zenoh::Session> {
    let mut cfg = zenoh::Config::default();
    cfg.insert_json5("mode", r#""client""#).map_err(|e| anyhow!("{e}"))?;
    cfg.insert_json5("connect/endpoints", &format!("[\"{}\"]", args.endpoint))
        .map_err(|e| anyhow!("{e}"))?;
    let tls_block = format!(
        r#"{{ "root_ca_certificate": "{}", "connect_certificate": "{}", "connect_private_key": "{}", "enable_mtls": true, "verify_name_on_connect": false }}"#,
        args.root_ca, args.cert, args.key
    );
    cfg.insert_json5("transport/link/tls", &tls_block)
        .map_err(|e| anyhow!("{e}"))?;
    cfg.insert_json5("scouting/multicast/enabled", "false")
        .map_err(|e| anyhow!("{e}"))?;
    zenoh::open(cfg).await.map_err(|e| anyhow!("zenoh open: {e}"))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let session = open_session(&args).await?;
    info!(
        site = %args.site,
        probe = %args.probe_addr,
        interval_ms = args.interval_ms,
        "crypto-posture publishing"
    );
    let mut ticker = tokio::time::interval(Duration::from_millis(args.interval_ms));
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if let Err(e) = publish_round(&session, &args).await {
                    warn!(error = %e, "publish round failed");
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("shutting down");
                break;
            }
        }
    }
    session.close().await.ok();
    Ok(())
}
