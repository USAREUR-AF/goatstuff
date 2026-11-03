// SPDX-License-Identifier: Apache-2.0
//
// Payload generators per docs/design/trashgoat.md §"Payload spec":
//
//   { "kind": "bytes",          "size_bytes": N, "content": "random"|"zeros"|"repeating:<pattern>" }
//   { "kind": "chunked",        "total_size_bytes": N, "chunk_size_bytes": M, "content": ... }
//   { "kind": "json_shape",     "template": { ... } }
//   { "kind": "protobuf_shape", "message_type": "pkg.Msg", "fields": { ... } }
//
// Each `Payload::render` returns ONE OR MORE byte vectors plus the
// per-vector key suffix; the runner publishes each in turn on the
// per-spec topic.
//
// `chunked` emits N per-chunk publishes followed by a manifest publish
// (Pattern E from pubsub-best-practices §5), with key suffixes
// `/chunk/<index>` and `/manifest`.
//
// `protobuf_shape` is stubbed in v1 — the design doc notes this is a
// placeholder until the runner can reach into the panoscope decoder
// cache (or its own descriptor store) for FileDescriptorSet lookup.
// For now the generator returns the supplied `fields` as a JSON object
// with a header byte indicating "stub" so consumers see *something* and
// can verify the runner is reachable.

use crate::cadence::unwrap_string_payload;
use anyhow::{anyhow, Result};
use rand::{RngCore, SeedableRng};
use serde_json::json;

/// Parsed payload spec.
#[derive(Debug, Clone)]
pub enum Payload {
    Bytes {
        size_bytes: usize,
        content: ContentDirective,
    },
    Chunked {
        total_size_bytes: usize,
        chunk_size_bytes: usize,
        content: ContentDirective,
    },
    JsonShape {
        template: serde_json::Value,
    },
    ProtobufShape {
        message_type: String,
        fields: serde_json::Value,
    },
}

#[derive(Debug, Clone)]
pub enum ContentDirective {
    Random,
    Zeros,
    Repeating(Vec<u8>),
}

impl ContentDirective {
    fn parse(v: Option<&serde_json::Value>) -> Result<Self> {
        let s = match v {
            None => return Ok(ContentDirective::Random),
            Some(v) => v
                .as_str()
                .ok_or_else(|| anyhow!("payload_spec.content must be a string"))?,
        };
        if s == "random" {
            Ok(ContentDirective::Random)
        } else if s == "zeros" {
            Ok(ContentDirective::Zeros)
        } else if let Some(pat) = s.strip_prefix("repeating:") {
            if pat.is_empty() {
                Err(anyhow!(
                    "payload_spec.content repeating: needs a pattern after the colon"
                ))
            } else {
                Ok(ContentDirective::Repeating(pat.as_bytes().to_vec()))
            }
        } else {
            Err(anyhow!(
                "payload_spec.content unknown: {s:?} (expected random|zeros|repeating:<pattern>)"
            ))
        }
    }

    /// Fill `out` with content per the directive. `out.len()` bytes
    /// are written.
    fn fill(&self, out: &mut [u8]) {
        match self {
            ContentDirective::Random => {
                let mut rng = rand::rngs::StdRng::from_entropy();
                rng.fill_bytes(out);
            }
            ContentDirective::Zeros => {
                for b in out.iter_mut() {
                    *b = 0;
                }
            }
            ContentDirective::Repeating(pat) => {
                if pat.is_empty() {
                    return;
                }
                for (i, b) in out.iter_mut().enumerate() {
                    *b = pat[i % pat.len()];
                }
            }
        }
    }
}

impl Payload {
    pub fn parse(raw: &serde_json::Value) -> Result<Self> {
        let v = unwrap_string_payload(raw)?;
        let obj = v
            .as_object()
            .ok_or_else(|| anyhow!("payload_spec must be a JSON object, got {v:?}"))?;
        let kind = obj
            .get("kind")
            .and_then(|k| k.as_str())
            .ok_or_else(|| anyhow!("payload_spec.kind missing or not a string"))?;
        match kind {
            "bytes" => {
                let size_bytes = obj
                    .get("size_bytes")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| anyhow!("payload_spec.size_bytes missing or not an integer"))?
                    as usize;
                let content = ContentDirective::parse(obj.get("content"))?;
                Ok(Payload::Bytes {
                    size_bytes,
                    content,
                })
            }
            "chunked" => {
                let total_size_bytes = obj
                    .get("total_size_bytes")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| {
                        anyhow!("payload_spec.total_size_bytes missing or not an integer")
                    })? as usize;
                let chunk_size_bytes = obj
                    .get("chunk_size_bytes")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| {
                        anyhow!("payload_spec.chunk_size_bytes missing or not an integer")
                    })? as usize;
                if chunk_size_bytes == 0 {
                    return Err(anyhow!("payload_spec.chunk_size_bytes must be > 0"));
                }
                let content = ContentDirective::parse(obj.get("content"))?;
                Ok(Payload::Chunked {
                    total_size_bytes,
                    chunk_size_bytes,
                    content,
                })
            }
            "json_shape" => {
                let template = obj
                    .get("template")
                    .cloned()
                    .ok_or_else(|| anyhow!("payload_spec.template missing"))?;
                if !template.is_object() {
                    return Err(anyhow!("payload_spec.template must be an object"));
                }
                Ok(Payload::JsonShape { template })
            }
            "protobuf_shape" => {
                let message_type = obj
                    .get("message_type")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("payload_spec.message_type missing or not a string"))?
                    .to_string();
                let fields = obj
                    .get("fields")
                    .cloned()
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                Ok(Payload::ProtobufShape {
                    message_type,
                    fields,
                })
            }
            other => Err(anyhow!(
                "payload_spec.kind unknown: {other:?} (expected bytes|chunked|json_shape|protobuf_shape)"
            )),
        }
    }

    /// Generate the payloads for ONE cadence tick. Most payloads are
    /// single-publish; `chunked` is multi-publish (N chunks + 1
    /// manifest). The `Vec<(key_suffix, bytes)>` is suffixed onto the
    /// resolved topic key by the runner.
    pub fn render(&self, seq: u64) -> Vec<(String, Vec<u8>)> {
        match self {
            Payload::Bytes {
                size_bytes,
                content,
            } => {
                let mut buf = vec![0u8; *size_bytes];
                content.fill(&mut buf);
                vec![(String::new(), buf)]
            }
            Payload::Chunked {
                total_size_bytes,
                chunk_size_bytes,
                content,
            } => {
                let mut out = Vec::new();
                let mut remaining = *total_size_bytes;
                let mut idx: usize = 0;
                while remaining > 0 {
                    let take = remaining.min(*chunk_size_bytes);
                    let mut buf = vec![0u8; take];
                    content.fill(&mut buf);
                    out.push((format!("/chunk/{idx}"), buf));
                    remaining -= take;
                    idx += 1;
                }
                let manifest = json!({
                    "kind": "trashgoat-chunked-manifest",
                    "seq": seq,
                    "total_chunks": idx,
                    "total_size_bytes": total_size_bytes,
                    "chunk_size_bytes": chunk_size_bytes,
                });
                out.push((
                    "/manifest".to_string(),
                    serde_json::to_vec(&manifest).unwrap_or_default(),
                ));
                out
            }
            Payload::JsonShape { template } => {
                let body = substitute_json_template(template, seq);
                vec![(
                    String::new(),
                    serde_json::to_vec(&body).unwrap_or_default(),
                )]
            }
            Payload::ProtobufShape {
                message_type,
                fields,
            } => {
                // Stub per docs/design/trashgoat.md §"Phased rollout"
                // — protobuf encoding requires descriptor-set lookup
                // we don't have in v1. Return a JSON envelope flagging
                // the stub so a consumer can tell this is a placeholder
                // (no false impression that the bytes are valid wire
                // protobuf).
                let body = json!({
                    "kind": "trashgoat-protobuf-stub",
                    "message_type": message_type,
                    "fields": fields,
                    "seq": seq,
                    "note": "protobuf_shape stubbed — runner emits JSON envelope; see docs/design/trashgoat.md",
                });
                vec![(
                    String::new(),
                    serde_json::to_vec(&body).unwrap_or_default(),
                )]
            }
        }
    }
}

/// Substitute `$uuid` / `$now_ms` / `$seq` markers in a JSON template.
/// Walks every value; replaces string literals that exactly equal one of
/// the markers. Non-marker strings + non-string values are preserved
/// as-is. Matches the design doc §"Payload spec / json_shape" examples.
fn substitute_json_template(template: &serde_json::Value, seq: u64) -> serde_json::Value {
    fn walk(v: &serde_json::Value, seq: u64) -> serde_json::Value {
        match v {
            serde_json::Value::String(s) => match s.as_str() {
                "$uuid" => serde_json::Value::String(uuid::Uuid::new_v4().to_string()),
                "$now_ms" => serde_json::Value::from(now_ms()),
                "$seq" => serde_json::Value::from(seq),
                _ => serde_json::Value::String(s.clone()),
            },
            serde_json::Value::Array(arr) => {
                serde_json::Value::Array(arr.iter().map(|x| walk(x, seq)).collect())
            }
            serde_json::Value::Object(obj) => {
                let mapped: serde_json::Map<String, serde_json::Value> = obj
                    .iter()
                    .map(|(k, x)| (k.clone(), walk(x, seq)))
                    .collect();
                serde_json::Value::Object(mapped)
            }
            other => other.clone(),
        }
    }
    walk(template, seq)
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_bytes_random_default_content() {
        let p = Payload::parse(&json!({"kind": "bytes", "size_bytes": 16})).unwrap();
        match p {
            Payload::Bytes {
                size_bytes,
                content,
            } => {
                assert_eq!(size_bytes, 16);
                assert!(matches!(content, ContentDirective::Random));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_bytes_zeros() {
        let p =
            Payload::parse(&json!({"kind": "bytes", "size_bytes": 8, "content": "zeros"})).unwrap();
        match p {
            Payload::Bytes { content, .. } => assert!(matches!(content, ContentDirective::Zeros)),
            _ => panic!(),
        }
    }

    #[test]
    fn parse_bytes_repeating() {
        let p = Payload::parse(
            &json!({"kind": "bytes", "size_bytes": 6, "content": "repeating:abc"}),
        )
        .unwrap();
        let chunks = p.render(0);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].1, b"abcabc");
    }

    #[test]
    fn parse_chunked_emits_n_plus_manifest() {
        let p = Payload::parse(&json!({
            "kind": "chunked",
            "total_size_bytes": 10,
            "chunk_size_bytes": 4,
            "content": "zeros"
        }))
        .unwrap();
        let chunks = p.render(42);
        // 4+4+2 chunks + manifest = 4 publishes
        assert_eq!(chunks.len(), 4);
        assert_eq!(chunks[0].0, "/chunk/0");
        assert_eq!(chunks[0].1.len(), 4);
        assert_eq!(chunks[1].0, "/chunk/1");
        assert_eq!(chunks[1].1.len(), 4);
        assert_eq!(chunks[2].0, "/chunk/2");
        assert_eq!(chunks[2].1.len(), 2);
        assert_eq!(chunks[3].0, "/manifest");
        let manifest: serde_json::Value = serde_json::from_slice(&chunks[3].1).unwrap();
        assert_eq!(manifest["total_chunks"], 3);
        assert_eq!(manifest["seq"], 42);
    }

    #[test]
    fn parse_json_shape_template_substitution() {
        let p = Payload::parse(&json!({
            "kind": "json_shape",
            "template": {
                "id": "$uuid",
                "seq": "$seq",
                "literal": "x"
            }
        }))
        .unwrap();
        let chunks = p.render(7);
        let body: serde_json::Value = serde_json::from_slice(&chunks[0].1).unwrap();
        assert_eq!(body["seq"], 7);
        assert_eq!(body["literal"], "x");
        // uuid is a string and looks UUID-ish (36 chars with dashes).
        let id = body["id"].as_str().unwrap();
        assert_eq!(id.len(), 36);
        assert!(id.chars().filter(|c| *c == '-').count() == 4);
    }

    #[test]
    fn parse_protobuf_shape_stubbed_envelope() {
        let p = Payload::parse(&json!({
            "kind": "protobuf_shape",
            "message_type": "notional.tracks_demo.v1.Entity",
            "fields": {"confidence": 0.42}
        }))
        .unwrap();
        let chunks = p.render(0);
        let body: serde_json::Value = serde_json::from_slice(&chunks[0].1).unwrap();
        assert_eq!(body["kind"], "trashgoat-protobuf-stub");
        assert_eq!(body["message_type"], "notional.tracks_demo.v1.Entity");
    }

    #[test]
    fn parse_string_wrapped_payload() {
        // Portal emits payload_spec as a string-wrapped JSON literal.
        let wrapped = json!(r#"{"kind":"bytes","size_bytes":4,"content":"zeros"}"#);
        let p = Payload::parse(&wrapped).unwrap();
        let chunks = p.render(0);
        assert_eq!(chunks[0].1, vec![0u8; 4]);
    }

    #[test]
    fn parse_unknown_kind_errors() {
        let err = Payload::parse(&json!({"kind": "spaghetti"})).unwrap_err();
        assert!(err.to_string().contains("unknown"));
    }
}
