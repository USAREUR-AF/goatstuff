// SPDX-License-Identifier: Apache-2.0
//
// Per-topic ring buffer of recent zenoh sample payloads, with
// best-effort format detection (json / cbor / text / hex). Backs the
// /api/samples endpoint and the click-to-stream UI on each node.
//
// Memory discipline: per-topic cap (default 50 entries) + per-entry
// byte cap (default 4 KiB). v1 simple — no whole-buffer max, no LRU
// eviction across topics. Sufficient for the trial fabric where the
// active topic count is single-digits.

use serde::Serialize;
use std::collections::{HashMap, VecDeque};

/// Maximum sample entries retained per topic. Older entries drop off
/// the back as newer ones arrive.
pub const MAX_ENTRIES_PER_TOPIC: usize = 50;

/// Maximum payload bytes retained per entry. Larger payloads truncate
/// with `truncated: true` flagged on the entry.
pub const MAX_PAYLOAD_BYTES: usize = 4096;

/// Best-effort sample-payload format classification.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SampleFormat {
    Json,
    Cbor,
    Protobuf,
    Text,
    Hex,
    Empty,
}

/// A single captured sample's metadata + decoded payload preview.
#[derive(Debug, Clone, Serialize)]
pub struct SampleEntry {
    pub ts_ms: u64,
    pub topic: String,
    pub source_peer: Option<String>,
    pub format: SampleFormat,
    /// The stringified self-describing wire `Encoding` when the publisher
    /// set a meaningful one (`application/protobuf;TrackUpdate`,
    /// `application/json`, …); `None` for the default `zenoh/bytes`. Lets
    /// the decoder pick the protobuf message name off the wire when the
    /// portal topic row doesn't declare a `format`
    /// (docs/design/zenoh-capability-adoption.md gap 1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
    pub size_bytes: usize,
    pub truncated: bool,
    /// Best-effort decoded preview. For `Json`, the pretty-printed JSON
    /// string. For `Text`, the utf-8 string (truncated to MAX_PAYLOAD_BYTES).
    /// For `Hex`/`Cbor`, hex dump of the first MAX_PAYLOAD_BYTES bytes.
    /// For `Empty`, an empty string.
    pub preview: String,
    /// Raw payload bytes retained for in-process protobuf decode (the
    /// `Decoded` stream view in the side panel; see decoder.rs). Skipped
    /// from JSON serialization — only the in-process /api/samples/decoded
    /// handler reads it. None when the payload exceeded MAX_PAYLOAD_BYTES,
    /// to keep the per-topic memory cap bounded.
    #[serde(skip)]
    pub raw_bytes: Option<Vec<u8>>,
    /// True when this entry was NOT observed live on the wire but recalled
    /// on-demand from the router's Zenoh storage because the in-RAM ring
    /// was empty (panoscope-stale-recall B1). `ts_ms` then carries the
    /// sample's *original publish* time, so the frontend can render
    /// "last seen Ns ago" and desaturate the pane (never present as live).
    /// Defaults false for every live-ring entry.
    #[serde(default)]
    pub stale: bool,
}

/// Per-topic ring buffer.
#[derive(Default)]
pub struct SampleBuffer {
    by_topic: HashMap<String, VecDeque<SampleEntry>>,
}

impl SampleBuffer {
    pub fn push(
        &mut self,
        topic: String,
        ts_ms: u64,
        source_peer: Option<String>,
        payload: &[u8],
        encoding: Option<String>,
    ) {
        let entry = build_entry(topic.clone(), ts_ms, source_peer, payload, encoding);
        let q = self.by_topic.entry(topic).or_default();
        if q.len() == MAX_ENTRIES_PER_TOPIC {
            q.pop_front();
        }
        q.push_back(entry);
    }

    /// Return the most-recent `limit` samples for the given topic,
    /// newest first. Empty Vec if the topic has no buffered samples.
    pub fn recent(&self, topic: &str, limit: usize) -> Vec<SampleEntry> {
        self.by_topic
            .get(topic)
            .map(|q| q.iter().rev().take(limit).cloned().collect())
            .unwrap_or_default()
    }

    /// Latest classified wire format for `topic`, derived from the most
    /// recent buffered sample. `None` when no samples are buffered yet.
    pub fn latest_format(&self, topic: &str) -> Option<SampleFormat> {
        self.by_topic.get(topic).and_then(|q| q.back()).map(|e| e.format)
    }

    /// Latest buffered entry per topic under `prefix` (the /api/top
    /// assembly path — the `ops/**/health` + footprint beats already flow
    /// through the firehose, so "current state" is the newest ring entry
    /// per key). Excludes stale-recall entries: a recalled sample was not
    /// observed live and must never seed a live dashboard row.
    pub fn latest_by_prefix(&self, prefix: &str) -> Vec<&SampleEntry> {
        self.by_topic
            .iter()
            .filter(|(topic, _)| topic.starts_with(prefix))
            .filter_map(|(_, q)| q.iter().rev().find(|e| !e.stale))
            .collect()
    }
}

fn build_entry(
    topic: String,
    ts_ms: u64,
    source_peer: Option<String>,
    payload: &[u8],
    encoding: Option<String>,
) -> SampleEntry {
    let size_bytes = payload.len();
    if size_bytes == 0 {
        return SampleEntry {
            ts_ms,
            topic,
            source_peer,
            format: SampleFormat::Empty,
            encoding,
            size_bytes: 0,
            truncated: false,
            preview: String::new(),
            raw_bytes: Some(Vec::new()),
            stale: false,
        };
    }
    let truncated = size_bytes > MAX_PAYLOAD_BYTES;
    let view = if truncated {
        &payload[..MAX_PAYLOAD_BYTES]
    } else {
        payload
    };
    let (format, preview) = classify(view);
    // Retain raw bytes up to the per-entry cap so the decoder can replay
    // them through prost-reflect. Truncated payloads skip retention —
    // partial protobuf bytes don't decode cleanly and the operator already
    // sees `truncated: true` in the raw view.
    let raw_bytes = if truncated { None } else { Some(payload.to_vec()) };
    SampleEntry {
        ts_ms,
        topic,
        source_peer,
        format,
        encoding,
        size_bytes,
        truncated,
        preview,
        raw_bytes,
        stale: false,
    }
}

/// Build a one-off `SampleEntry` for a sample recalled from Zenoh storage
/// (panoscope-stale-recall B1) rather than observed live. Identical
/// classification + raw-byte retention to a live entry, but `stale` is
/// flagged true so the frontend never presents it as a live sample.
/// `ts_ms` is the sample's original publish timestamp (from the stored
/// Zenoh `Timestamp`), so "last seen Ns ago" reflects when the data was
/// actually produced, not when panoscope queried it.
pub fn stale_entry(
    topic: String,
    ts_ms: u64,
    source_peer: Option<String>,
    payload: &[u8],
    encoding: Option<String>,
) -> SampleEntry {
    SampleEntry {
        stale: true,
        ..build_entry(topic, ts_ms, source_peer, payload, encoding)
    }
}

fn classify(bytes: &[u8]) -> (SampleFormat, String) {
    // 1. JSON — try parse, return pretty-printed.
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) {
        let pretty = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
        return (SampleFormat::Json, pretty);
    }
    // 2. CBOR — heuristic: serde_cbor isn't a dep here; instead detect
    //    canonical CBOR initial bytes (0xa0-0xbf for map, 0x80-0x9f for
    //    array). Mark as cbor and hex-dump.
    if let Some(&first) = bytes.first() {
        let major = first >> 5;
        if (major == 4 || major == 5) && bytes.len() > 1 {
            return (SampleFormat::Cbor, hex_dump(bytes));
        }
    }
    // 3. Text — all utf-8 + all printable (or whitespace).
    if let Ok(s) = std::str::from_utf8(bytes) {
        let printable = s
            .chars()
            .all(|c| !c.is_control() || c == '\n' || c == '\r' || c == '\t');
        if printable {
            return (SampleFormat::Text, s.to_string());
        }
    }
    // 4. Protobuf wire-shape heuristic. A proto message begins with a
    //    field-tag byte: (field_number << 3) | wire_type. For field
    //    numbers 1..15 and wire types {0 varint, 2 length-delimited,
    //    5 32-bit} the first byte is in 0x08-0x7F with wire-type bits
    //    in {0,2,5}. Requires >=2 bytes so a single 0x0A doesn't false-
    //    positive. Real proto payloads from synth-vendor (RadarContacts,
    //    EffectorStatus, FusedTracks) all start with 0x0A or 0x12
    //    (field 1 or 2, wire type 2).
    if bytes.len() >= 2 {
        let first = bytes[0];
        if (0x08..=0x7F).contains(&first) {
            let wire_type = first & 0x07;
            if matches!(wire_type, 0 | 1 | 2 | 5) {
                return (SampleFormat::Protobuf, hex_dump(bytes));
            }
        }
    }
    // 5. Else hex dump.
    (SampleFormat::Hex, hex_dump(bytes))
}

fn hex_dump(bytes: &[u8]) -> String {
    // 16 bytes per row, " | " separator, ascii sidebar.
    let mut out = String::with_capacity(bytes.len() * 4);
    for (i, chunk) in bytes.chunks(16).enumerate() {
        use std::fmt::Write;
        let _ = write!(out, "{:08x}  ", i * 16);
        for (j, b) in chunk.iter().enumerate() {
            let _ = write!(out, "{:02x}", b);
            out.push(if j == 7 { '|' } else { ' ' });
        }
        for _ in chunk.len()..16 {
            out.push_str("   ");
        }
        out.push_str(" | ");
        for b in chunk {
            out.push(if (32..127).contains(b) { *b as char } else { '.' });
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_payload_classifies_as_json_with_pretty_preview() {
        let entry = build_entry(
            "t".into(),
            1,
            None,
            br#"{"track_id":42,"lat":35.5}"#,
            None,
        );
        assert_eq!(entry.format, SampleFormat::Json);
        assert!(entry.preview.contains("\"track_id\": 42"));
    }

    #[test]
    fn text_payload_classifies_as_text() {
        let entry = build_entry("t".into(), 1, None, b"hello world", None);
        assert_eq!(entry.format, SampleFormat::Text);
        assert_eq!(entry.preview, "hello world");
    }

    #[test]
    fn binary_payload_classifies_as_hex() {
        let entry = build_entry("t".into(), 1, None, &[0xff, 0x00, 0x7f], None);
        assert_eq!(entry.format, SampleFormat::Hex);
        assert!(entry.preview.starts_with("00000000"));
    }

    #[test]
    fn stale_entry_flags_stale_and_classifies_like_live() {
        let payload = br#"{"track_id":7}"#;
        let live = build_entry("drs/r/RADAR/tracks".into(), 100, Some("drs/r".into()), payload, None);
        let recalled = stale_entry("drs/r/RADAR/tracks".into(), 100, Some("drs/r".into()), payload, None);
        // Same classification + raw retention as a live entry...
        assert_eq!(recalled.format, live.format);
        assert_eq!(recalled.preview, live.preview);
        assert_eq!(recalled.raw_bytes, live.raw_bytes);
        assert_eq!(recalled.ts_ms, 100);
        // ...but flagged stale (live entries never are).
        assert!(!live.stale);
        assert!(recalled.stale);
    }

    #[test]
    fn empty_payload_classifies_as_empty() {
        let entry = build_entry("t".into(), 1, None, &[], None);
        assert_eq!(entry.format, SampleFormat::Empty);
        assert_eq!(entry.preview, "");
    }

    #[test]
    fn ring_buffer_caps_at_max_entries() {
        let mut buf = SampleBuffer::default();
        for i in 0..(MAX_ENTRIES_PER_TOPIC + 10) {
            buf.push("t".into(), i as u64, None, b"x", None);
        }
        let recent = buf.recent("t", 1000);
        assert_eq!(recent.len(), MAX_ENTRIES_PER_TOPIC);
        // Newest first.
        assert_eq!(recent[0].ts_ms, (MAX_ENTRIES_PER_TOPIC + 9) as u64);
    }

    #[test]
    fn large_payload_truncates_and_flags() {
        let big = vec![b'a'; MAX_PAYLOAD_BYTES + 100];
        let entry = build_entry("t".into(), 1, None, &big, None);
        assert!(entry.truncated);
        assert_eq!(entry.size_bytes, MAX_PAYLOAD_BYTES + 100);
    }
}
