//! The throwaway certificate authority.
//!
//! `init-ca` mode mints the CA root, zenohd's server cert, and the observer
//! leaf into the shared certs volume (so zenohd can boot before the long-lived
//! orchestrator). `serve` mode reloads the CA key and mints persona leaves on
//! demand — carrying the real URI-SAN grammar (`urn:goat:zagros:org=<anchor>,…`)
//! and CN=<anchor> (the axis the zenohd ACL keys on). This is the portal/CA
//! issuance path (gate ②); SPIRE is the other (gate ③), see spire.rs.

use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, Ia5String, IsCa, KeyPair, KeyUsagePurpose, SanType, SerialNumber,
};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const CA_CN: &str = "trust-sandbox throwaway CA";

/// Monotonic-ish serial so every (re)mint is visibly distinct (rotation shows a
/// fresh serial — ADR 1044 in miniature).
fn next_serial() -> u64 {
    static CTR: AtomicU64 = AtomicU64::new(0);
    let base = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(1);
    base.wrapping_add(CTR.fetch_add(1, Ordering::Relaxed))
}

fn ca_params() -> Result<CertificateParams> {
    let mut p = CertificateParams::new(Vec::<String>::new()).context("ca params")?;
    p.distinguished_name = DistinguishedName::new();
    p.distinguished_name.push(DnType::CommonName, CA_CN);
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    p.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    p.not_before = rcgen::date_time_ymd(2024, 1, 1);
    p.not_after = rcgen::date_time_ymd(2034, 1, 1);
    Ok(p)
}

fn leaf_params(
    cn: &str,
    dns: &[&str],
    uri_san: Option<&str>,
    expired: bool,
    server: bool,
) -> Result<CertificateParams> {
    let sans: Vec<String> = dns.iter().map(|s| s.to_string()).collect();
    let mut p = CertificateParams::new(sans).context("leaf params")?;
    p.distinguished_name = DistinguishedName::new();
    p.distinguished_name.push(DnType::CommonName, cn);
    if let Some(u) = uri_san {
        p.subject_alt_names
            .push(SanType::URI(Ia5String::try_from(u).context("uri-san")?));
    }
    p.is_ca = IsCa::NoCa;
    p.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    p.extended_key_usages = vec![if server {
        ExtendedKeyUsagePurpose::ServerAuth
    } else {
        ExtendedKeyUsagePurpose::ClientAuth
    }];
    if expired {
        // A real expired cert — the "stale credential" condition rejects at the
        // TLS layer for real, no simulation.
        p.not_before = rcgen::date_time_ymd(2020, 1, 1);
        p.not_after = rcgen::date_time_ymd(2020, 1, 2);
    } else {
        p.not_before = rcgen::date_time_ymd(2024, 1, 1);
        p.not_after = rcgen::date_time_ymd(2034, 1, 1);
    }
    p.serial_number = Some(SerialNumber::from(next_serial()));
    Ok(p)
}

/// One-shot: mint the CA + zenohd server cert + observer leaf into `out`.
///
/// IDEMPOTENT: if a CA already exists in `out`, do nothing. Recreating a
/// dependent service re-runs this one-shot; re-minting would hand zenohd (still
/// serving its boot-time cert) a fresh CA and break every mTLS handshake with a
/// signature mismatch. `docker compose down -v` is the way to roll a fresh CA.
pub fn init_ca(out: &Path) -> Result<()> {
    std::fs::create_dir_all(out).context("create certs dir")?;
    if out.join("ca-root.pem").exists() {
        tracing::info!("init-ca: CA already present in {} — leaving it (down -v to roll)", out.display());
        return Ok(());
    }

    let ca_key = KeyPair::generate().context("gen ca key")?;
    let ca_cert = ca_params()?.self_signed(&ca_key).context("self-sign ca")?;
    let ca_root_pem = ca_cert.pem();

    std::fs::write(out.join("ca-root.pem"), &ca_root_pem)?;
    std::fs::write(out.join("ca-key.pem"), ca_key.serialize_pem())?;
    // Initial client trust bundle = CA root only. spire-server's entrypoint
    // appends the SPIRE bundle before zenohd starts (sandbox-CA ⊕ SPIRE).
    std::fs::write(out.join("client-ca-bundle.pem"), &ca_root_pem)?;

    // zenohd server cert (SAN: zenohd, localhost — verify_name_on_connect is off
    // anyway, chain-to-root is the gate).
    let srv_key = KeyPair::generate()?;
    let srv = leaf_params("zenohd-server", &["zenohd", "localhost"], None, false, true)?
        .signed_by(&srv_key, &ca_cert, &ca_key)
        .context("sign server cert")?;
    std::fs::write(out.join("zenohd-cert.pem"), srv.pem())?;
    std::fs::write(out.join("zenohd-key.pem"), srv_key.serialize_pem())?;

    // Observer leaf — CN=observer; the orchestrator's long-lived read-all mTLS
    // session (matches the any-tls-peer read subject). Never writes.
    let obs_key = KeyPair::generate()?;
    let obs = leaf_params("observer", &[], None, false, false)?
        .signed_by(&obs_key, &ca_cert, &ca_key)
        .context("sign observer cert")?;
    std::fs::write(out.join("observer-cert.pem"), obs.pem())?;
    std::fs::write(out.join("observer-key.pem"), obs_key.serialize_pem())?;

    Ok(())
}

/// A self-signed leaf that does NOT chain to the sandbox CA — the `forged`
/// condition (an identity rooted in no trusted issuer). Returns (cert, key) PEM.
pub fn self_signed_leaf(cn: &str) -> Result<(String, String)> {
    let key = KeyPair::generate()?;
    let cert = leaf_params(cn, &[], None, false, false)?
        .self_signed(&key)
        .context("self-sign forged leaf")?;
    Ok((cert.pem(), key.serialize_pem()))
}

/// A freshly minted leaf, PEM-encoded, ready to drop into a Zenoh TLS config.
pub struct MintedLeaf {
    pub cert_pem: String,
    pub key_pem: String,
    pub cn: String,
    pub uri_san: String,
    pub serial: String,
    pub not_after: String,
}

/// Loaded CA used in `serve` mode to mint persona leaves on demand.
pub struct CaSigner {
    ca_cert: Certificate,
    ca_key: KeyPair,
}

impl CaSigner {
    pub fn load(dir: &Path) -> Result<Self> {
        let key_pem = std::fs::read_to_string(dir.join("ca-key.pem")).context("read ca-key")?;
        let ca_key = KeyPair::from_pem(&key_pem).context("parse ca-key")?;
        // Reconstruct the issuer deterministically (same DN + key → same chain).
        // signed_by only reads the issuer's DN, key-id method, key usages, and
        // key, so the on-disk ca-root.pem stays the trust anchor.
        let ca_cert = ca_params()?.self_signed(&ca_key).context("reconstruct ca")?;
        Ok(Self { ca_cert, ca_key })
    }

    /// Mint a client leaf with CN=<cn> and the given URI-SAN grammar string.
    pub fn mint(&self, cn: &str, uri_san: Option<&str>, expired: bool) -> Result<MintedLeaf> {
        let params = leaf_params(cn, &[], uri_san, expired, false)?;
        let serial = params
            .serial_number
            .as_ref()
            .map(|s| format!("{:x?}", s.to_bytes()))
            .unwrap_or_default();
        let not_after = params.not_after.to_string();
        let leaf_key = KeyPair::generate()?;
        let cert = params
            .signed_by(&leaf_key, &self.ca_cert, &self.ca_key)
            .context("sign leaf")?;
        Ok(MintedLeaf {
            cert_pem: cert.pem(),
            key_pem: leaf_key.serialize_pem(),
            cn: cn.to_string(),
            uri_san: uri_san.unwrap_or_default().to_string(),
            serial,
            not_after,
        })
    }
}
