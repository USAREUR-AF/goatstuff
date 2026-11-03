// SPDX-License-Identifier: Apache-2.0
//
// Protobuf payload decode via `prost-reflect`. Mirrors the shape of
// `ops/panoscope/src/decoder.rs` (Block 82A) but stripped to a
// pure function — flashlight's decode is invoked on-demand from either
// the browser Tier 2 page (an operator clicking a row to see the
// "DECODED" tab) or the native `/api/compute` endpoint. There is no
// async caching here; the caller is responsible for shipping the
// FileDescriptorSet bytes (typically fetched via
// `panoscope_core::PortalClient::fetch_fds` on the server side, or via
// a browser-side `fetch()` and forwarded into WASM on the Tier 2 side).
//
// Per ADR 0516, a topic's `.fds` declares exactly one root message —
// `message_name` is optional; when omitted, the first message in the
// pool is used (matching panoscope's `parse_first_message` convention).

use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage};
use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::events::Event;

/// Per-decode failure mode. The variants are operator-actionable
/// (each maps to a distinct frontend rendering — see panoscope's
/// `SampleDecodeResult` / `TopicDecodeStatus` for the same taxonomy).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum DecodeError {
    /// `fds_bytes` failed to parse as a FileDescriptorSet. Usually means
    /// the portal returned non-FDS bytes (HTML error page, truncated
    /// download). Operator action: check the portal upload.
    InvalidFds(String),
    /// FDS parsed but contains no message types. Usually means a vendor
    /// uploaded a structurally empty `.fds`. Operator action: re-upload
    /// a valid descriptor set.
    EmptyDescriptor,
    /// `message_name` was Some(name) but no message with that fully-
    /// qualified name is in the pool. Operator action: check the
    /// format-tag on the topic declaration vs the actual FDS contents.
    MessageNotFound(String),
    /// Payload bytes didn't fit the descriptor. Per-sample error per
    /// panoscope's pattern; the rest of a stream's samples can still
    /// decode.
    PayloadDecodeFailed(String),
    /// `DynamicMessage` serialized but `serde_json` rejected the result.
    /// Rare; would only happen on a `prost-reflect` invariant break.
    SerializeFailed(String),
}

/// Decode a protobuf payload against a FileDescriptorSet.
///
/// `message_name` is the fully-qualified message name (e.g.
/// `flashlight.test.Observation`). When None, picks the first message
/// in the FDS per the ADR 0516 one-topic-one-message default — matches
/// `panoscope::decoder::parse_first_message`.
///
/// Returns the decoded message as a JSON value. The output shape comes
/// from `prost-reflect`'s `serde` feature: protobuf field names map to
/// JSON keys via the proto3 JSON mapping (camelCase by default, scalar
/// values directly, repeated fields as arrays).
pub fn decode_payload(
    payload: &[u8],
    fds_bytes: &[u8],
    message_name: Option<&str>,
) -> Result<JsonValue, DecodeError> {
    let pool = DescriptorPool::decode(fds_bytes)
        .map_err(|e| DecodeError::InvalidFds(e.to_string()))?;

    let descriptor = match message_name {
        Some(name) => pool
            // 1. Fully-qualified match (e.g. "notional.tracks_demo.v1.Entity").
            .get_message_by_name(name)
            // 2. Bare leaf-name walk over every message (top-level + nested).
            //    Topics declare a simple `protobuf:Entity`, but a real proto
            //    has a `package`, so the FDS registers the qualified
            //    `notional.tracks_demo.v1.Entity` and the FQ lookup misses.
            //    Mirror `panoscope::decoder::select_message` so the two
            //    surfaces decode identically. (panoscope's step 3 —
            //    fall back to the first message when the name is absent — is
            //    deliberately NOT taken here: flashlight's honesty bar prefers
            //    the labelled schema-less wire view over rendering a wrong
            //    message's field names, so a genuinely-absent name still
            //    errors below and the caller falls back to the wire view.)
            .or_else(|| pool.all_messages().find(|m| m.name() == name))
            .ok_or_else(|| DecodeError::MessageNotFound(name.to_string()))?,
        None => pool
            .all_messages()
            .next()
            .ok_or(DecodeError::EmptyDescriptor)?,
    };

    let dynamic = DynamicMessage::decode(descriptor, payload)
        .map_err(|e| DecodeError::PayloadDecodeFailed(e.to_string()))?;

    serde_json::to_value(&dynamic).map_err(|e| DecodeError::SerializeFailed(e.to_string()))
}

/// Convenience for the Engine path: pull `payload` + `format` off an
/// `Event` and decode. Format must be `protobuf:<MessageName>` per the
/// portal-side `topic.format` convention. Returns None when the event
/// isn't tagged as protobuf (silent skip — the caller renders the raw
/// view in that case).
pub fn decode_event(
    event: &Event,
    fds_bytes: &[u8],
) -> Option<Result<JsonValue, DecodeError>> {
    let format = event.format.as_deref()?;
    let message_name = format.strip_prefix("protobuf:")?;
    Some(decode_payload(&event.payload, fds_bytes, Some(message_name)))
}

// ---------------------------------------------------------------------------
// Schema-less structural decode (à la `protoc --decode_raw`).
//
// When the portal has no `.fds` for a topic, `decode_payload` can't run —
// today that leaves the operator staring at hex. This walks the protobuf
// wire format with no schema and renders a structural view keyed by field
// NUMBER (names are unknowable without a descriptor). It is deliberately
// strict: it returns `None` unless the ENTIRE buffer parses cleanly as
// protobuf, so arbitrary non-protobuf binary falls back to hex rather than
// being mis-rendered as plausible-but-fake fields. That strictness is the
// honesty bar (minimalism doctrine Principle 11): a structural view is
// offered only when the bytes really are protobuf, and it's always labelled
// as "no schema" so it's never confused with the real decoded message.
// ---------------------------------------------------------------------------

/// Maximum nesting depth for the recursive length-delimited heuristic —
/// a backstop against pathological / adversarial inputs.
const MAX_WIRE_DEPTH: usize = 12;

/// Best-effort schema-less decode of protobuf wire bytes. Returns a JSON
/// object keyed by field number (as strings; repeated fields become
/// arrays), or `None` if `payload` is empty or does not parse cleanly as
/// protobuf with no trailing bytes. Scalar interpretation without a schema
/// is necessarily lossy — varints render as integers, 32/64-bit fields as
/// hex, length-delimited as a nested message / UTF-8 string / hex in that
/// preference order.
pub fn decode_wire_structure(payload: &[u8]) -> Option<JsonValue> {
    if payload.is_empty() {
        return None;
    }
    parse_wire_message(payload, 0)
}

/// Parse a complete protobuf message, consuming all of `buf`. Returns
/// `None` on any malformed tag, length overrun, group wire-type (3/4,
/// deprecated + ambiguous), or trailing bytes.
fn parse_wire_message(buf: &[u8], depth: usize) -> Option<JsonValue> {
    if depth > MAX_WIRE_DEPTH {
        return None;
    }
    let mut map = serde_json::Map::new();
    let mut i = 0usize;
    while i < buf.len() {
        let (tag, n) = read_varint(&buf[i..])?;
        i += n;
        let field = tag >> 3;
        let wire = (tag & 0x7) as u8;
        if field == 0 {
            return None; // field number 0 is illegal
        }
        let value = match wire {
            0 => {
                let (v, n) = read_varint(&buf[i..])?;
                i += n;
                JsonValue::Number(v.into())
            }
            1 => {
                if i + 8 > buf.len() {
                    return None;
                }
                let v = hex_0x(&buf[i..i + 8]);
                i += 8;
                JsonValue::String(v)
            }
            2 => {
                let (len, n) = read_varint(&buf[i..])?;
                i += n;
                let len = len as usize;
                if i + len > buf.len() {
                    return None;
                }
                let v = length_delimited_value(&buf[i..i + len], depth);
                i += len;
                v
            }
            5 => {
                if i + 4 > buf.len() {
                    return None;
                }
                let v = hex_0x(&buf[i..i + 4]);
                i += 4;
                JsonValue::String(v)
            }
            _ => return None, // 3/4 (groups) or invalid wire types
        };
        insert_wire_field(&mut map, field, value);
    }
    Some(JsonValue::Object(map))
}

/// Render a length-delimited field: prefer a printable UTF-8 string, then
/// a cleanly-parsing nested message, then hex. (The string-first order
/// matches `protoc --decode_raw`'s behaviour and avoids treating obvious
/// text as a spurious nested message.)
fn length_delimited_value(sub: &[u8], depth: usize) -> JsonValue {
    if sub.is_empty() {
        return JsonValue::String(String::new());
    }
    if let Ok(s) = core::str::from_utf8(sub) {
        if s.chars()
            .all(|c| !c.is_control() || c == '\n' || c == '\t' || c == '\r')
        {
            return JsonValue::String(s.to_string());
        }
    }
    if let Some(msg) = parse_wire_message(sub, depth + 1) {
        return msg;
    }
    JsonValue::String(hex_0x(sub))
}

/// Insert a decoded field by number; on a repeat, fold into a JSON array
/// (protobuf allows repeated occurrences of the same field number).
fn insert_wire_field(map: &mut serde_json::Map<String, JsonValue>, field: u64, value: JsonValue) {
    let key = field.to_string();
    match map.get_mut(&key) {
        Some(JsonValue::Array(arr)) => arr.push(value),
        Some(existing) => {
            let prev = existing.take();
            *existing = JsonValue::Array(vec![prev, value]);
        }
        None => {
            map.insert(key, value);
        }
    }
}

/// Read a base-128 varint. Returns `(value, bytes_consumed)` or `None` on
/// truncation or an over-long (> 10 byte) encoding.
fn read_varint(buf: &[u8]) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    for (idx, &b) in buf.iter().enumerate() {
        if idx >= 10 {
            return None; // varint longer than a u64 can hold
        }
        result |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Some((result, idx + 1));
        }
        shift += 7;
    }
    None // ran off the end with the continuation bit still set
}

/// Lowercase `0x`-prefixed hex of `bytes` (for fixed32 / fixed64 fields,
/// whose signedness + float-ness is unknowable without a schema).
fn hex_0x(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push('0');
    out.push('x');
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

// ---------------------------------------------------------------------------
// Helpers shared by tests + future capability commits — kept here so the
// hand-built FileDescriptorSet pattern stays close to the decode it tests.
// ---------------------------------------------------------------------------

/// Hand-build a FileDescriptorSet matching the schema:
///
/// ```proto
/// syntax = "proto3";
/// package flashlight.test;
/// message Observation {
///   string sensor_id = 1;
///   double value     = 2;
///   int64  ts_ms     = 3;
/// }
/// ```
///
/// Returned as protobuf-encoded bytes — exactly what
/// `panoscope_core::PortalClient::fetch_fds` would return for a topic
/// whose schema has been uploaded.
///
/// Lives in non-test code so the same builder can seed fixtures for
/// future capability commits (schema-diff needs two FDS values to
/// diff; rolling-stats fixtures may want to anchor on a real schema
/// to feel realistic).
#[doc(hidden)]
pub fn build_observation_fds() -> Vec<u8> {
    use prost_types::{
        field_descriptor_proto::{Label, Type},
        DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    };

    let fds = FileDescriptorSet {
        file: vec![FileDescriptorProto {
            name: Some("observation.proto".to_string()),
            package: Some("flashlight.test".to_string()),
            syntax: Some("proto3".to_string()),
            message_type: vec![DescriptorProto {
                name: Some("Observation".to_string()),
                field: vec![
                    FieldDescriptorProto {
                        name: Some("sensor_id".to_string()),
                        number: Some(1),
                        label: Some(Label::Optional as i32),
                        r#type: Some(Type::String as i32),
                        json_name: Some("sensorId".to_string()),
                        ..Default::default()
                    },
                    FieldDescriptorProto {
                        name: Some("value".to_string()),
                        number: Some(2),
                        label: Some(Label::Optional as i32),
                        r#type: Some(Type::Double as i32),
                        json_name: Some("value".to_string()),
                        ..Default::default()
                    },
                    FieldDescriptorProto {
                        name: Some("ts_ms".to_string()),
                        number: Some(3),
                        label: Some(Label::Optional as i32),
                        r#type: Some(Type::Int64 as i32),
                        json_name: Some("tsMs".to_string()),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    fds.encode_to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost_reflect::Value as PrValue;

    fn encode_sample_observation(fds_bytes: &[u8]) -> Vec<u8> {
        let pool = DescriptorPool::decode(fds_bytes).expect("decode fds");
        let msg_desc = pool
            .get_message_by_name("flashlight.test.Observation")
            .expect("Observation present");
        let mut dyn_msg = DynamicMessage::new(msg_desc);
        dyn_msg.set_field_by_name("sensor_id", PrValue::String("alpha-fob".to_string()));
        dyn_msg.set_field_by_name("value", PrValue::F64(2.5));
        dyn_msg.set_field_by_name("ts_ms", PrValue::I64(1_716_750_000_000));
        dyn_msg.encode_to_vec()
    }

    #[test]
    fn decode_payload_observation_roundtrip() {
        let fds = build_observation_fds();
        let payload = encode_sample_observation(&fds);

        let json = decode_payload(&payload, &fds, Some("flashlight.test.Observation"))
            .expect("decode ok");
        assert_eq!(json["sensorId"], "alpha-fob");
        assert!((json["value"].as_f64().unwrap() - 2.5).abs() < 1e-9);
        assert_eq!(json["tsMs"], "1716750000000");
    }

    #[test]
    fn decode_payload_bare_name_resolves_packaged_message() {
        // The reported flashlight bug: a topic declares `protobuf:Entity` (a
        // bare leaf name), but the FDS — built from a `.proto` with a
        // `package` — registers the qualified `<pkg>.Entity`, so the
        // fully-qualified lookup misses. The tier-2 leaf-name fallback must
        // resolve it (decode parity with panoscope::decoder::select_message),
        // rather than dropping to the schema-less numbered wire view.
        let fds = build_observation_fds(); // packaged as flashlight.test.Observation
        let payload = encode_sample_observation(&fds);
        let json = decode_payload(&payload, &fds, Some("Observation"))
            .expect("bare leaf name resolves the packaged message");
        assert_eq!(json["sensorId"], "alpha-fob");
        assert_eq!(json["tsMs"], "1716750000000");
    }

    #[test]
    fn decode_payload_first_message_when_name_omitted() {
        let fds = build_observation_fds();
        let payload = encode_sample_observation(&fds);
        // Same FDS has exactly one message; passing None should resolve
        // to it per the ADR 0516 first-message convention.
        let json = decode_payload(&payload, &fds, None).expect("decode ok");
        assert_eq!(json["sensorId"], "alpha-fob");
    }

    #[test]
    fn decode_payload_unknown_message_name_errors() {
        let fds = build_observation_fds();
        let err = decode_payload(&[], &fds, Some("flashlight.test.Missing")).unwrap_err();
        assert!(matches!(err, DecodeError::MessageNotFound(ref n) if n == "flashlight.test.Missing"));
    }

    #[test]
    fn decode_payload_invalid_fds_errors() {
        let err = decode_payload(&[], &[0xff, 0xff, 0xff, 0xff, 0xff], None).unwrap_err();
        assert!(matches!(err, DecodeError::InvalidFds(_)));
    }

    #[test]
    fn decode_payload_empty_fds_errors() {
        let err = decode_payload(&[], &[], None).unwrap_err();
        assert!(matches!(err, DecodeError::EmptyDescriptor));
    }

    #[test]
    fn decode_event_skips_non_protobuf_events() {
        let fds = build_observation_fds();
        let event = Event {
            key: "t".into(),
            ts_ms: 0,
            payload: vec![],
            format: Some("json".into()),
        };
        assert!(decode_event(&event, &fds).is_none());
    }

    #[test]
    fn wire_structure_decodes_observation_by_field_number() {
        // Real protobuf bytes, but pretend we have no schema. The
        // structural view should surface the three fields by number.
        let fds = build_observation_fds();
        let payload = encode_sample_observation(&fds);
        let v = decode_wire_structure(&payload).expect("parses as protobuf");
        // field 1 = sensor_id (string, length-delimited → printable UTF-8)
        assert_eq!(v["1"], "alpha-fob");
        // field 2 = value (double, 64-bit → hex)
        assert!(v["2"].as_str().unwrap().starts_with("0x"));
        // field 3 = ts_ms (int64, varint → integer)
        assert_eq!(v["3"], 1_716_750_000_000u64);
    }

    #[test]
    fn wire_structure_handles_nested_messages() {
        // Outer { 1: { 1: 7 } } — a nested message inside field 1.
        let inner = [0x08, 0x07]; // field 1, varint 7
        let mut outer = vec![0x0a, inner.len() as u8]; // field 1, len-delimited
        outer.extend_from_slice(&inner);
        let v = decode_wire_structure(&outer).expect("nested parses");
        assert_eq!(v["1"]["1"], 7u64);
    }

    #[test]
    fn wire_structure_folds_repeated_fields_into_array() {
        // field 1 (varint) appears twice: 1 then 2 → JSON array [1, 2].
        let bytes = [0x08, 0x01, 0x08, 0x02];
        let v = decode_wire_structure(&bytes).expect("parses");
        assert_eq!(v["1"], serde_json::json!([1, 2]));
    }

    #[test]
    fn wire_structure_rejects_non_protobuf_bytes() {
        // JSON object bytes: '{' = 0x7b → field 15, wire type 3 (group),
        // which we reject. Must be None, not a fake structure.
        assert!(decode_wire_structure(b"{\"a\":1}").is_none());
        // Truncated length-delimited (claims 9 bytes, only 1 follows).
        assert!(decode_wire_structure(&[0x0a, 0x09, 0x00]).is_none());
        // Empty payload.
        assert!(decode_wire_structure(b"").is_none());
    }

    #[test]
    fn decode_event_strips_protobuf_prefix() {
        let fds = build_observation_fds();
        let payload = encode_sample_observation(&fds);
        let event = Event {
            key: "vendor-a/observations/v1".into(),
            ts_ms: 0,
            payload,
            format: Some("protobuf:flashlight.test.Observation".into()),
        };
        let json = decode_event(&event, &fds).expect("protobuf event").expect("decode ok");
        assert_eq!(json["sensorId"], "alpha-fob");
    }
}
