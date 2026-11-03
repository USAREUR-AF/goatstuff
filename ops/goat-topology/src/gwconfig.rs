// SPDX-License-Identifier: Apache-2.0
//! Reads the DECLARED `gateway` block out of a router's config file.
//!
//! # Why the file and not the adminspace
//!
//! The adminspace does not expose the gateway config at all: `@/local/**`
//! returns no config keys. So the declared mode has to come from
//! `/etc/<stack>/config.json5` on the router host, which the publisher sits on.
//!
//! # Why only the `gateway` block
//!
//! That file is ~900 KB and is overwhelmingly `access_control` — the goatnet's
//! live ACL, including slot anchors and subject CNs. **None of it is read into
//! the envelope, logged, or published.** A meter that shipped ACL content onto
//! the bus would be an identity leak dressed as observability. This module
//! finds one top-level key by brace-matching and parses only that fragment;
//! everything else in the file is skipped without being interpreted.
//!
//! # Why the mode is never inferred from observed regions
//!
//! `Preset(Auto)` and a `Custom` list whose filters match nothing produce **the
//! same observable regions**, and the second is a real misconfiguration. A
//! reader that guessed from observation would report a fault as healthy. When
//! this module cannot read the file it says `Unreadable`, never `Auto`.

use anyhow::{Context, Result};

use crate::wire::{GatewayFilterV1, GatewayMode, GatewaySubregionV1};

pub struct Declared {
    pub mode: GatewayMode,
    pub subregions: Vec<GatewaySubregionV1>,
    /// Operator-facing reason the mode is `Unreadable`. Never parsed.
    pub error: String,
}

impl Declared {
    fn unreadable(why: impl Into<String>) -> Self {
        Self {
            mode: GatewayMode::Unreadable,
            subregions: vec![],
            error: why.into(),
        }
    }
    fn auto() -> Self {
        Self {
            mode: GatewayMode::Auto,
            subregions: vec![],
            error: String::new(),
        }
    }
}

/// Read `path` and extract the declared gateway posture.
///
/// A missing file is `Unreadable`, not `Auto`: we were told to look at a
/// router's config and could not, which is a different fact from "the config
/// has no gateway key".
pub fn read(path: &str) -> Declared {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return Declared::unreadable(format!("{path}: {e}")),
    };
    match parse(&text) {
        Ok(d) => d,
        Err(e) => Declared::unreadable(format!("{path}: {e}")),
    }
}

/// Extract and interpret the `gateway` block from config text.
pub fn parse(text: &str) -> Result<Declared> {
    let Some(fragment) = extract_block(text, "gateway") else {
        // The key is genuinely absent, which IS zenoh's Auto default
        // (`config.gateway.south.clone().unwrap_or_default()`,
        // zenoh-1.9.0 net/runtime/region.rs:122). This is the usual case for a goat
        // router.
        return Ok(Declared::auto());
    };

    let v: serde_json::Value =
        json5::from_str(&fragment).context("gateway block is not parseable JSON5")?;

    let Some(south) = v.get("south") else {
        // `gateway: {}` with no `south` — the field is Option and defaults the
        // same way an absent key does.
        return Ok(Declared::auto());
    };

    // `GatewaySouthConf` is serde-untagged: a string is the Preset, an array is
    // the Custom list (zenoh-config-1.9.0/src/gateway.rs:29-32).
    if let Some(preset) = south.as_str() {
        return if preset.eq_ignore_ascii_case("auto") {
            Ok(Declared::auto())
        } else {
            Ok(Declared::unreadable(format!(
                "unknown gateway.south preset {preset:?}"
            )))
        };
    }

    let Some(list) = south.as_array() else {
        return Ok(Declared::unreadable(
            "gateway.south is neither a preset string nor a list",
        ));
    };

    let mut subregions = Vec::with_capacity(list.len());
    for (index, entry) in list.iter().enumerate() {
        // Absent `filters` matches EVERY peer; an empty list matches NONE.
        // The distinction is the whole of finding T-002, so it is carried
        // explicitly rather than collapsed into an empty vec.
        let filters_value = entry.get("filters");
        let filters_present = !matches!(filters_value, None | Some(serde_json::Value::Null));
        let filters = filters_value
            .and_then(|f| f.as_array())
            .map(|fs| fs.iter().map(read_filter).collect())
            .unwrap_or_default();
        subregions.push(GatewaySubregionV1 {
            index: index as u32,
            filters_present,
            filters,
        });
    }

    Ok(Declared {
        mode: GatewayMode::Custom,
        subregions,
        error: String::new(),
    })
}

/// Record WHICH axes a filter uses. The values are deliberately not carried —
/// the surface needs to know `zids`/`region_names` are in play (T-001), not
/// what they match.
fn read_filter(f: &serde_json::Value) -> GatewayFilterV1 {
    let has = |k: &str| !matches!(f.get(k), None | Some(serde_json::Value::Null));
    GatewayFilterV1 {
        uses_modes: has("modes"),
        uses_interfaces: has("interfaces"),
        uses_zids: has("zids"),
        uses_region_names: has("region_names"),
        negated: f.get("negated").and_then(|v| v.as_bool()).unwrap_or(false),
    }
}

/// Find `<key>:` at the top level of a JSON5 object and return `{...}` for it.
///
/// Brace-matched rather than fully parsed so a 900 KB ACL is never
/// materialized. String and comment awareness matter: the ACL is full of
/// key-expressions containing braces, and a naive counter would run off the end
/// of the file and return nonsense.
fn extract_block(text: &str, key: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut depth = 0i32;
    let mut in_str: Option<u8> = None;
    let mut line_comment = false;
    let mut block_comment = false;

    while i < bytes.len() {
        let c = bytes[i];

        if line_comment {
            if c == b'\n' {
                line_comment = false;
            }
            i += 1;
            continue;
        }
        if block_comment {
            if c == b'*' && bytes.get(i + 1) == Some(&b'/') {
                block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if let Some(q) = in_str {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' | b'\'' => {
                in_str = Some(c);
                i += 1;
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                line_comment = true;
                i += 2;
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                block_comment = true;
                i += 2;
                continue;
            }
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }

        // Only match the key at object depth 1 — the top level of the config.
        // A `gateway` key nested inside some other block is not this one.
        if depth == 1 && c == key.as_bytes()[0] && text[i..].starts_with(key) {
            let after = i + key.len();
            let rest = text[after..].trim_start();
            let before_ok = i == 0
                || matches!(
                    bytes[i - 1],
                    b'{' | b',' | b'\n' | b'\r' | b' ' | b'\t' | b'"' | b'\''
                );
            if before_ok && rest.starts_with(':') {
                let val_rel = text[after..].find(':')? + 1;
                let val_start = after + val_rel + text[after + val_rel..].len()
                    - text[after + val_rel..].trim_start().len();
                if bytes.get(val_start) == Some(&b'{') {
                    let end = match_braces(bytes, val_start)?;
                    return Some(text[val_start..=end].to_string());
                }
                return None;
            }
        }
        i += 1;
    }
    None
}

/// Walk from an opening `{` to its partner, skipping strings and comments.
fn match_braces(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = start;
    let mut in_str: Option<u8> = None;
    let mut line_comment = false;
    let mut block_comment = false;
    while i < bytes.len() {
        let c = bytes[i];
        if line_comment {
            if c == b'\n' {
                line_comment = false;
            }
        } else if block_comment {
            if c == b'*' && bytes.get(i + 1) == Some(&b'/') {
                block_comment = false;
                i += 2;
                continue;
            }
        } else if let Some(q) = in_str {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                in_str = None;
            }
        } else {
            match c {
                b'"' | b'\'' => in_str = Some(c),
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    line_comment = true;
                    i += 2;
                    continue;
                }
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    block_comment = true;
                    i += 2;
                    continue;
                }
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The usual deployed shape: no `gateway` key,
    /// a large `access_control` block, and comments mentioning the word
    /// "gateway" that must NOT be mistaken for the key.
    const REAL_SHAPE: &str = r#"{
  mode: "router",
  // (+ the ingress-gateway quarantine-prefix rule when it lands).
  listen: { endpoints: ["tcp/0.0.0.0:7447"] },
  access_control: {
    enabled: true,
    rules: [ { id: "allow-x", key_exprs: ["a/**"] } ]
  },
}"#;

    #[test]
    fn no_gateway_key_is_auto_not_unreadable() {
        let d = parse(REAL_SHAPE).unwrap();
        assert_eq!(d.mode, GatewayMode::Auto);
        assert!(d.subregions.is_empty());
    }

    #[test]
    fn a_comment_mentioning_gateway_is_not_the_key() {
        // The only "gateway" hit here is inside a comment. Reading it
        // as the key would report a fabricated posture for the whole fleet.
        assert!(extract_block(REAL_SHAPE, "gateway").is_none());
    }

    #[test]
    fn preset_auto_string_is_auto() {
        let d = parse(r#"{ gateway: { south: "auto" } }"#).unwrap();
        assert_eq!(d.mode, GatewayMode::Auto);
    }

    #[test]
    fn custom_list_records_index_and_axes() {
        // A test arm: one subregion matching two routers by ZID.
        let d = parse(
            r#"{
              mode: "router",
              gateway: {
                south: [
                  { filters: [ { zids: ["b1b1b1b1", "b2b2b2b2"] } ] },
                  { filters: [ { modes: "peer", negated: true } ] },
                ],
              },
            }"#,
        )
        .unwrap();
        assert_eq!(d.mode, GatewayMode::Custom);
        assert_eq!(d.subregions.len(), 2);

        let first = &d.subregions[0];
        assert_eq!(first.index, 0, "the index IS the region id");
        assert!(first.filters[0].uses_zids);
        assert!(
            first.filters[0].uses_peer_asserted_axis(),
            "this must raise T-001"
        );
        assert!(!first.is_inert());

        let second = &d.subregions[1];
        assert_eq!(second.index, 1);
        assert!(second.filters[0].uses_modes);
        assert!(second.filters[0].negated);
        assert!(!second.filters[0].uses_peer_asserted_axis());
    }

    #[test]
    fn an_empty_filter_list_is_kept_distinct_from_an_absent_one() {
        let empty = parse(r#"{ gateway: { south: [ { filters: [] } ] } }"#).unwrap();
        assert!(
            empty.subregions[0].is_inert(),
            "empty list matches nothing — T-002"
        );

        let absent = parse(r#"{ gateway: { south: [ { } ] } }"#).unwrap();
        assert!(
            !absent.subregions[0].is_inert(),
            "absent filters matches everything"
        );
    }

    #[test]
    fn braces_inside_acl_strings_do_not_derail_the_scan() {
        // Key-expressions and comments in the ACL contain braces. A naive
        // counter runs off the end and returns garbage.
        let cfg = r#"{
  access_control: { rules: [ { key_exprs: ["a/{b}/**", "c/**"] } ] },
  /* a block comment with { an unbalanced brace */
  gateway: { south: [ { filters: [ { region_names: ["r1"] } ] } ] },
}"#;
        let d = parse(cfg).unwrap();
        assert_eq!(d.mode, GatewayMode::Custom);
        assert!(d.subregions[0].filters[0].uses_region_names);
    }

    #[test]
    fn a_missing_file_is_unreadable_never_auto() {
        let d = read("/nonexistent/config.json5");
        assert_eq!(d.mode, GatewayMode::Unreadable);
        assert!(
            !d.error.is_empty(),
            "an unreadable mode must carry its reason"
        );
    }
}
