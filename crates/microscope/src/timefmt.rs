// SPDX-License-Identifier: Apache-2.0
//! One canonical timestamp form for the merged timeline.
//!
//! Five upstreams feed the timeline and they do NOT agree on shape:
//!
//!   - goat-id-console  Go RFC3339Nano, UTC, trailing zeros stripped
//!   - Ory Kratos       RFC3339 with nanoseconds, Z
//!   - panoscope-core   ISO-8601 UTC, fixed milliseconds
//!   - goat-zenoh-portal  **int64 Unix SECONDS** (`occurred_at INTEGER NOT NULL`)
//!   - NetBird          RFC3339
//!
//! Two bugs came out of pretending otherwise, both fixed by this module:
//!
//! 1. `portal_audit` called `as_str()` on `occurred_at`. serde_json returns
//!    `None` for a number, so `unwrap_or_default()` gave `""` — EVERY portal
//!    event rendered a blank timestamp and, because the merge sorted raw
//!    strings descending, sank below every dated event. Undated and
//!    misordered, on the stream carrying onboarding and probation actions.
//! 2. Go's RFC3339Nano strips trailing zeros, so console-audit emits both
//!    `…:56Z` and `…:56.5Z`. Within the same second `'.'` (0x2E) sorts before
//!    `'Z'` (0x5A), inverting order — invisible until you look closely.
//!
//! The fix is to parse at ingest and re-render to ONE fixed-width form, so
//! lexicographic order is chronological order by construction and the sort
//! needs no cleverness. Fixed width is the whole trick: same-length strings
//! with zero-padded fields compare correctly as bytes.
//!
//! This does NOT violate microscope.md's "events carry their own timestamps,
//! microscope never re-stamps". We reformat the SOURCE's own instant; we
//! never substitute read time. An unparseable value stays unknown (empty)
//! rather than silently becoming "now", which would be the exact lie that
//! rule exists to prevent.

use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use serde_json::Value;

/// Below this, a numeric timestamp is seconds; at or above it, milliseconds.
/// 1e11 seconds is year 5138 and 1e11 ms is 1973 — every real value we handle
/// sits far from the boundary, so the ambiguity is theoretical.
const MILLIS_THRESHOLD: i64 = 100_000_000_000;

/// Canonical form: fixed-width RFC3339, UTC, exactly three fractional digits.
/// `2026-08-07T06:21:48.000Z`
pub fn render(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Parse whatever an adapter found into an instant.
///
/// Accepts a JSON number (Unix seconds or milliseconds) or a JSON string
/// (RFC3339, with or without fractional seconds). Anything else is `None` —
/// the caller renders that as unknown rather than inventing a value.
pub fn parse(v: &Value) -> Option<DateTime<Utc>> {
    match v {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                from_epoch(i)
            } else {
                // Float epoch (fractional seconds) — seen from Python-ish
                // producers. Truncate toward the millisecond.
                n.as_f64()
                    .map(|f| (f * 1000.0) as i64)
                    .and_then(from_millis)
            }
        }
        Value::String(s) => parse_str(s),
        _ => None,
    }
}

/// Parse an already-string timestamp. Kept separate because several adapters
/// hold a typed `String` rather than a `serde_json::Value`.
pub fn parse_str(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // A bare integer arriving as a string is still an epoch — SQLite/JSON
    // round-trips do this often enough to be worth accepting.
    if let Ok(i) = s.parse::<i64>() {
        return from_epoch(i);
    }
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn from_epoch(i: i64) -> Option<DateTime<Utc>> {
    if i.abs() >= MILLIS_THRESHOLD {
        from_millis(i)
    } else {
        Utc.timestamp_opt(i, 0).single()
    }
}

fn from_millis(ms: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_millis_opt(ms).single()
}

/// Ingest helper: JSON value in, canonical string out, empty when unknown.
/// Empty sorts last under the timeline's descending order, which is where an
/// undated event belongs — visible, at the bottom, labelled unknown.
pub fn canonical(v: &Value) -> String {
    parse(v).map(render).unwrap_or_default()
}

/// Ingest helper for adapters holding a `String`.
pub fn canonical_str(s: &str) -> String {
    parse_str(s).map(render).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn portal_epoch_seconds_render_a_real_timestamp() {
        // The exact shape that produced blank cells: occurred_at as int64.
        // 1_785_000_000 = 2026-07-25T17:20:00Z (verified against chrono, not by hand)
        let got = canonical(&json!(1_785_000_000_i64));
        assert_eq!(got, "2026-07-25T17:20:00.000Z", "{got}");
        assert!(!got.is_empty(), "an epoch integer must never render blank");
    }

    #[test]
    fn epoch_milliseconds_are_distinguished_from_seconds() {
        let secs = canonical(&json!(1_785_000_000_i64));
        let millis = canonical(&json!(1_785_000_000_000_i64));
        assert_eq!(secs, millis, "same instant expressed in both units");
    }

    #[test]
    fn rfc3339_variants_normalize_to_one_width() {
        // Go RFC3339Nano strips trailing zeros; Ory emits nanos; panoscope
        // emits fixed millis. All three must land on the same form.
        let a = canonical(&json!("2026-08-07T06:21:56Z"));
        let b = canonical(&json!("2026-08-07T06:21:56.5Z"));
        let c = canonical(&json!("2026-08-07T06:21:56.500000000Z"));
        assert_eq!(a, "2026-08-07T06:21:56.000Z");
        assert_eq!(b, c);
        assert_eq!(a.len(), b.len(), "canonical form must be fixed width");
    }

    #[test]
    fn subsecond_ordering_is_no_longer_inverted() {
        // Pre-fix, '.' (0x2E) < 'Z' (0x5A) made :56.5Z sort BEFORE :56Z.
        let whole = canonical(&json!("2026-08-07T06:21:56Z"));
        let half = canonical(&json!("2026-08-07T06:21:56.5Z"));
        assert!(whole < half, "{whole} must sort before {half}");
    }

    #[test]
    fn offset_timestamps_convert_to_utc() {
        assert_eq!(
            canonical(&json!("2026-08-07T08:21:56+02:00")),
            "2026-08-07T06:21:56.000Z"
        );
    }

    #[test]
    fn unparseable_stays_unknown_never_now() {
        // The honesty property: we do not substitute read time.
        assert_eq!(canonical(&json!("not a timestamp")), "");
        assert_eq!(canonical(&json!(null)), "");
        assert_eq!(canonical(&json!({"a": 1})), "");
        assert_eq!(canonical_str(""), "");
    }

    #[test]
    fn integer_arriving_as_a_string_is_still_an_epoch() {
        assert_eq!(canonical(&json!("1785000000")), "2026-07-25T17:20:00.000Z");
    }
}
