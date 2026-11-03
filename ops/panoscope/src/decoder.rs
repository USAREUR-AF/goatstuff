// SPDX-License-Identifier: Apache-2.0
//
// Descriptor cache + dynamic protobuf decoder for the side-panel
// "Decoded" stream view.
//
// Hot path: the ring-buffer raw bytes (see sample_buffer.rs) go through
// `decode_sample` against a cached `MessageDescriptor`. Cache miss fires
// one `.fds` fetch via PortalClient, parses it into a `DescriptorPool`,
// and picks the message *declared by the topic's `format` field*
// (`protobuf:<Name>`). Falls back to the first MessageDescriptor in the
// pool only when topic.format is unset or non-protobuf.
//
// Multi-message-FDS handling: `protoc --include_imports` produces an
// FDS whose first FileDescriptorProto is typically `google/protobuf/...`
// or a shared `common/...` import (e.g. `LatLongAlt`, `Vector3`),
// not the vendor's actual wire message. Earlier the decoder
// silently picked that first import as the decode target, producing
// nonsensical "invalid wire type" errors that looked like data
// corruption. Now the decoder honors the
// topic's declared format so a multi-message FDS routes to the right
// message.
//
// Cache discipline:
//
//   - `Ready` entries: never expire. Schemas are append-only by version;
//     a v1 descriptor stays valid forever. If a vendor uploads v2, the
//     cache still serves v1's descriptor — operator-visible drift is fine
//     for the trial fabric ("you're decoding against v1"), the production
//     path tracks schema_version on every decoded message.
//   - `NoSchema` entries: 60s TTL. Lets a newly-uploaded schema surface
//     within a minute without flooding the portal with 404s every refresh.
//   - `Transient` failures: 10s TTL. Slow enough that a flapping portal
//     doesn't get DDoSed; fast enough that an operator who notices "no
//     decode" can fix the portal and see results within seconds.
//
// Design memo: docs/design/panoscope.md §"Decoded stream view".

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};
use panoscope_core::PortalError;
use serde::Serialize;
use serde_json::Value as JsonValue;
use tokio::sync::Mutex;
use tracing::{info, warn};

use panoscope_core::{FetchOutcome, PortalClient};

/// How long a NoSchema verdict stays cached before re-asking the portal.
const NO_SCHEMA_TTL: Duration = Duration::from_secs(60);

/// How long a transient-failure verdict stays cached before retry.
const TRANSIENT_TTL: Duration = Duration::from_secs(10);

/// One sample's decode result, returned per-sample from
/// `Decoder::decode_topic_samples`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleDecodeResult {
    /// Decoded successfully — the embedded JSON is what the operator
    /// sees in the "Decoded" stream pane.
    Decoded(JsonValue),
    /// NOT protobuf-decoded — the topic has no `.fds`, but the wire bytes
    /// are already structured (JSON) or human-readable (text), so we show
    /// them as-is rather than an empty "no schema" pane. The frontend MUST
    /// render this with a visible "passthrough" marker so an operator never
    /// mistakes it for a schema-validated protobuf decode. For JSON the
    /// payload is the parsed value; for text it's a JSON string. The
    /// `format` names which (`"json"` | `"text"`).
    Passthrough { format: String, body: JsonValue },
    /// Sample's bytes didn't fit the descriptor — operator-visible error
    /// message; the rest of the topic's samples still render.
    Error(String),
    /// Sample was retained but exceeds MAX_PAYLOAD_BYTES so raw_bytes is
    /// None — decoder can't replay. Distinct from `Error` so the frontend
    /// can render "[raw bytes not retained for samples >4 KiB]" rather
    /// than a misleading decode-error message.
    NoRawBytes,
}

/// Topic-level decode outcome. Wraps per-sample results plus a topic-wide
/// status flag the frontend uses for the toggle / NO SCHEMA badge.
#[derive(Debug, Clone, Serialize)]
pub struct TopicDecodeOutcome {
    pub topic: String,
    pub status: TopicDecodeStatus,
    /// Empty when status != Decoded.
    pub samples: Vec<DecodedSampleRow>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TopicDecodeStatus {
    /// Descriptor was loaded; per-sample results are in `samples`.
    Decoded,
    /// No protobuf `.fds` for this topic, but the samples are JSON/text and
    /// are shown via passthrough (per-sample `Passthrough` results). The
    /// frontend badges this distinctly from `Decoded` ("PASSTHROUGH — no
    /// protobuf schema") so the operator knows the bytes were not validated
    /// against a schema. Reached for schemaless JSON vendors (e.g. vendor-d)
    /// and for stale recalled JSON samples alike.
    Passthrough,
    /// Portal returned 404 for this topic — no `.fds` has been uploaded.
    /// Frontend renders the NO SCHEMA badge. Only reached when passthrough
    /// did NOT apply (samples are binary / protobuf-shaped, genuinely
    /// needing a schema), so the upload-a-schema guidance stays correct.
    NoSchema,
    /// Portal client isn't configured (env vars missing); frontend
    /// renders NO SCHEMA but the badge tooltip can hint at the deploy gap.
    PortalDisabled,
    /// Descriptor was fetched but contained no message types — operator-
    /// uploaded `.fds` is structurally empty. Rare; surfaces as a distinct
    /// status so we don't blame "no schema" when one exists but is unusable.
    EmptyDescriptor,
    /// Portal returned transient error; operator can retry.
    Transient(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct DecodedSampleRow {
    pub ts_ms: u64,
    pub source_peer: Option<String>,
    pub size_bytes: usize,
    pub result: SampleDecodeResult,
    /// True when the underlying sample was recalled from Zenoh storage on
    /// an empty live ring (panoscope-stale-recall B1) rather than observed
    /// live. `ts_ms` is then the original publish time. The frontend
    /// renders the decoded-stream pane stale (desaturated + "last seen Ns
    /// ago"); never presented as live. Defaults false.
    #[serde(default)]
    pub stale: bool,
}

#[derive(Clone)]
enum CacheState {
    Ready {
        descriptor: MessageDescriptor,
    },
    NoSchema {
        cached_at: Instant,
    },
    Transient {
        msg: String,
        cached_at: Instant,
    },
}

pub struct Decoder {
    portal: PortalClient,
    cache: Mutex<HashMap<String, CacheState>>,
}

impl Decoder {
    pub fn new(portal: PortalClient) -> Arc<Self> {
        Arc::new(Self {
            portal,
            cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn portal_enabled(&self) -> bool {
        self.portal.is_enabled()
    }

    /// Resolve a descriptor for `topic_key`, honoring the cache. Returns
    /// `Ok(Some(_))` when the descriptor is ready, `Ok(None)` when the
    /// portal definitively reports no schema, and `Err(_)` for transient
    /// failures the caller surfaces to the operator.
    async fn resolve_descriptor(
        &self,
        topic_key: &str,
        wire_msg: Option<&str>,
    ) -> Result<Option<MessageDescriptor>, ResolveError> {
        // Fast path — read-only cache hit.
        {
            let cache = self.cache.lock().await;
            if let Some(state) = cache.get(topic_key) {
                match state {
                    CacheState::Ready { descriptor } => {
                        return Ok(Some(descriptor.clone()));
                    }
                    CacheState::NoSchema { cached_at } => {
                        if cached_at.elapsed() < NO_SCHEMA_TTL {
                            return Ok(None);
                        }
                    }
                    CacheState::Transient { msg, cached_at } => {
                        if cached_at.elapsed() < TRANSIENT_TTL {
                            return Err(ResolveError::Transient(msg.clone()));
                        }
                    }
                }
            }
        }

        // Cache miss / TTL expired — fetch from portal. Two roundtrips:
        // (1) the topic row, for its declared `format: protobuf:<Name>`;
        // (2) the .fds bytes. Topic lookup is best-effort — when it
        // fails we fall back to the first-message-in-FDS heuristic so a
        // partial portal outage still yields some decode rather than
        // erroring across the board.
        //
        // Message-name priority: the portal topic row's declared `format`
        // is the source of truth; the sample's self-describing wire
        // encoding (`wire_msg`) is the fallback when the row doesn't
        // declare one — which also lets a registered-but-format-less topic
        // (or one fetched while the portal's row lookup is flaky) still
        // select the right message instead of guessing the first in the FDS
        // (docs/design/zenoh-capability-adoption.md gap 1).
        let declared_msg = match self.portal.find_topic_by_key(topic_key).await {
            Ok(Some(t)) => parse_protobuf_format(&t.format),
            Ok(None) => None,
            Err(PortalError::Disabled) | Err(PortalError::NotFound { .. }) => None,
            Err(e) => {
                warn!(topic = %topic_key, error = %e, "find_topic_by_key transient — falling back to first-message");
                None
            }
        }
        .or_else(|| wire_msg.map(str::to_string));

        match self.portal.fetch_fds(topic_key).await {
            FetchOutcome::Ready(bytes) => match select_message(&bytes, declared_msg.as_deref()) {
                Ok(Some((descriptor, selection))) => {
                    info!(
                        topic = %topic_key,
                        message = %descriptor.full_name(),
                        bytes = bytes.len(),
                        selection = %selection,
                        "panoscope cached protobuf descriptor for topic"
                    );
                    self.cache.lock().await.insert(
                        topic_key.to_string(),
                        CacheState::Ready {
                            descriptor: descriptor.clone(),
                        },
                    );
                    Ok(Some(descriptor))
                }
                Ok(None) => Err(ResolveError::EmptyDescriptor),
                Err(e) => {
                    warn!(topic = %topic_key, error = %e, "FDS parse failed");
                    Err(ResolveError::Transient(format!("FDS parse: {e}")))
                }
            },
            FetchOutcome::NoSchema | FetchOutcome::Disabled => {
                self.cache.lock().await.insert(
                    topic_key.to_string(),
                    CacheState::NoSchema {
                        cached_at: Instant::now(),
                    },
                );
                Ok(None)
            }
            FetchOutcome::Transient(msg) => {
                self.cache.lock().await.insert(
                    topic_key.to_string(),
                    CacheState::Transient {
                        msg: msg.clone(),
                        cached_at: Instant::now(),
                    },
                );
                Err(ResolveError::Transient(msg))
            }
        }
    }

    /// Decode every sample in `samples` against the resolved descriptor
    /// for `topic_key`. Returns a topic-level outcome that wraps per-
    /// sample results — one bad sample doesn't sink the rest.
    pub async fn decode_topic_samples(
        &self,
        topic_key: &str,
        samples: &[crate::sample_buffer::SampleEntry],
    ) -> TopicDecodeOutcome {
        // Resolve the protobuf descriptor. Every "no usable protobuf schema"
        // outcome (portal disabled, 404 no-schema, structurally-empty .fds)
        // falls through to JSON/text passthrough so schemaless-but-structured
        // topics still render — `no_schema_status` is the status to surface
        // if passthrough does NOT apply (binary samples genuinely needing a
        // schema). A *transient* portal error keeps its retry status and does
        // NOT passthrough — the schema may yet resolve.
        // The self-describing wire encoding (when a publisher set one) names
        // the protobuf message; use it as the message-selection fallback for
        // descriptor resolution. Consistent per topic, so the first sample
        // that carries a `protobuf:<Name>` encoding is representative.
        let wire_msg = samples
            .iter()
            .find_map(|s| s.encoding.as_deref().and_then(protobuf_msg_from_encoding));
        let (descriptor, no_schema_status) = if !self.portal.is_enabled() {
            (None, TopicDecodeStatus::PortalDisabled)
        } else {
            match self.resolve_descriptor(topic_key, wire_msg).await {
                Ok(Some(d)) => (Some(d), TopicDecodeStatus::Decoded),
                Ok(None) => (None, TopicDecodeStatus::NoSchema),
                Err(ResolveError::EmptyDescriptor) => (None, TopicDecodeStatus::EmptyDescriptor),
                Err(ResolveError::Transient(msg)) => {
                    return TopicDecodeOutcome {
                        topic: topic_key.to_string(),
                        status: TopicDecodeStatus::Transient(msg),
                        samples: Vec::new(),
                    };
                }
            }
        };
        let Some(descriptor) = descriptor else {
            return self.passthrough_outcome(topic_key, samples, no_schema_status);
        };

        let mut out = Vec::with_capacity(samples.len());
        for s in samples {
            let result = match &s.raw_bytes {
                None => SampleDecodeResult::NoRawBytes,
                Some(bytes) => match DynamicMessage::decode(descriptor.clone(), bytes.as_slice()) {
                    Ok(msg) => match serde_json::to_value(&msg) {
                        Ok(v) => SampleDecodeResult::Decoded(v),
                        Err(e) => SampleDecodeResult::Error(format!("json serialize: {e}")),
                    },
                    Err(e) => SampleDecodeResult::Error(format!("decode: {e}")),
                },
            };
            out.push(DecodedSampleRow {
                ts_ms: s.ts_ms,
                source_peer: s.source_peer.clone(),
                size_bytes: s.size_bytes,
                result,
                stale: s.stale,
            });
        }
        TopicDecodeOutcome {
            topic: topic_key.to_string(),
            status: TopicDecodeStatus::Decoded,
            samples: out,
        }
    }

    /// JSON/text passthrough fallback for topics with no usable protobuf
    /// schema. Builds `Passthrough` rows for every JSON/text sample so a
    /// schemaless-but-structured topic (e.g. a JSON vendor like vendor-d, or
    /// a stale recalled JSON sample) still renders in the decoded pane —
    /// clearly marked as passthrough, never as a protobuf decode. When NO
    /// sample is JSON/text (binary / protobuf-shaped bytes that genuinely
    /// need a `.fds`), returns `fallback_status` with no samples so the
    /// frontend keeps the correct upload-a-schema guidance.
    fn passthrough_outcome(
        &self,
        topic_key: &str,
        samples: &[crate::sample_buffer::SampleEntry],
        fallback_status: TopicDecodeStatus,
    ) -> TopicDecodeOutcome {
        let rows: Vec<DecodedSampleRow> = samples.iter().filter_map(passthrough_row).collect();
        if rows.is_empty() {
            return TopicDecodeOutcome {
                topic: topic_key.to_string(),
                status: fallback_status,
                samples: Vec::new(),
            };
        }
        TopicDecodeOutcome {
            topic: topic_key.to_string(),
            status: TopicDecodeStatus::Passthrough,
            samples: rows,
        }
    }
}

/// Build a `Passthrough` row for a JSON or text sample. Returns `None` for
/// any other classified format (cbor / hex / protobuf-without-schema /
/// empty) — those aren't human-structured without a `.fds`, so the caller
/// keeps the no-schema signal for them. Mirrors `decode_topic_samples`'s
/// row shape (carrying `stale` through unchanged).
fn passthrough_row(s: &crate::sample_buffer::SampleEntry) -> Option<DecodedSampleRow> {
    use crate::sample_buffer::SampleFormat;
    let result = match s.format {
        SampleFormat::Json => {
            // Parse the original bytes; fall back to the pretty-printed
            // preview if raw bytes weren't retained (>4 KiB cap).
            let parsed = s
                .raw_bytes
                .as_ref()
                .and_then(|b| serde_json::from_slice::<JsonValue>(b).ok())
                .or_else(|| serde_json::from_str::<JsonValue>(&s.preview).ok())?;
            SampleDecodeResult::Passthrough {
                format: "json".to_string(),
                body: parsed,
            }
        }
        SampleFormat::Text => SampleDecodeResult::Passthrough {
            format: "text".to_string(),
            body: JsonValue::String(s.preview.clone()),
        },
        _ => return None,
    };
    Some(DecodedSampleRow {
        ts_ms: s.ts_ms,
        source_peer: s.source_peer.clone(),
        size_bytes: s.size_bytes,
        result,
        stale: s.stale,
    })
}

enum ResolveError {
    Transient(String),
    EmptyDescriptor,
}

/// Extract `<Name>` from a topic's `format` field when it's shaped as
/// `protobuf:<Name>`. Empty `protobuf:` (no name) and non-protobuf
/// formats return `None` — the decoder falls back to the first-message
/// heuristic for those, preserving the earlier behavior on
/// topics that don't declare a specific message.
fn parse_protobuf_format(format: &str) -> Option<String> {
    let rest = format.strip_prefix("protobuf:")?;
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    }
}

/// Extract the protobuf message name from a self-describing wire
/// `Encoding` string. `application/protobuf;TrackUpdate` → `TrackUpdate`;
/// a bare `application/protobuf` (no schema) or any non-protobuf encoding
/// → `None`. The schema after `;` is what zenoh's `Encoding::with_schema`
/// stamps and what the publishers we own set from their declared format.
fn protobuf_msg_from_encoding(encoding: &str) -> Option<&str> {
    let (media, schema) = encoding.split_once(';')?;
    if media.trim() != "application/protobuf" {
        return None;
    }
    let name = schema.trim();
    (!name.is_empty()).then_some(name)
}

/// Reason the descriptor was chosen — included in cache-write log so an
/// operator scanning panoscope logs can spot multi-message-FDS topics
/// that aren't honoring the declared format.
#[derive(Debug)]
enum SelectionReason {
    /// Declared `protobuf:<Name>` matched a top-level (or nested)
    /// message in the descriptor pool.
    DeclaredFormat,
    /// Topic didn't declare a protobuf:<Name> format, or the portal
    /// lookup failed — fell back to the first message in the FDS.
    FirstMessageFallback,
    /// Topic declared `protobuf:<Name>` but the name isn't anywhere in
    /// the FDS; took the first message as a last-resort fallback so the
    /// stream pane still renders SOMETHING (the operator will see the
    /// mismatched-name warning in the log and trip the read-side
    /// schema-body check).
    DeclaredButNotFoundFallback,
}

impl std::fmt::Display for SelectionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SelectionReason::DeclaredFormat => f.write_str("declared-format"),
            SelectionReason::FirstMessageFallback => f.write_str("first-message-fallback"),
            SelectionReason::DeclaredButNotFoundFallback => {
                f.write_str("declared-but-not-found-fallback")
            }
        }
    }
}

/// Parse a FileDescriptorSet and pick the right message descriptor.
///
/// When `declared_msg` carries a name from the topic's `protobuf:<Name>`
/// format, prefer that — look up by fully-qualified name first, then
/// by simple name across every message in the pool (top-level OR
/// nested). When the declared name isn't found, fall back to the first
/// message so the cache still produces SOMETHING and the operator sees
/// a warning in the log.
///
/// When `declared_msg` is None (topic format isn't `protobuf:<Name>`,
/// or the portal lookup failed transiently), preserves the original
/// behavior of picking the first message in the pool.
fn select_message(
    fds_bytes: &[u8],
    declared_msg: Option<&str>,
) -> anyhow::Result<Option<(MessageDescriptor, SelectionReason)>> {
    let pool = DescriptorPool::decode(fds_bytes)?;
    if let Some(name) = declared_msg {
        // 1. Fully-qualified match (e.g. "radar1.Status").
        if let Some(d) = pool.get_message_by_name(name) {
            return Ok(Some((d, SelectionReason::DeclaredFormat)));
        }
        // 2. Simple-name walk over every message (top-level + nested).
        if let Some(d) = pool.all_messages().find(|m| m.name() == name) {
            return Ok(Some((d, SelectionReason::DeclaredFormat)));
        }
        // 3. Declared but not present — fall back, log a warning at the
        //    call site (resolve_descriptor) so operators see it. Bind the
        //    iterator's first element to a local so the `all_messages()`
        //    temporary (which borrows `pool`) is dropped before the
        //    function returns — `MessageDescriptor` is Arc-backed and
        //    lifetime-free, so it outlives `pool` on its own.
        warn!(declared = %name, "declared protobuf message not found in FDS — falling back to first message");
        let first = pool.all_messages().next();
        return Ok(first.map(|d| (d, SelectionReason::DeclaredButNotFoundFallback)));
    }
    let first = pool.all_messages().next();
    Ok(first.map(|d| (d, SelectionReason::FirstMessageFallback)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample_buffer::{SampleEntry, SampleFormat};

    #[tokio::test]
    async fn disabled_portal_short_circuits_to_portal_disabled_status() {
        let decoder = Decoder::new(PortalClient::new(String::new(), String::new()));
        let outcome = decoder.decode_topic_samples("notional/foo/v1", &[]).await;
        assert!(matches!(outcome.status, TopicDecodeStatus::PortalDisabled));
        assert!(outcome.samples.is_empty());
    }

    /// JSON-classified sample, no protobuf schema → passthrough (not the
    /// empty NO-SCHEMA pane), clearly typed as passthrough json.
    #[tokio::test]
    async fn no_schema_json_sample_falls_through_to_passthrough() {
        let decoder = Decoder::new(PortalClient::new(String::new(), String::new()));
        let s = crate::sample_buffer::stale_entry(
            "vendor-d/feed/test".into(),
            42,
            Some("vendor-d/feed".into()),
            br#"{"hello":"vendor-d","n":7}"#,
            None,
        );
        assert_eq!(s.format, SampleFormat::Json); // sanity: classified JSON
        let outcome = decoder.decode_topic_samples("vendor-d/feed/test", &[s]).await;
        assert!(matches!(outcome.status, TopicDecodeStatus::Passthrough));
        assert_eq!(outcome.samples.len(), 1);
        let row = &outcome.samples[0];
        assert!(row.stale); // stale flag rides through passthrough
        match &row.result {
            SampleDecodeResult::Passthrough { format, body } => {
                assert_eq!(format, "json");
                assert_eq!(body["hello"], "vendor-d");
                assert_eq!(body["n"], 7);
            }
            other => panic!("expected Passthrough json, got {other:?}"),
        }
    }

    /// Binary / protobuf-shaped bytes with no schema must NOT passthrough —
    /// they genuinely need a `.fds`, so the no-schema signal is preserved.
    #[tokio::test]
    async fn no_schema_binary_sample_keeps_no_schema_signal() {
        let decoder = Decoder::new(PortalClient::new(String::new(), String::new()));
        // 0x0A 0x03 ... = protobuf wire shape (field 1, length-delimited).
        let s = crate::sample_buffer::stale_entry(
            "drs/x/RADAR/tracks".into(),
            1,
            None,
            &[0x0a, 0x03, 0x66, 0x6f, 0x6f],
            None,
        );
        assert_eq!(s.format, SampleFormat::Protobuf);
        let outcome = decoder.decode_topic_samples("drs/x/RADAR/tracks", &[s]).await;
        // Portal disabled here → fallback status is PortalDisabled, no rows.
        assert!(matches!(outcome.status, TopicDecodeStatus::PortalDisabled));
        assert!(outcome.samples.is_empty());
    }

    /// Text-classified sample passes through as a json string.
    #[tokio::test]
    async fn no_schema_text_sample_passthrough_as_string() {
        let decoder = Decoder::new(PortalClient::new(String::new(), String::new()));
        let s = crate::sample_buffer::stale_entry("x/note/v1".into(), 1, None, b"hello world", None);
        assert_eq!(s.format, SampleFormat::Text);
        let outcome = decoder.decode_topic_samples("x/note/v1", &[s]).await;
        assert!(matches!(outcome.status, TopicDecodeStatus::Passthrough));
        match &outcome.samples[0].result {
            SampleDecodeResult::Passthrough { format, body } => {
                assert_eq!(format, "text");
                assert_eq!(body, "hello world");
            }
            other => panic!("expected Passthrough text, got {other:?}"),
        }
    }

    #[test]
    fn select_message_returns_none_on_empty_fds() {
        // An empty FileDescriptorSet decodes from an empty buffer (zero
        // fields encoded). prost-reflect accepts that and yields a pool
        // with no messages — select_message returns None regardless of
        // whether a format was declared.
        assert!(select_message(&[], None).expect("parse ok").is_none());
        assert!(select_message(&[], Some("Status"))
            .expect("parse ok")
            .is_none());
    }

    #[test]
    fn parse_protobuf_format_extracts_name_or_none() {
        // The happy path: `protobuf:<Name>` yields the bare message name
        // used to route a multi-message .fds to the right descriptor.
        assert_eq!(
            parse_protobuf_format("protobuf:Status").as_deref(),
            Some("Status")
        );
        assert_eq!(
            parse_protobuf_format("protobuf:radar1.Tracks").as_deref(),
            Some("radar1.Tracks")
        );
        // Empty name after the prefix → None (fall back to first-message,
        // not an empty-string lookup that can never match).
        assert!(parse_protobuf_format("protobuf:").is_none());
        // Non-protobuf formats (json, cbor, the dropdown default) → None,
        // preserving first-message behavior for them.
        assert!(parse_protobuf_format("json").is_none());
        assert!(parse_protobuf_format("").is_none());
    }

    #[test]
    fn protobuf_msg_from_encoding_extracts_schema() {
        // The wire encoding's `;schema` suffix names the protobuf message,
        // the gap-1 fallback when the portal row declares no format.
        assert_eq!(
            protobuf_msg_from_encoding("application/protobuf;Status"),
            Some("Status")
        );
        assert_eq!(
            protobuf_msg_from_encoding("application/protobuf; radar1.Tracks "),
            Some("radar1.Tracks")
        );
        // Bare protobuf (no schema) and non-protobuf encodings → None, so
        // resolution falls through to the portal/first-message path.
        assert_eq!(protobuf_msg_from_encoding("application/protobuf"), None);
        assert_eq!(protobuf_msg_from_encoding("application/json"), None);
        assert_eq!(protobuf_msg_from_encoding("application/cbor;Foo"), None);
    }

    /// Build a two-message FileDescriptorSet where the vendor's wire
    /// message (`Status`) is NOT the first descriptor — `LatLongAlt`
    /// (standing in for the `protoc --include_imports` shared-import type)
    /// sorts first. This is the exact F-162 shape.
    fn two_message_fds() -> Vec<u8> {
        use prost::Message;
        use prost_types::{DescriptorProto, FileDescriptorProto, FileDescriptorSet};
        let file = FileDescriptorProto {
            name: Some("radar.proto".to_string()),
            package: Some("radar1".to_string()),
            message_type: vec![
                DescriptorProto {
                    name: Some("LatLongAlt".to_string()),
                    ..Default::default()
                },
                DescriptorProto {
                    name: Some("Status".to_string()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let set = FileDescriptorSet { file: vec![file] };
        let mut buf = Vec::new();
        set.encode(&mut buf).expect("encode fds");
        buf
    }

    #[test]
    fn select_message_prefers_declared_over_first() {
        let fds = two_message_fds();

        // Declared format names the non-first message → decoder picks it,
        // NOT the LatLongAlt import that sorts first (the F-162 bug).
        let (desc, reason) = select_message(&fds, Some("Status"))
            .expect("parse ok")
            .expect("some descriptor");
        assert_eq!(desc.name(), "Status");
        assert!(matches!(reason, SelectionReason::DeclaredFormat));

        // Fully-qualified name resolves too (get_message_by_name path).
        let (desc, _) = select_message(&fds, Some("radar1.Status"))
            .expect("parse ok")
            .expect("some descriptor");
        assert_eq!(desc.name(), "Status");

        // No declared format → first-message fallback (pre-fix behavior).
        let (desc, reason) = select_message(&fds, None)
            .expect("parse ok")
            .expect("some descriptor");
        assert_eq!(desc.name(), "LatLongAlt");
        assert!(matches!(reason, SelectionReason::FirstMessageFallback));

        // Declared but absent from the FDS → first-message fallback, but
        // flagged distinctly so the operator sees the drift in the log.
        let (desc, reason) = select_message(&fds, Some("Nonexistent"))
            .expect("parse ok")
            .expect("some descriptor");
        assert_eq!(desc.name(), "LatLongAlt");
        assert!(matches!(reason, SelectionReason::DeclaredButNotFoundFallback));
    }

    fn mk_sample(bytes: Option<Vec<u8>>) -> SampleEntry {
        SampleEntry {
            ts_ms: 1,
            topic: "t".into(),
            source_peer: None,
            format: SampleFormat::Protobuf,
            encoding: None,
            size_bytes: bytes.as_ref().map(|b| b.len()).unwrap_or(99_999),
            truncated: bytes.is_none(),
            preview: String::new(),
            raw_bytes: bytes,
            stale: false,
        }
    }

    #[test]
    fn sample_without_raw_bytes_produces_no_raw_bytes_result() {
        let s = mk_sample(None);
        assert!(s.raw_bytes.is_none());
    }
}
