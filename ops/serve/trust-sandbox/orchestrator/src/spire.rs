//! The SPIRE / SPIFFE workload-identity path (gate ③).
//!
//! The orchestrator IS a SPIRE workload (registered with CN=edge-processor via
//! `-dns`). It shells out to the bundled `spire-agent api fetch x509 -write`
//! to obtain a REAL, attested, short-TTL SVID — far more robust than the
//! workload-API client crate. The SVID's expiry drives the rotation countdown;
//! presenting it to zenohd is a real mTLS handshake gated by SPIRE-issued
//! identity, and its CN (edge-processor) is what the zenohd ACL authorizes.

use crate::certinfo::{self, CertInfo};
use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;
use std::process::Command;

pub struct Svid {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub info: CertInfo,
}

/// Fetch the current SVID into `out_dir`. Returns the on-disk paths + parsed
/// validity. Errors if the agent isn't ready (the SPIRE gate then reads "down").
pub fn fetch_svid(socket: &str, out_dir: &str) -> Result<Svid> {
    std::fs::create_dir_all(out_dir).context("svid dir")?;
    let sock = socket
        .strip_prefix("unix://")
        .unwrap_or(socket);

    let out = Command::new("spire-agent")
        .args([
            "api", "fetch", "x509",
            "-socketPath", sock,
            "-write", out_dir,
        ])
        .output()
        .context("spawn spire-agent")?;
    if !out.status.success() {
        return Err(anyhow!(
            "spire-agent fetch failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    // `-write` emits svid.0.pem (leaf+intermediates), svid.0.key, bundle.0.pem.
    let cert_path = PathBuf::from(out_dir).join("svid.0.pem");
    let key_path = PathBuf::from(out_dir).join("svid.0.key");
    let pem = std::fs::read(&cert_path).context("read svid.0.pem")?;
    let info = certinfo::parse_pem(&pem).context("parse svid")?;

    Ok(Svid {
        cert_path,
        key_path,
        info,
    })
}

/// Is the SPIRE agent reachable + issuing? (drives the health surface)
pub fn available(socket: &str, out_dir: &str) -> bool {
    fetch_svid(socket, out_dir).is_ok()
}
