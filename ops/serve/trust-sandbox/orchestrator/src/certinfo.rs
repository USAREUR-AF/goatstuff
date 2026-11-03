//! Small x509 reader for display + validity — used for SVIDs and minted leaves.

use anyhow::{anyhow, Result};
use x509_parser::prelude::*;

#[derive(Debug, Clone, Default)]
pub struct CertInfo {
    pub cn: String,
    pub uri_sans: Vec<String>,
    pub not_after_unix: i64,
    pub serial: String,
}

pub fn parse_pem(pem_bytes: &[u8]) -> Result<CertInfo> {
    let (_, pem) = parse_x509_pem(pem_bytes).map_err(|e| anyhow!("pem: {e}"))?;
    let (_, cert) = X509Certificate::from_der(&pem.contents).map_err(|e| anyhow!("der: {e}"))?;

    let cn = cert
        .subject()
        .iter_common_name()
        .next()
        .and_then(|a| a.as_str().ok())
        .unwrap_or("")
        .to_string();

    let mut uri_sans = Vec::new();
    if let Ok(Some(san)) = cert.subject_alternative_name() {
        for gn in &san.value.general_names {
            if let GeneralName::URI(u) = gn {
                uri_sans.push(u.to_string());
            }
        }
    }

    Ok(CertInfo {
        cn,
        uri_sans,
        not_after_unix: cert.validity().not_after.timestamp(),
        serial: cert.raw_serial_as_string(),
    })
}
