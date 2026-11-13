// SPDX-License-Identifier: Apache-2.0
//
// goat-peer-health — the `peer` domain of the environment-health surface.
//
// goatcore ADR 1232, docs/design/peer-fault-handling.md, stage 1 (measure),
// first detector: a peer whose clock runs ahead. A reader beside a Zenoh router
// follows the router's log (standard input or a file; never the docker
// socket), turns each uhlc timestamp-rejection line into a per-peer offset and
// rate, grades the reading before blaming anyone, and publishes:
//
//   ops/peer/health/<site>   <- EnvHealthV1{domain:"peer"}, PeerFaultDetailV1 (arm 17)
//   ops/peer/verdict/<zid>   <- one JSON verdict per peer at fault (and once when cleared)
//
// It enforces nothing and persists nothing. If it stops, the fabric behaves
// exactly as before.

pub mod clock;
pub mod detector;
pub mod follow;
pub mod parse;
pub mod wire;

use prost::Message as _;
use std::collections::HashMap;
use std::future::Future;
use std::time::{Duration, UNIX_EPOCH};

/// RFC 3339 UTC with milliseconds, from Unix milliseconds.
pub fn rfc3339_ms(unix_ms: i64) -> String {
    let t = UNIX_EPOCH + Duration::from_millis(unix_ms.max(0) as u64);
    humantime::format_rfc3339_millis(t).to_string()
}

/// "17.7", "0.5", "1", "0.25": seconds without trailing zeros.
fn fmt_secs(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

// ---------------------------------------------------------------------------
// Attribution: --peers-file, a JSON map from Zenoh ID (or source address) to
// {name, owner}. No lookups over the network in this increment.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
pub struct Owner {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Attribution {
    /// Keyed by normalized Zenoh ID, or by the address as written.
    entries: HashMap<String, Owner>,
}

impl Attribution {
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        let raw: HashMap<String, Owner> = serde_json::from_str(text)?;
        let entries = raw
            .into_iter()
            .map(|(k, v)| {
                let k = k.trim().to_string();
                let key = if parse::is_zid(&k.to_ascii_lowercase()) {
                    parse::normalize_zid(&k)
                } else {
                    k
                };
                (key, v)
            })
            .collect();
        Ok(Self { entries })
    }

    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("read peers file {}: {e}", path.display()))?;
        Self::from_json(&text)
            .map_err(|e| anyhow::anyhow!("parse peers file {}: {e}", path.display()))
    }

    /// Look a peer up by Zenoh ID. Entries keyed by address match nothing until
    /// the reader can join a Zenoh ID to its source address (admin space; a
    /// later increment).
    pub fn lookup(&self, zid: &str) -> Option<&Owner> {
        self.entries.get(&parse::normalize_zid(zid))
    }

    /// The Zenoh IDs this file declares (part of the known-peer population).
    pub fn zids(&self) -> impl Iterator<Item = String> + '_ {
        self.entries.keys().filter(|k| parse::is_zid(k)).cloned()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Read the sessions file: a JSON list of the Zenoh IDs that currently hold a
/// session with our router. Re-read every tick; whoever writes it (an
/// admin-space poll, a later increment) should replace it atomically.
pub fn load_sessions(path: &std::path::Path) -> anyhow::Result<Vec<String>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("read sessions file {}: {e}", path.display()))?;
    let zids: Vec<String> = serde_json::from_str(&text)
        .map_err(|e| anyhow::anyhow!("parse sessions file {}: {e}", path.display()))?;
    Ok(zids)
}

// ---------------------------------------------------------------------------
// The per-peer verdict: a small JSON document on ops/peer/verdict/<zid>, the
// key a peer may be allowed to read about itself.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VerdictDoc {
    pub schema: String,
    pub fault: String,
    /// at_fault | cleared
    pub state: String,
    pub offset_seconds: f64,
    pub limit_seconds: f64,
    pub rejections_per_minute: f64,
    pub observed_at: String,
    /// Our router's Zenoh ID; "" when not known.
    pub observed_by: String,
    pub sentence: String,
    /// Set on a peering verdict (ops/peer/verdict/peering/<name>).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peering: Option<String>,
    /// On a peering verdict: the Zenoh IDs in the partner network whose
    /// timestamps were rejected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zids: Vec<String>,
}

impl VerdictDoc {
    pub fn at_fault(
        offset: f64,
        limit: f64,
        rate: f64,
        observed_at: &str,
        observed_by: &str,
    ) -> Self {
        let sentence = format!(
            "the router sees your clock {:.1} s ahead of its own; the limit is {} s",
            offset,
            fmt_secs(limit)
        );
        Self {
            schema: "goat.peer.verdict.v1".to_string(),
            fault: "clock_ahead".to_string(),
            state: "at_fault".to_string(),
            offset_seconds: offset,
            limit_seconds: limit,
            rejections_per_minute: rate,
            observed_at: observed_at.to_string(),
            observed_by: observed_by.to_string(),
            sentence,
            peering: None,
            zids: Vec::new(),
        }
    }

    pub fn peering_at_fault(
        peering: &str,
        zids: Vec<String>,
        max_offset: f64,
        limit: f64,
        rate: f64,
        observed_at: &str,
        observed_by: &str,
    ) -> Self {
        let mut d = Self::at_fault(max_offset, limit, rate, observed_at, observed_by);
        d.sentence = format!(
            "the router sees clocks in your network up to {:.1} s ahead of its own ({} stamping router(s)); the limit is {} s",
            max_offset,
            zids.len(),
            fmt_secs(limit)
        );
        d.peering = Some(peering.to_string());
        d.zids = zids;
        d
    }

    pub fn cleared(limit: f64, clean_after_ms: i64, observed_at: &str, observed_by: &str) -> Self {
        let sentence = format!(
            "the router has rejected no timestamp from you for {} min; the clock fault is cleared",
            fmt_secs(clean_after_ms as f64 / 60_000.0)
        );
        Self {
            schema: "goat.peer.verdict.v1".to_string(),
            fault: "clock_ahead".to_string(),
            state: "cleared".to_string(),
            offset_seconds: 0.0,
            limit_seconds: limit,
            rejections_per_minute: 0.0,
            observed_at: observed_at.to_string(),
            observed_by: observed_by.to_string(),
            sentence,
            peering: None,
            zids: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Publishing. The publisher is a trait so the integration test captures what
// would go on the fabric without a router.
// ---------------------------------------------------------------------------

pub const HEALTH_PREFIX: &str = "ops/peer/health/";
pub const VERDICT_PREFIX: &str = "ops/peer/verdict/";

pub trait Publisher {
    fn put(
        &mut self,
        key: &str,
        payload: Vec<u8>,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;
}

/// Publish one reading: the envelope (protobuf), then each verdict (JSON).
/// A failed verdict does not stop the others; the first error is returned.
pub async fn publish_reading<P: Publisher>(
    p: &mut P,
    site: &str,
    r: &detector::Reading,
) -> anyhow::Result<()> {
    let mut first_err = None;
    if let Err(e) = p
        .put(
            &format!("{HEALTH_PREFIX}{site}"),
            r.envelope.encode_to_vec(),
        )
        .await
    {
        first_err.get_or_insert(e);
    }
    for (zid, doc) in &r.verdicts {
        let body = serde_json::to_vec(doc)?;
        if let Err(e) = p.put(&format!("{VERDICT_PREFIX}{zid}"), body).await {
            first_err.get_or_insert(e);
        }
    }
    first_err.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_format() {
        assert_eq!(fmt_secs(0.5), "0.5");
        assert_eq!(fmt_secs(1.0), "1");
        assert_eq!(fmt_secs(0.25), "0.25");
        assert_eq!(fmt_secs(10.0), "10");
    }

    #[test]
    fn verdict_sentences() {
        let v = VerdictDoc::at_fault(17.702, 0.5, 120.0, "t", "z");
        assert_eq!(
            v.sentence,
            "the router sees your clock 17.7 s ahead of its own; the limit is 0.5 s"
        );
        let c = VerdictDoc::cleared(0.5, 600_000, "t", "z");
        assert_eq!(
            c.sentence,
            "the router has rejected no timestamp from you for 10 min; the clock fault is cleared"
        );
    }

    #[test]
    fn attribution_keys_normalize() {
        let a =
            Attribution::from_json(r#"{"00AB":{"name":"n","owner":"o"},"10.0.0.5":{"name":"m"}}"#)
                .unwrap();
        assert_eq!(a.lookup("ab").unwrap().owner.as_deref(), Some("o"));
        assert_eq!(a.lookup("00ab").unwrap().name.as_deref(), Some("n"));
        assert_eq!(a.zids().collect::<Vec<_>>(), vec!["ab".to_string()]);
        assert_eq!(a.len(), 2);
        assert!(Attribution::from_json("[1]").is_err());
    }
}
