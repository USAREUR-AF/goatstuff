// SPDX-License-Identifier: Apache-2.0
//
// Router log line classification. The one line this increment reads is the uhlc
// timestamp rejection, which a Zenoh router logs once per rejected sample:
//
//   <ts>  WARN <task> uhlc: incoming timestamp from <zid> exceeding delta 500ms
//         is rejected: <peer time> vs. now: <router time>
//
// The router also logs an ERROR line carrying the same text in parentheses
// ("Error treating timestamp for received Data (incoming timestamp from ...").
// That is the same rejection said twice; only the line whose tracing target is
// `uhlc` counts, so each rejected sample is counted once.

use std::time::UNIX_EPOCH;

/// The marker that identifies the counted line: the `uhlc` tracing target
/// immediately followed by the rejection text.
const UHLC_MARKER: &str = "uhlc: incoming timestamp from ";
/// Logged by the Zenoh runtime at start-up; names the router's own Zenoh ID.
const ZID_MARKER: &str = "Using ZID: ";

/// One rejected timestamp, as the router reported it.
#[derive(Debug, Clone, PartialEq)]
pub struct Rejection {
    /// Zenoh ID of the router that stamped the sample (1 to 32 lowercase hex).
    pub zid: String,
    /// The router's acceptance limit, parsed from "exceeding delta <d>".
    pub limit_seconds: f64,
    /// The rejected timestamp, nanoseconds since the Unix epoch.
    pub peer_unix_ns: i64,
    /// The router's own clock when it rejected it, nanoseconds since the epoch.
    pub router_unix_ns: i64,
}

impl Rejection {
    /// Peer time minus router time, in seconds. Positive = the peer is ahead.
    pub fn offset_seconds(&self) -> f64 {
        (self.peer_unix_ns - self.router_unix_ns) as f64 / 1e9
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LineKind {
    /// A counted uhlc rejection.
    Rejection(Rejection),
    /// The router announcing its own Zenoh ID.
    RouterZid(String),
    /// A uhlc rejection line whose shape did not parse: the log format may
    /// have moved, so the reading must not grade clean.
    Unparsed,
    /// Any other line. It still proves the log stream is alive.
    Other,
}

/// Remove ANSI escape sequences (CSI `ESC [ ... final`, OSC `ESC ] ... BEL`
/// or `ESC ] ... ESC \`, and two-byte escapes).
pub fn strip_ansi(s: &str) -> String {
    if !s.contains('\x1b') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('[') => {
                // Parameters and intermediates, then one final byte 0x40..=0x7e.
                for n in it.by_ref() {
                    if ('\x40'..='\x7e').contains(&n) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(n) = it.next() {
                    if n == '\x07' {
                        break;
                    }
                    if n == '\x1b' {
                        if it.peek() == Some(&'\\') {
                            it.next();
                        }
                        break;
                    }
                }
            }
            _ => {} // two-byte escape, or a lone ESC at the end
        }
    }
    out
}

/// A Zenoh ID as the router prints it: 1 to 32 lowercase hex characters.
pub fn is_zid(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Canonical form for comparing Zenoh IDs: lowercase, leading zeros removed.
pub fn normalize_zid(s: &str) -> String {
    let lower = s.trim().to_ascii_lowercase();
    let trimmed = lower.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Parse a duration such as `500ms`, `1s`, `1.5s`, `250us`, `2m` into seconds.
pub fn parse_duration_secs(s: &str) -> Option<f64> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().ok()?;
    let scale = match unit {
        "ns" => 1e-9,
        "us" | "µs" => 1e-6,
        "ms" => 1e-3,
        "s" | "" => 1.0,
        "m" | "min" => 60.0,
        _ => return None,
    };
    let v = n * scale;
    (v.is_finite() && v >= 0.0).then_some(v)
}

/// Parse a uhlc time as printed in the rejection line: RFC 3339 UTC with up to
/// nine fractional digits, optionally followed by `/<id>`; or a raw NTP64
/// integer (seconds since the Unix epoch in the high 32 bits, fraction in the
/// low 32 bits), which older uhlc versions printed.
pub fn parse_time_ns(s: &str) -> Option<i64> {
    let s = s.trim().trim_end_matches([')', ',', '.', ';']);
    let s = s.split('/').next()?;
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        let raw: u64 = s.parse().ok()?;
        let secs = (raw >> 32) as i64;
        let frac = ((raw & 0xffff_ffff) as u128 * 1_000_000_000u128) >> 32;
        return Some(secs * 1_000_000_000 + frac as i64);
    }
    let t = humantime::parse_rfc3339(s).ok()?;
    let d = t.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(d.as_nanos()).ok()
}

/// Classify one raw log line (ANSI codes allowed).
pub fn classify(raw: &str) -> LineKind {
    let line = strip_ansi(raw);
    if let Some(at) = line.find(UHLC_MARKER) {
        return match parse_rejection(&line[at + UHLC_MARKER.len()..]) {
            Some(r) => LineKind::Rejection(r),
            None => LineKind::Unparsed,
        };
    }
    if let Some(at) = line.find(ZID_MARKER) {
        let zid = line[at + ZID_MARKER.len()..]
            .split(|c: char| !c.is_ascii_alphanumeric())
            .next()
            .unwrap_or("");
        if is_zid(zid) {
            return LineKind::RouterZid(zid.to_string());
        }
    }
    LineKind::Other
}

/// Parse the text after the marker:
/// `<zid> exceeding delta <d> is rejected: <peer time> vs. now: <router time>`.
fn parse_rejection(rest: &str) -> Option<Rejection> {
    let mut words = rest.split_whitespace();
    let zid = words.next()?;
    if !is_zid(zid) {
        return None;
    }
    if words.next()? != "exceeding" || words.next()? != "delta" {
        return None;
    }
    let limit_seconds = parse_duration_secs(words.next()?)?;
    if words.next()? != "is" || words.next()? != "rejected:" {
        return None;
    }
    let peer_unix_ns = parse_time_ns(words.next()?)?;
    if words.next()? != "vs." || words.next()? != "now:" {
        return None;
    }
    let router_unix_ns = parse_time_ns(words.next()?)?;
    Some(Rejection {
        zid: zid.to_string(),
        limit_seconds,
        peer_unix_ns,
        router_unix_ns,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made-up Zenoh IDs, same shape as real ones.
    const WARN: &str = "2026-11-13T13:18:41.152065Z  WARN  rx-1366 ThreadId(5912) uhlc: incoming timestamp from 3c9a51e0d2b7f4a6c18e05d9b3a7f2e1 exceeding delta 500ms is rejected: 2026-11-13T13:18:58.854766973Z vs. now: 2026-11-13T13:18:41.152060252Z";
    const ERROR: &str = "2026-11-13T13:18:41.152081Z ERROR  rx-1366 ThreadId(5912) zenoh::net::routing::dispatcher::pubsub: Error treating timestamp for received Data (incoming timestamp from 3c9a51e0d2b7f4a6c18e05d9b3a7f2e1 exceeding delta 500ms is rejected: 2026-11-13T13:18:58.854766973Z vs. now: 2026-11-13T13:18:41.152060252Z). Replace timestamp: Some(7707644891537264496/e4d2a9c07b13f58a26c9d0e1f7b4a385)";

    fn rejection(line: &str) -> Rejection {
        match classify(line) {
            LineKind::Rejection(r) => r,
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    #[test]
    fn the_warn_line_is_a_rejection_with_offset_and_limit() {
        let r = rejection(WARN);
        assert_eq!(r.zid, "3c9a51e0d2b7f4a6c18e05d9b3a7f2e1");
        assert_eq!(r.limit_seconds, 0.5);
        let off = r.offset_seconds();
        assert!((off - 17.702706721).abs() < 1e-6, "offset {off}");
    }

    #[test]
    fn the_error_line_is_not_counted() {
        assert_eq!(classify(ERROR), LineKind::Other);
    }

    #[test]
    fn ansi_colored_lines_parse_the_same() {
        let colored = "\x1b[2m2026-11-13T13:18:41.152065Z\x1b[0m \x1b[33m WARN\x1b[0m \x1b[2mrx-1366\x1b[0m ThreadId(5912) \x1b[2muhlc\x1b[0m\x1b[2m:\x1b[0m incoming timestamp from 3c9a51e0d2b7f4a6c18e05d9b3a7f2e1 exceeding delta 500ms is rejected: 2026-11-13T13:18:58.854766973Z vs. now: 2026-11-13T13:18:41.152060252Z";
        assert_eq!(rejection(colored), rejection(WARN));
        let colored_err = format!("\x1b[31mERROR\x1b[0m {ERROR}");
        assert_eq!(classify(&colored_err), LineKind::Other);
    }

    #[test]
    fn strip_ansi_handles_csi_osc_and_a_trailing_escape() {
        assert_eq!(strip_ansi("a\x1b[1;31mb\x1b[0mc"), "abc");
        assert_eq!(strip_ansi("a\x1b]0;title\x07b"), "ab");
        assert_eq!(strip_ansi("a\x1b]8;;x\x1b\\b"), "ab");
        assert_eq!(strip_ansi("ab\x1b"), "ab");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn a_short_zid_and_another_limit_parse() {
        let line = "WARN uhlc: incoming timestamp from 9f2 exceeding delta 1500ms is rejected: 2026-11-13T13:00:03Z vs. now: 2026-11-13T13:00:00Z";
        let r = rejection(line);
        assert_eq!(r.zid, "9f2");
        assert_eq!(r.limit_seconds, 1.5);
        assert_eq!(r.offset_seconds(), 3.0);
    }

    #[test]
    fn docker_timestamp_prefix_is_tolerated() {
        let line = format!("2026-11-13T13:18:41.152070000Z {WARN}");
        assert_eq!(rejection(&line).zid, "3c9a51e0d2b7f4a6c18e05d9b3a7f2e1");
    }

    #[test]
    fn garbage_and_unrelated_lines() {
        assert_eq!(classify(""), LineKind::Other);
        assert_eq!(
            classify("\u{fffd}\u{fffd} binary junk \x00\x01"),
            LineKind::Other
        );
        assert_eq!(
            classify("2026-11-13T13:18:41Z  INFO zenoh::net::runtime: Zenoh can be reached at: tls/[::]:7447"),
            LineKind::Other
        );
        // The uhlc marker with a broken tail: the log shape moved.
        assert_eq!(
            classify("uhlc: incoming timestamp from XYZ exceeding delta 500ms"),
            LineKind::Unparsed
        );
        assert_eq!(
            classify("uhlc: incoming timestamp from abc exceeding delta 500ms is rejected: soon vs. now: later"),
            LineKind::Unparsed
        );
        assert_eq!(
            classify("uhlc: incoming timestamp from abc exceeding delta lots is rejected: 2026-11-13T13:00:03Z vs. now: 2026-11-13T13:00:00Z"),
            LineKind::Unparsed
        );
        // 33 hex characters is not a Zenoh ID.
        assert_eq!(
            classify("uhlc: incoming timestamp from 3c9a51e0d2b7f4a6c18e05d9b3a7f2e1a exceeding delta 500ms is rejected: 2026-11-13T13:00:03Z vs. now: 2026-11-13T13:00:00Z"),
            LineKind::Unparsed
        );
    }

    #[test]
    fn router_zid_announcement() {
        assert_eq!(
            classify("2026-11-13T13:00:00Z  INFO main ThreadId(1) zenoh::net::runtime: Using ZID: 5be08d2f41a97c36e0d1f8b2a4c6e9d7"),
            LineKind::RouterZid("5be08d2f41a97c36e0d1f8b2a4c6e9d7".to_string())
        );
    }

    #[test]
    fn durations_and_times() {
        assert_eq!(parse_duration_secs("500ms"), Some(0.5));
        assert_eq!(parse_duration_secs("2s"), Some(2.0));
        assert_eq!(parse_duration_secs("250us"), Some(0.00025));
        assert_eq!(parse_duration_secs("ms"), None);
        assert_eq!(parse_duration_secs("5h"), None);
        assert_eq!(parse_time_ns("1970-01-01T00:00:01.5Z"), Some(1_500_000_000));
        assert_eq!(
            parse_time_ns("1970-01-01T00:00:01Z/abc"),
            Some(1_000_000_000)
        );
        // NTP64: 1 s and half a second.
        assert_eq!(
            parse_time_ns(&((1u64 << 32) | (1u64 << 31)).to_string()),
            Some(1_500_000_000)
        );
        assert_eq!(parse_time_ns("yesterday"), None);
    }

    #[test]
    fn zid_normalization() {
        assert_eq!(normalize_zid("00ab"), "ab");
        assert_eq!(normalize_zid("AB"), "ab");
        assert_eq!(normalize_zid("000"), "0");
        assert!(is_zid("0"));
        assert!(!is_zid(""));
        assert!(!is_zid("ABC"));
    }
}
