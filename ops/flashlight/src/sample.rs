// SPDX-License-Identifier: Apache-2.0
//
// GET /api/sample/{seq} — on-demand payload retrieval for the Tier 1
// "click a row → inspect the raw payload" surface
// (docs/design/flashlight.md §"Tier 1 surfaces"). The live SSE tail
// carries no bytes; the browser fetches them here only when the
// operator clicks. Reads from the bounded `SampleStore`; a click on an
// evicted sample returns 404 so the UI can say "sample expired" rather
// than spin.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use panoscope_core::portal::FetchOutcome;
use serde::Serialize;

use crate::store::{SampleFormat, StoredSample};
use crate::AppState;

/// JSON view returned for a retained sample. `hex` is always present
/// (the faithful byte view); `text` is the UTF-8-lossy decode for texty
/// formats; `decoded` is the protobuf-deserialized JSON when the topic
/// has a schema in the portal — the UI defaults to whichever richest
/// view is available (decoded → text → hex).
#[derive(Debug, Serialize)]
pub struct SampleView {
    pub seq: u64,
    pub key: String,
    pub ts_ms: u64,
    pub format: SampleFormat,
    /// True wire length of the payload.
    pub payload_len: usize,
    /// Bytes actually retained in the preview (≤ payload_len).
    pub preview_len: usize,
    /// True when the preview is shorter than the wire payload.
    pub truncated: bool,
    /// Space-free lowercase hex of the retained preview bytes.
    pub hex: String,
    /// UTF-8-lossy decode of the preview — only for texty formats.
    pub text: Option<String>,
    /// Deserialized payload (pretty JSON). Either the schema-based
    /// protobuf decode (topic declares `protobuf:<Name>` + the portal has
    /// the descriptor) OR — when no schema is available — a best-effort
    /// schema-less structural view of the protobuf wire bytes.
    pub decoded: Option<String>,
    /// Label for the decoded pane, distinguishing a true schema decode
    /// ("decoded") from the schema-less structural fallback
    /// ("protobuf structure · no schema"). `None` when `decoded` is absent.
    pub decoded_label: Option<String>,
    /// Operator-facing note on the decode attempt (why decoded is
    /// present/absent): "decoded" / "no schema in portal" /
    /// "payload truncated — not decoded" / "not a protobuf topic" / etc.
    pub decode_note: String,
}

impl SampleView {
    /// Raw view (hex + text); `decoded` left empty for the handler to
    /// fill after its async portal/descriptor lookup.
    fn raw(s: &StoredSample) -> Self {
        let text = if s.format.is_texty() {
            Some(String::from_utf8_lossy(&s.preview).into_owned())
        } else {
            None
        };
        SampleView {
            seq: s.seq,
            key: s.key.clone(),
            ts_ms: s.ts_ms,
            format: s.format,
            payload_len: s.payload_len,
            preview_len: s.preview.len(),
            truncated: s.truncated(),
            hex: to_hex(&s.preview),
            text,
            decoded: None,
            decoded_label: None,
            decode_note: String::new(),
        }
    }
}

/// `GET /api/sample/{seq}` — the retained payload preview plus, when the
/// topic has a schema, the protobuf-decoded JSON. 404 when the sample
/// evicted out of the ring.
pub async fn sample_handler(
    State(state): State<AppState>,
    Path(seq): Path<u64>,
) -> axum::response::Response {
    let Some(s) = state.store.get(seq) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "sample expired or never existed — the live tail retains a bounded ring",
                "seq": seq,
            })),
        )
            .into_response();
    };
    let (decoded, decoded_label, decode_note) = try_decode(&state, &s).await;
    let mut view = SampleView::raw(&s);
    view.decoded = decoded;
    view.decoded_label = decoded_label;
    view.decode_note = decode_note;
    (StatusCode::OK, Json(view)).into_response()
}

/// Memoized decode context for a key: the declared wire format and the
/// `.fds` descriptor bytes. Both resolved once per key (first decode) and
/// cached so auto-follow refreshes don't re-hit the portal. `None` fields
/// are definitive negatives (no portal topic row / no descriptor).
#[derive(Debug, Clone, Default)]
pub struct DecodeCtx {
    /// Declared format, e.g. `protobuf:notional.tracks_demo.v1.Entity`.
    pub format: Option<String>,
    /// FDS descriptor bytes for the topic.
    pub fds: Option<Vec<u8>>,
}

/// Attempt to deserialize the sample. Returns `(decoded_json,
/// decoded_label, note)`. Prefers a true schema-based protobuf decode —
/// full (non-truncated) payload, topic declares `protobuf:<Name>`, portal
/// has the `.fds`. When no schema is available (or the schema decode
/// fails), falls back to a schema-less structural view of the protobuf
/// wire bytes so the operator sees the message shape instead of only hex.
/// Both `format` + `fds` are memoized per key in the descriptor cache.
async fn try_decode(state: &AppState, s: &StoredSample) -> (Option<String>, Option<String>, String) {
    if s.truncated() {
        return (
            None,
            None,
            "payload larger than the preview cap — not decoded".into(),
        );
    }
    let ctx = decode_ctx_for_key(state, &s.key, s.encoding.as_deref()).await;

    // Schema-based decode: declared protobuf format + portal descriptor.
    if let Some(declared) = ctx.format.as_deref() {
        match declared.strip_prefix("protobuf:").filter(|n| !n.is_empty()) {
            None => {
                // Declared, but not protobuf. Texty payloads use the text
                // view; binary non-protobuf gets no structural guess.
                return (None, None, format!("topic format is `{declared}`, not protobuf"));
            }
            Some(message_name) => match ctx.fds.as_deref() {
                Some(fds) => {
                    match flashlight_analytics::decode::decode_payload(
                        &s.preview,
                        fds,
                        Some(message_name),
                    ) {
                        Ok(json) => {
                            return (
                                Some(pretty(&json)),
                                Some("decoded".into()),
                                "decoded".into(),
                            );
                        }
                        Err(e) => {
                            return schemaless_fallback(s, format!("schema decode failed: {e:?}"));
                        }
                    }
                }
                None => {
                    return schemaless_fallback(
                        s,
                        "no .fds descriptor uploaded to the portal".into(),
                    );
                }
            },
        }
    }

    schemaless_fallback(s, "topic not in the portal registry — no declared schema".into())
}

/// No schema is available (or the schema decode failed). For a binary
/// payload, try the schema-less structural protobuf view; otherwise return
/// just the `cause` note (hex/text panes still render). The structural
/// view is always labelled "no schema" so it's never confused with a real
/// decode, and only appears when the bytes genuinely parse as protobuf.
fn schemaless_fallback(s: &StoredSample, cause: String) -> (Option<String>, Option<String>, String) {
    if matches!(s.format, SampleFormat::Binary) {
        if let Some(v) = flashlight_analytics::decode::decode_wire_structure(&s.preview) {
            return (
                Some(pretty(&v)),
                Some("protobuf structure · no schema".into()),
                format!("{cause} — showing protobuf wire structure (field numbers, not names)"),
            );
        }
    }
    (None, None, cause)
}

/// Pretty-print a JSON value, falling back to its compact form.
fn pretty(json: &serde_json::Value) -> String {
    serde_json::to_string_pretty(json).unwrap_or_else(|_| json.to_string())
}

/// Resolve + memoize the decode context for `key`. The declared format
/// comes from the **portal topic registry** (`find_topic_by_key`, the
/// authoritative schema source — synth-vendor schema-decls carry
/// role/site but not `format`), falling back to a schema-decl
/// `format_hint` if the portal has no row. The descriptor comes from
/// `fetch_fds`. Transient portal errors are not cached so they retry.
async fn decode_ctx_for_key(state: &AppState, key: &str, wire_encoding: Option<&str>) -> DecodeCtx {
    if let Some(hit) = state
        .descriptor_cache
        .lock()
        .ok()
        .and_then(|c| c.get(key).cloned())
    {
        return hit;
    }

    // Declared format priority:
    //   1. portal topic registry — the schema SoT (also where the .fds lives)
    //   2. the sample's self-describing wire `Encoding` — the message name the
    //      publisher actually stamped (docs/design/zenoh-capability-adoption.md
    //      gap 1). Closes the "topic not in registry" / portal-down gap: the
    //      wire still says which protobuf message it is.
    //   3. schema-decl `format_hint` from the topology.
    let mut transient = false;
    let wire_format = wire_encoding.and_then(protobuf_format_from_encoding);
    let format = match state.portal.find_topic_by_key(key).await {
        Ok(Some(t)) if !t.format.is_empty() => Some(t.format),
        Ok(_) => wire_format
            .clone()
            .or_else(|| topology_format_hint(state, key)),
        Err(_) => {
            transient = true;
            wire_format.or_else(|| topology_format_hint(state, key))
        }
    };

    // Descriptor bytes.
    let fds = match state.portal.fetch_fds(key).await {
        FetchOutcome::Ready(b) => Some(b),
        FetchOutcome::NoSchema | FetchOutcome::Disabled => None,
        FetchOutcome::Transient(_) => {
            transient = true;
            None
        }
    };

    let ctx = DecodeCtx { format, fds };
    if !transient {
        if let Ok(mut c) = state.descriptor_cache.lock() {
            c.insert(key.to_string(), ctx.clone());
        }
    }
    ctx
}

/// Schema-decl `format_hint` for `key`'s publisher (topology fallback).
fn topology_format_hint(state: &AppState, key: &str) -> Option<String> {
    let topo = state.topology.read().ok()?;
    topo.peers
        .values()
        .find(|p| crate::keymatch::key_matches_any(key, &p.publishes))
        .and_then(|p| p.format_hint.clone())
}

/// Turn a self-describing wire `Encoding` into the `protobuf:<Name>`
/// declared-format shape the decoder expects, or `None` when it isn't a
/// schema-bearing protobuf encoding. `application/protobuf;TrackUpdate`
/// → `protobuf:TrackUpdate`; a bare `application/protobuf` (no schema
/// name) → `None` since there's no message to select.
fn protobuf_format_from_encoding(encoding: &str) -> Option<String> {
    let (media, schema) = encoding.split_once(';')?;
    if media.trim() != "application/protobuf" {
        return None;
    }
    let name = schema.trim();
    if name.is_empty() {
        None
    } else {
        Some(format!("protobuf:{name}"))
    }
}

/// Lowercase hex without separators. Avoids pulling in a hex/base64
/// crate for a ≤4 KiB preview — keeps the dep set lean per the
/// minimalism doctrine.
fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SampleStore;

    #[test]
    fn to_hex_encodes_bytes() {
        assert_eq!(to_hex(&[0x00, 0x0f, 0xff, 0x7b]), "000fff7b");
        assert_eq!(to_hex(b""), "");
    }

    #[test]
    fn protobuf_format_from_encoding_extracts_schema() {
        assert_eq!(
            protobuf_format_from_encoding("application/protobuf;TrackUpdate"),
            Some("protobuf:TrackUpdate".to_string())
        );
        // whitespace around the schema name is tolerated
        assert_eq!(
            protobuf_format_from_encoding("application/protobuf; notional.tracks.v1.Entity "),
            Some("protobuf:notional.tracks.v1.Entity".to_string())
        );
        // bare protobuf (no message name) → nothing to select
        assert_eq!(protobuf_format_from_encoding("application/protobuf"), None);
        // non-protobuf media types are not our concern here
        assert_eq!(protobuf_format_from_encoding("application/json"), None);
        assert_eq!(protobuf_format_from_encoding("application/cbor;Foo"), None);
    }

    #[test]
    fn view_for_json_carries_text_and_hex() {
        let store = SampleStore::new(8, 4096);
        let (seq, _) = store.ingest("vendor-a/obs/v1".into(), 100, b"{\"a\":1}", None);
        let view = SampleView::raw(&store.get(seq).unwrap());
        assert_eq!(view.format, SampleFormat::JsonObj);
        assert_eq!(view.text.as_deref(), Some("{\"a\":1}"));
        assert_eq!(view.hex, "7b226122 3a317d".replace(' ', ""));
        assert!(!view.truncated);
        assert_eq!(view.payload_len, 7);
        assert_eq!(view.preview_len, 7);
    }

    #[test]
    fn view_for_binary_has_no_text() {
        let store = SampleStore::new(8, 4096);
        let (seq, _) = store.ingest("k".into(), 0, &[0x08, 0x96, 0x01, 0xff], None);
        let view = SampleView::raw(&store.get(seq).unwrap());
        assert_eq!(view.format, SampleFormat::Binary);
        assert!(view.text.is_none());
        assert_eq!(view.hex, "089601ff");
    }

    #[test]
    fn view_reports_truncation() {
        let store = SampleStore::new(8, 4);
        let (seq, _) = store.ingest("k".into(), 0, b"0123456789", None);
        let view = SampleView::raw(&store.get(seq).unwrap());
        assert!(view.truncated);
        assert_eq!(view.payload_len, 10);
        assert_eq!(view.preview_len, 4);
        assert_eq!(view.hex, "30313233");
    }
}
