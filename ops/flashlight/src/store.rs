// SPDX-License-Identifier: Apache-2.0
//
// SampleStore — a bounded, in-memory ring of recent samples' payload
// bytes, keyed by a monotonic ingest sequence number.
//
// Why this exists: the live SSE tail deliberately carries NO payload
// bytes — only key / ts / seq / format / length — to honor
// docs/design/minimalism-under-network-constraints.md (bytes don't
// flow over the wire until the operator asks for them). When the
// operator clicks a row, the browser fetches `GET /api/sample/{seq}`,
// which reads the bounded preview out of this store. Old samples evict
// as the ring wraps; a click on an evicted sample returns 404
// "expired", which the Tier 1 UI surfaces honestly rather than hanging.
//
// This is the substrate that turns flashlight from a rate meter (key +
// byte-count only) into an inspection surface (the actual payload) per
// docs/design/flashlight.md §"Tier 1 surfaces" → "Sample payload
// viewer (raw)".

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::Serialize;

/// Default number of samples retained in the ring. Sized for the nova-a
/// trial fabric's aggregate rate; an operator inspecting a focused
/// slice clicks within seconds of a row appearing, well inside the
/// retention window. Override via `FLASHLIGHT_SAMPLE_STORE_CAP`.
pub const DEFAULT_STORE_CAPACITY: usize = 1024;

/// Default per-sample byte cap for the retained preview. Matches the
/// design doc's "max 4 KiB inline" bound. Override via
/// `FLASHLIGHT_SAMPLE_PREVIEW_BYTES`.
pub const DEFAULT_PREVIEW_BYTES: usize = 4096;

/// Coarse wire-format classification at ingest. Prefers the
/// self-describing wire `Encoding` when the publisher set one (the only
/// way to know protobuf vs arbitrary binary), and falls back to a cheap
/// content heuristic otherwise. The full schema-aware decode still happens
/// on demand via the portal registry (Tier 2 decode view). Serialized
/// kebab-case for the SSE wire + the icon mapping in the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SampleFormat {
    /// First non-whitespace byte is `{` — looks like a JSON object.
    JsonObj,
    /// First non-whitespace byte is `[` — looks like a JSON array.
    JsonArr,
    /// Valid UTF-8 with no non-whitespace control bytes — human text.
    Text,
    /// Protobuf payload — known authoritatively from the wire `Encoding`
    /// (`application/protobuf`), never from byte-sniffing (protobuf is
    /// indistinguishable from arbitrary binary by content alone).
    Protobuf,
    /// CBOR payload — known authoritatively from the wire `Encoding`
    /// (`application/cbor`).
    Cbor,
    /// Non-text bytes with no self-describing encoding — render as hex.
    Binary,
    /// Zero-length payload.
    Empty,
}

impl SampleFormat {
    /// Classify a payload, preferring the self-describing wire `Encoding`
    /// over byte content when the publisher set one.
    ///
    /// `encoding` is the stringified Zenoh `Encoding`
    /// (`application/protobuf;TrackUpdate`, `application/json`,
    /// `application/cbor`, `text/plain`, …). The default/unset encoding
    /// (`zenoh/bytes`), `application/octet-stream`, or anything
    /// unrecognized falls through to content classification — so a
    /// publisher that sets no encoding is no worse off than before.
    ///
    /// This is the consumer half of the self-describing-encoding adoption
    /// (docs/design/zenoh-capability-adoption.md gap 1): the publishers we
    /// own set the encoding from their one declared format, and the
    /// inspection surfaces read it off the wire instead of guessing.
    pub fn classify_with_encoding(bytes: &[u8], encoding: Option<&str>) -> Self {
        if bytes.is_empty() {
            return SampleFormat::Empty;
        }
        if let Some(fmt) = encoding.and_then(Self::from_encoding) {
            // The encoding says JSON but can't say object vs array — refine
            // from the first non-whitespace byte. Every other mapped format
            // is taken as authoritative.
            return match fmt {
                SampleFormat::JsonObj => match bytes.iter().find(|b| !b.is_ascii_whitespace()) {
                    Some(b'[') => SampleFormat::JsonArr,
                    _ => SampleFormat::JsonObj,
                },
                other => other,
            };
        }
        SampleFormat::classify(bytes)
    }

    /// Map a stringified wire `Encoding` to a coarse format, or `None`
    /// when it carries no useful format signal (`zenoh/bytes`,
    /// `application/octet-stream`, unknown) so the caller falls back to
    /// content classification. JSON maps to `JsonObj` as the
    /// representative — callers that have the bytes refine object vs array.
    pub fn from_encoding(encoding: &str) -> Option<Self> {
        // Strip any `;schema` suffix and match the media type.
        let base = encoding.split(';').next().unwrap_or(encoding).trim();
        match base {
            "application/json" | "application/json-seq" | "application/json-patch+json" => {
                Some(SampleFormat::JsonObj)
            }
            "application/cbor" => Some(SampleFormat::Cbor),
            "application/protobuf" => Some(SampleFormat::Protobuf),
            "text/plain" => Some(SampleFormat::Text),
            _ => None,
        }
    }

    /// Classify a payload by content alone (the fallback when the wire
    /// carries no meaningful encoding). Pure + cheap; allocation-free.
    pub fn classify(bytes: &[u8]) -> Self {
        if bytes.is_empty() {
            return SampleFormat::Empty;
        }
        match std::str::from_utf8(bytes) {
            Ok(s) => match s.trim_start().as_bytes().first().copied() {
                Some(b'{') => SampleFormat::JsonObj,
                Some(b'[') => SampleFormat::JsonArr,
                _ => {
                    if s.chars().all(|c| !c.is_control() || c.is_whitespace()) {
                        SampleFormat::Text
                    } else {
                        SampleFormat::Binary
                    }
                }
            },
            Err(_) => SampleFormat::Binary,
        }
    }

    /// True when the preview is worth offering as decoded UTF-8 text in
    /// addition to the always-present hex view.
    pub fn is_texty(self) -> bool {
        matches!(
            self,
            SampleFormat::JsonObj | SampleFormat::JsonArr | SampleFormat::Text
        )
    }
}

/// One retained sample. `payload_len` is the TRUE wire length; `preview`
/// holds at most `preview_bytes` leading bytes.
#[derive(Debug, Clone)]
pub struct StoredSample {
    pub seq: u64,
    pub key: String,
    pub ts_ms: u64,
    pub format: SampleFormat,
    /// The stringified wire `Encoding` when the publisher set a meaningful
    /// one (`application/protobuf;TrackUpdate`, `application/json`, …);
    /// `None` for the default/unset `zenoh/bytes`. Lets the on-demand
    /// decode path pick a decoder from the wire — including the protobuf
    /// schema name in the `;schema` suffix — instead of an out-of-band
    /// portal lookup.
    pub encoding: Option<String>,
    pub payload_len: usize,
    pub preview: Vec<u8>,
}

impl StoredSample {
    /// True when the retained preview is shorter than the wire payload.
    pub fn truncated(&self) -> bool {
        self.preview.len() < self.payload_len
    }
}

/// Bounded ring of recent samples, shared across the broad Zenoh
/// subscriber (writer) and the `/api/sample/{seq}` handler (reader).
pub struct SampleStore {
    capacity: usize,
    preview_bytes: usize,
    seq: AtomicU64,
    /// Wall-clock ms of the most recent ingest, 0 before the first one.
    /// Read by `/api/stats` so an operator can see how long the tail has
    /// been silent without shelling onto the host — the signal that goes
    /// missing when flashlight runs blind with `/healthz` green.
    last_ingest_ms: AtomicU64,
    ring: Mutex<VecDeque<StoredSample>>,
}

impl SampleStore {
    pub fn new(capacity: usize, preview_bytes: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            preview_bytes,
            seq: AtomicU64::new(0),
            last_ingest_ms: AtomicU64::new(0),
            ring: Mutex::new(VecDeque::with_capacity(capacity.min(1024))),
        }
    }

    /// Build from `FLASHLIGHT_SAMPLE_STORE_CAP` +
    /// `FLASHLIGHT_SAMPLE_PREVIEW_BYTES`, falling back to defaults.
    pub fn from_env() -> Self {
        Self::new(
            env_usize("FLASHLIGHT_SAMPLE_STORE_CAP", DEFAULT_STORE_CAPACITY),
            env_usize("FLASHLIGHT_SAMPLE_PREVIEW_BYTES", DEFAULT_PREVIEW_BYTES),
        )
    }

    /// Assign the next monotonic seq, capture a bounded preview, retain
    /// it (evicting the oldest if at capacity), and return
    /// `(seq, format)` for the SSE event. Bytes beyond the preview cap
    /// are dropped from retention but the returned/stored `payload_len`
    /// keeps the true length.
    ///
    /// Seq assignment happens under the ring lock so the deque stays
    /// strictly ascending by seq even under concurrent ingest from
    /// multiple subscribe loops — `get`'s binary search depends on it.
    pub fn ingest(
        &self,
        key: String,
        ts_ms: u64,
        payload: &[u8],
        encoding: Option<&str>,
    ) -> (u64, SampleFormat) {
        let format = SampleFormat::classify_with_encoding(payload, encoding);
        let take = payload.len().min(self.preview_bytes);
        let preview = payload[..take].to_vec();
        let payload_len = payload.len();

        self.last_ingest_ms.store(ts_ms, Ordering::Relaxed);
        let mut ring = self.ring.lock().unwrap();
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        if ring.len() == self.capacity {
            ring.pop_front();
        }
        ring.push_back(StoredSample {
            seq,
            key,
            ts_ms,
            format,
            encoding: encoding.map(str::to_owned),
            payload_len,
            preview,
        });
        (seq, format)
    }

    /// Look up a retained sample by seq. `None` when it never existed
    /// or has evicted out of the ring.
    pub fn get(&self, seq: u64) -> Option<StoredSample> {
        let ring = self.ring.lock().unwrap();
        ring.binary_search_by_key(&seq, |s| s.seq)
            .ok()
            .map(|idx| ring[idx].clone())
    }

    /// Current number of retained samples (diagnostics / tests).
    pub fn len(&self) -> usize {
        self.ring.lock().unwrap().len()
    }

    /// Wall-clock ms of the most recent ingest, or `None` before the
    /// first sample. Distinguishes "nothing has ever arrived" from "the
    /// tail went quiet at T" — an absence verdict that names its horizon
    /// rather than rounding cannot-tell into no.
    pub fn last_ingest_ms(&self) -> Option<u64> {
        match self.last_ingest_ms.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(ms),
        }
    }

    #[allow(dead_code)] // companion to len() for clippy::len_without_is_empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_json_object() {
        assert_eq!(SampleFormat::classify(b"{\"a\":1}"), SampleFormat::JsonObj);
        // Leading whitespace tolerated.
        assert_eq!(SampleFormat::classify(b"  \n{}"), SampleFormat::JsonObj);
    }

    #[test]
    fn classify_json_array() {
        assert_eq!(SampleFormat::classify(b"[1,2,3]"), SampleFormat::JsonArr);
    }

    #[test]
    fn classify_text() {
        assert_eq!(SampleFormat::classify(b"hello world"), SampleFormat::Text);
    }

    #[test]
    fn classify_binary_on_control_bytes() {
        // Protobuf-ish bytes with field tags + control chars.
        assert_eq!(
            SampleFormat::classify(&[0x08, 0x96, 0x01, 0x12, 0x03]),
            SampleFormat::Binary
        );
    }

    #[test]
    fn classify_binary_on_invalid_utf8() {
        assert_eq!(
            SampleFormat::classify(&[0xff, 0xfe, 0xfd]),
            SampleFormat::Binary
        );
    }

    #[test]
    fn classify_empty() {
        assert_eq!(SampleFormat::classify(b""), SampleFormat::Empty);
    }

    #[test]
    fn encoding_makes_protobuf_authoritative() {
        // Protobuf bytes sniff as Binary; the wire encoding makes them
        // authoritatively Protobuf, schema suffix and all.
        let pb = &[0x08u8, 0x96, 0x01, 0x12, 0x03];
        assert_eq!(SampleFormat::classify(pb), SampleFormat::Binary);
        assert_eq!(
            SampleFormat::classify_with_encoding(pb, Some("application/protobuf;TrackUpdate")),
            SampleFormat::Protobuf
        );
    }

    #[test]
    fn encoding_cbor_and_text() {
        assert_eq!(
            SampleFormat::classify_with_encoding(&[0xa1, 0x61, 0x78], Some("application/cbor")),
            SampleFormat::Cbor
        );
        // text/plain encoding wins even when the bytes look JSON-ish.
        assert_eq!(
            SampleFormat::classify_with_encoding(b"{not json}", Some("text/plain;charset=utf-8")),
            SampleFormat::Text
        );
    }

    #[test]
    fn encoding_json_refines_object_vs_array_from_bytes() {
        // The encoding says JSON; the bytes decide object vs array.
        assert_eq!(
            SampleFormat::classify_with_encoding(b"[1,2]", Some("application/json")),
            SampleFormat::JsonArr
        );
        assert_eq!(
            SampleFormat::classify_with_encoding(b"{\"a\":1}", Some("application/json")),
            SampleFormat::JsonObj
        );
    }

    #[test]
    fn unset_or_octet_encoding_falls_back_to_content() {
        // Default/unset and octet-stream carry no format signal → sniff.
        assert_eq!(
            SampleFormat::classify_with_encoding(b"{\"a\":1}", Some("zenoh/bytes")),
            SampleFormat::JsonObj
        );
        assert_eq!(
            SampleFormat::classify_with_encoding(b"hello", Some("application/octet-stream")),
            SampleFormat::Text
        );
        assert_eq!(
            SampleFormat::classify_with_encoding(b"hello", None),
            SampleFormat::Text
        );
    }

    #[test]
    fn empty_payload_is_empty_regardless_of_encoding() {
        assert_eq!(
            SampleFormat::classify_with_encoding(b"", Some("application/protobuf;X")),
            SampleFormat::Empty
        );
    }

    #[test]
    fn from_encoding_maps_known_media_types() {
        assert_eq!(SampleFormat::from_encoding("application/cbor"), Some(SampleFormat::Cbor));
        assert_eq!(
            SampleFormat::from_encoding("application/protobuf;Foo"),
            Some(SampleFormat::Protobuf)
        );
        assert_eq!(SampleFormat::from_encoding("zenoh/bytes"), None);
        assert_eq!(SampleFormat::from_encoding("application/octet-stream"), None);
    }

    #[test]
    fn ingest_stores_wire_encoding() {
        let store = SampleStore::new(4, 4096);
        let (seq, fmt) = store.ingest(
            "v/sys/v1".into(),
            1,
            &[0x08, 0x01],
            Some("application/protobuf;TrackUpdate"),
        );
        assert_eq!(fmt, SampleFormat::Protobuf);
        let s = store.get(seq).unwrap();
        assert_eq!(s.encoding.as_deref(), Some("application/protobuf;TrackUpdate"));
    }

    #[test]
    fn last_ingest_is_none_before_any_sample() {
        // "Nothing has ever arrived" must stay distinguishable from
        // "arrived at epoch 0" — the whole point of the field is telling
        // never-started apart from went-quiet.
        let store = SampleStore::new(8, 4096);
        assert_eq!(store.last_ingest_ms(), None);
    }

    #[test]
    fn last_ingest_tracks_the_newest_sample() {
        let store = SampleStore::new(8, 4096);
        store.ingest("k/0".into(), 1_700_000_000_000, b"a", None);
        assert_eq!(store.last_ingest_ms(), Some(1_700_000_000_000));
        store.ingest("k/1".into(), 1_700_000_005_000, b"b", None);
        assert_eq!(store.last_ingest_ms(), Some(1_700_000_005_000));
    }

    #[test]
    fn last_ingest_survives_ring_eviction() {
        // A full ring that stops moving reads the same as a cycling one
        // on `len()`; the timestamp is what still moves, so it must not
        // be tied to ring occupancy.
        let store = SampleStore::new(2, 4096);
        for (i, ts) in [10u64, 20, 30].into_iter().enumerate() {
            store.ingest(format!("k/{i}"), ts, b"x", None);
        }
        assert_eq!(store.len(), 2, "ring evicted as expected");
        assert_eq!(store.last_ingest_ms(), Some(30));
    }

    #[test]
    fn ingest_assigns_monotonic_seq_from_zero() {
        let store = SampleStore::new(8, 4096);
        let (s0, _) = store.ingest("k/0".into(), 1, b"a", None);
        let (s1, _) = store.ingest("k/1".into(), 2, b"b", None);
        assert_eq!(s0, 0);
        assert_eq!(s1, 1);
    }

    #[test]
    fn get_returns_stored_preview() {
        let store = SampleStore::new(8, 4096);
        let (seq, fmt) = store.ingest("vendor-a/obs/v1".into(), 42, b"{\"x\":1}", None);
        assert_eq!(fmt, SampleFormat::JsonObj);
        let s = store.get(seq).expect("sample retained");
        assert_eq!(s.key, "vendor-a/obs/v1");
        assert_eq!(s.ts_ms, 42);
        assert_eq!(s.preview, b"{\"x\":1}");
        assert!(!s.truncated());
    }

    #[test]
    fn preview_is_capped_but_len_is_true() {
        let store = SampleStore::new(8, 4);
        let payload = b"0123456789";
        let (seq, _) = store.ingest("k".into(), 0, payload, None);
        let s = store.get(seq).unwrap();
        assert_eq!(s.payload_len, 10);
        assert_eq!(s.preview, b"0123");
        assert!(s.truncated());
    }

    #[test]
    fn ring_evicts_oldest_at_capacity() {
        let store = SampleStore::new(3, 4096);
        for i in 0..5u64 {
            store.ingest(format!("k/{i}"), i, b"x", None);
        }
        assert_eq!(store.len(), 3);
        // seq 0 + 1 evicted; 2,3,4 retained.
        assert!(store.get(0).is_none());
        assert!(store.get(1).is_none());
        assert!(store.get(2).is_some());
        assert!(store.get(4).is_some());
    }

    #[test]
    fn get_missing_seq_is_none() {
        let store = SampleStore::new(8, 4096);
        store.ingest("k".into(), 0, b"x", None);
        assert!(store.get(999).is_none());
    }
}
