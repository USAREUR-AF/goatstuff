// SPDX-License-Identifier: Apache-2.0
//
// URL filter grammar — declares the focused slice. Per
// docs/design/flashlight.md §"URL filter grammar":
//
//   ?key=<zenoh-key-expr>      — subscribe to exact key (supports wildcards)
//   ?vendor=<short-tag>        — match <vendor>/** AND <anchor>/<vendor>/**
//   ?role=<role-class>         — subscribe to peers with role_class=<x>
//   ?site=<short-name>         — subscribe to peers tagged with site
//   ?since=<duration>          — replay window from this far back
//   ?until=<duration|now>      — end of replay window
//   ?format=<wire-format>      — filter to topics with this format
//   ?compute=<browser|server>  — force compute placement
//
// Multiple parameters AND together (?vendor=a&role=sensor = both).
// Repeated parameters OR within (?vendor=a&vendor=b = either).
//
// This module is pure-Rust and Zenoh-agnostic: it parses query strings
// into a typed `Filter` struct. The downstream layer (PR #2 in the
// Phase C series) translates the Filter into the narrowest Zenoh key
// expression we can — empty Filter is the help page, not "subscribe to
// everything", per the design doc.

use std::collections::HashSet;
use std::time::Duration;

use serde::Serialize;
use thiserror::Error;

/// Parsed URL filter. Each field is a `HashSet<String>` (or equivalent)
/// because repeated query parameters OR within the field. An empty
/// `Filter` resolves to the help page server-side.
#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct Filter {
    /// Literal Zenoh key expressions (can carry `*`/`**` wildcards).
    pub keys: HashSet<String>,
    /// Vendor short_tag values — matched anchor-aware at sample time:
    /// `<vendor>/**` on per-vendor goatnets, `<slot-anchor>/<vendor>/**`
    /// on slot-pool goatnets.
    pub vendors: HashSet<String>,
    /// Role class values — resolve via peer registry's `role_class`.
    pub roles: HashSet<String>,
    /// Site short_name values — resolve via peer registry's `site`.
    pub sites: HashSet<String>,
    /// Wire-format filter — e.g. "protobuf", "json", "cbor".
    pub formats: HashSet<String>,
    /// Replay window: include events from this far back. None = live-only.
    /// Capped at 1 hour by the parser (design doc constraint).
    pub since: Option<Duration>,
    /// End of replay window. None = live (open-ended). "now" → Some(ZERO).
    pub until: Option<Duration>,
    /// Operator override for compute placement (browser = Tier 2 WASM;
    /// server = /api/compute native endpoint). v1 Tier 1 ignores this
    /// field structurally — PR #5 of Phase D wires Tier 2 boot.
    pub compute: Option<ComputePlacement>,
}

impl Filter {
    /// True when no slice was declared. The server SHOULD render the
    /// help page (with examples + grammar) for this case — never
    /// "subscribe to everything", per design doc.
    ///
    /// PR #4 of Phase C wires the actual help-page redirect that
    /// consults this method; PR #1 just exposes it for tests.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
            && self.vendors.is_empty()
            && self.roles.is_empty()
            && self.sites.is_empty()
            && self.formats.is_empty()
            && self.since.is_none()
            && self.until.is_none()
            && self.compute.is_none()
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ComputePlacement {
    Browser,
    Server,
}

/// Parser-side errors. Returned to the client as a structured 400 with
/// a `minimal example URL` body per design doc; consumers stringify via
/// `Display`.
#[derive(Debug, Error)]
pub enum FilterError {
    #[error("malformed query string: {0}")]
    MalformedQuery(String),
    #[error("invalid duration `{value}` for `{key}` — accepted forms: `10s`, `5m`, `1h`, `now` (for `until`)")]
    BadDuration { key: String, value: String },
    #[error("`since` exceeds the 1h hard cap (got {got_secs}s)")]
    SinceTooLarge { got_secs: u64 },
    #[error("`compute` must be `browser` or `server` (got `{0}`)")]
    BadCompute(String),
    #[error("unknown query parameter `{0}` — accepted: key, vendor, role, site, since, until, format, compute")]
    UnknownParam(String),
}

/// Maximum `since` window per design doc.
pub const MAX_SINCE: Duration = Duration::from_secs(3600);

/// Parse a query string (the part AFTER `?`; without the leading `?`)
/// into a `Filter`. Returns `Ok(Filter::default())` for an empty string.
///
/// Repeated parameters accumulate into the corresponding `HashSet`
/// (OR semantics). Recognized scalar params (`since`, `until`,
/// `compute`) take the LAST occurrence when repeated.
///
/// Unknown params are rejected with `FilterError::UnknownParam` so
/// that a typo (e.g. `?vender=vendor-b`) fails loud rather than
/// silently subscribing to "" + everything.
pub fn parse_filter(query: &str) -> Result<Filter, FilterError> {
    let mut f = Filter::default();
    if query.is_empty() {
        return Ok(f);
    }

    for piece in query.split('&') {
        if piece.is_empty() {
            continue;
        }
        let (raw_key, raw_value) = match piece.split_once('=') {
            Some((k, v)) => (k, v),
            None => return Err(FilterError::MalformedQuery(piece.to_string())),
        };
        let key = percent_decode(raw_key);
        let value = percent_decode(raw_value);
        if value.is_empty() {
            // A bare `?vendor=` is a degenerate filter; skip silently.
            // (A literal empty value matches nothing on the apply side.)
            continue;
        }
        match key.as_str() {
            "key" => {
                f.keys.insert(value);
            }
            "vendor" => {
                f.vendors.insert(value);
            }
            "role" => {
                f.roles.insert(value);
            }
            "site" => {
                f.sites.insert(value);
            }
            "format" => {
                f.formats.insert(value);
            }
            "since" => {
                let d = parse_duration(&value).ok_or_else(|| FilterError::BadDuration {
                    key: "since".to_string(),
                    value: value.clone(),
                })?;
                if d > MAX_SINCE {
                    return Err(FilterError::SinceTooLarge {
                        got_secs: d.as_secs(),
                    });
                }
                f.since = Some(d);
            }
            "until" => {
                if value == "now" {
                    f.until = Some(Duration::ZERO);
                } else {
                    let d = parse_duration(&value).ok_or_else(|| FilterError::BadDuration {
                        key: "until".to_string(),
                        value: value.clone(),
                    })?;
                    f.until = Some(d);
                }
            }
            "compute" => {
                f.compute = Some(match value.as_str() {
                    "browser" => ComputePlacement::Browser,
                    "server" => ComputePlacement::Server,
                    other => return Err(FilterError::BadCompute(other.to_string())),
                });
            }
            other => return Err(FilterError::UnknownParam(other.to_string())),
        }
    }

    Ok(f)
}

/// Parse a duration like `10s`, `5m`, `1h`, `90s`. Returns None on
/// any other shape. We do NOT accept fractional values (`1.5h`) — keep
/// the grammar shareable + URL-bar-typable.
fn parse_duration(s: &str) -> Option<Duration> {
    if s.is_empty() {
        return None;
    }
    let (digits, unit) = s.split_at(s.len() - 1);
    let n: u64 = digits.parse().ok()?;
    let secs = match unit {
        "s" => n,
        "m" => n.checked_mul(60)?,
        "h" => n.checked_mul(3600)?,
        _ => return None,
    };
    Some(Duration::from_secs(secs))
}

/// Minimal `application/x-www-form-urlencoded` decode: turn `+` into
/// space and `%xx` into the corresponding byte. Falls back to the raw
/// input on any malformed sequence (a `?key=%zz` returns a String that
/// will then fail downstream validation, not silently lose the value).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let h1 = hex_nibble(bytes[i + 1]);
                let h2 = hex_nibble(bytes[i + 2]);
                match (h1, h2) {
                    (Some(a), Some(b)) => {
                        out.push((a << 4) | b);
                        i += 3;
                    }
                    _ => {
                        // Malformed escape — preserve byte and advance.
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> String {
        x.to_string()
    }

    #[test]
    fn empty_query_yields_empty_filter() {
        let f = parse_filter("").unwrap();
        assert!(f.is_empty());
    }

    #[test]
    fn single_key_filter() {
        let f = parse_filter("key=vendor-a/observations/v1").unwrap();
        assert!(f.keys.contains("vendor-a/observations/v1"));
        assert!(f.vendors.is_empty());
        assert!(!f.is_empty());
    }

    #[test]
    fn vendor_filter() {
        let f = parse_filter("vendor=vendor-b").unwrap();
        assert!(f.vendors.contains("vendor-b"));
    }

    #[test]
    fn repeated_vendor_or_within() {
        let f = parse_filter("vendor=vendor-b&vendor=vendor-a").unwrap();
        assert_eq!(f.vendors, [s("vendor-b"), s("vendor-a")].into_iter().collect());
    }

    #[test]
    fn multiple_params_and_together() {
        let f = parse_filter("vendor=vendor-b&role=sensor&site=alpha-fob").unwrap();
        assert!(f.vendors.contains("vendor-b"));
        assert!(f.roles.contains("sensor"));
        assert!(f.sites.contains("alpha-fob"));
    }

    #[test]
    fn since_seconds() {
        let f = parse_filter("since=30s").unwrap();
        assert_eq!(f.since, Some(Duration::from_secs(30)));
    }

    #[test]
    fn since_minutes() {
        let f = parse_filter("since=10m").unwrap();
        assert_eq!(f.since, Some(Duration::from_secs(600)));
    }

    #[test]
    fn since_hours_at_cap() {
        let f = parse_filter("since=1h").unwrap();
        assert_eq!(f.since, Some(Duration::from_secs(3600)));
    }

    #[test]
    fn since_exceeds_cap_rejected() {
        let err = parse_filter("since=2h").unwrap_err();
        match err {
            FilterError::SinceTooLarge { got_secs } => assert_eq!(got_secs, 7200),
            other => panic!("expected SinceTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn since_bad_unit_rejected() {
        assert!(parse_filter("since=10x").is_err());
    }

    #[test]
    fn since_no_unit_rejected() {
        assert!(parse_filter("since=10").is_err());
    }

    #[test]
    fn until_now_zero_duration() {
        let f = parse_filter("until=now").unwrap();
        assert_eq!(f.until, Some(Duration::ZERO));
    }

    #[test]
    fn until_duration() {
        let f = parse_filter("until=5m").unwrap();
        assert_eq!(f.until, Some(Duration::from_secs(300)));
    }

    #[test]
    fn compute_browser() {
        let f = parse_filter("compute=browser").unwrap();
        assert_eq!(f.compute, Some(ComputePlacement::Browser));
    }

    #[test]
    fn compute_server() {
        let f = parse_filter("compute=server").unwrap();
        assert_eq!(f.compute, Some(ComputePlacement::Server));
    }

    #[test]
    fn compute_invalid_rejected() {
        let err = parse_filter("compute=peer").unwrap_err();
        match err {
            FilterError::BadCompute(v) => assert_eq!(v, "peer"),
            other => panic!("expected BadCompute, got {other:?}"),
        }
    }

    #[test]
    fn unknown_param_rejected() {
        let err = parse_filter("vender=vendor-b").unwrap_err();
        match err {
            FilterError::UnknownParam(p) => assert_eq!(p, "vender"),
            other => panic!("expected UnknownParam, got {other:?}"),
        }
    }

    #[test]
    fn malformed_no_equals_rejected() {
        let err = parse_filter("vendor").unwrap_err();
        match err {
            FilterError::MalformedQuery(p) => assert_eq!(p, "vendor"),
            other => panic!("expected MalformedQuery, got {other:?}"),
        }
    }

    #[test]
    fn empty_value_silently_skipped() {
        // ?vendor= is a degenerate filter — design says skip silently,
        // since an empty-vendor literal can't match any real publisher.
        let f = parse_filter("vendor=").unwrap();
        assert!(f.is_empty());
    }

    #[test]
    fn key_wildcards_preserved() {
        let f = parse_filter("key=vendor-a/**").unwrap();
        assert!(f.keys.contains("vendor-a/**"));
    }

    #[test]
    fn percent_decode_in_key() {
        // `/` (%2F) and `*` (%2A) commonly survive URL encoding;
        // verify both decode losslessly.
        let f = parse_filter("key=vendor-a%2Fobservations%2F%2A").unwrap();
        assert!(f.keys.contains("vendor-a/observations/*"));
    }

    #[test]
    fn plus_decodes_to_space() {
        let f = parse_filter("vendor=foo+bar").unwrap();
        assert!(f.vendors.contains("foo bar"));
    }

    #[test]
    fn malformed_percent_preserved() {
        // `%z` is not a valid escape; we preserve the byte rather
        // than silently losing it.
        let f = parse_filter("vendor=foo%zbar").unwrap();
        assert!(f.vendors.iter().any(|v| v.starts_with("foo")));
    }

    #[test]
    fn empty_fragments_skipped() {
        // `?&vendor=vendor-b&` (trailing/leading &) parses cleanly.
        let f = parse_filter("&vendor=vendor-b&").unwrap();
        assert!(f.vendors.contains("vendor-b"));
    }

    #[test]
    fn realistic_example_from_design_doc() {
        // ?role=c2&since=30m&compute=server  per the design doc examples
        let f = parse_filter("role=c2&since=30m&compute=server").unwrap();
        assert!(f.roles.contains("c2"));
        assert_eq!(f.since, Some(Duration::from_secs(1800)));
        assert_eq!(f.compute, Some(ComputePlacement::Server));
    }

    #[test]
    fn another_realistic_example() {
        // ?vendor=vendor-a&format=protobuf
        let f = parse_filter("vendor=vendor-a&format=protobuf").unwrap();
        assert!(f.vendors.contains("vendor-a"));
        assert!(f.formats.contains("protobuf"));
    }
}
