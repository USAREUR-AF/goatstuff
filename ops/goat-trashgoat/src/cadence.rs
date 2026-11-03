// SPDX-License-Identifier: Apache-2.0
//
// Cadence schedules per docs/design/trashgoat.md §"Cadence spec":
//
//   { "kind": "oneshot" }
//   { "kind": "every",  "interval_ms": 1000 }
//   { "kind": "burst",  "count": 10, "spacing_ms": 50, "then_idle": true }
//
// Each spec carries a cadence; the runner drives publishes from the
// `Cadence::next_tick` stream. oneshot fires once then returns None;
// every fires forever at the declared interval; burst fires `count`
// times spaced by `spacing_ms` then returns None.
//
// The portal-side validator already enforces sanity bounds (interval >
// 0, count > 0, etc., see internal/api/trashgoat_specs.go). The runner
// adds a max-Hz clamp on top per its --max-hz flag — if the requested
// rate would exceed the ceiling the cadence is widened to match the
// ceiling and a warning is logged. The runner is free to refuse cadence
// values it considers abusive (design doc §"Out of scope for v1").

use anyhow::{anyhow, Context, Result};
use std::time::Duration;

/// Parsed cadence. Constructed from a `serde_json::Value` (the
/// `cadence_spec` field on the control message) via `Cadence::parse`.
#[derive(Debug, Clone)]
pub enum Cadence {
    /// Fire once then idle.
    Oneshot,
    /// Fire every `interval`, forever (until pause/retire).
    Every { interval: Duration },
    /// Fire `count` times spaced by `spacing`, then idle.
    Burst { count: u32, spacing: Duration },
}

impl Cadence {
    /// Parse a cadence_spec JSON value. Accepts either a nested object
    /// (per the design doc) OR a JSON-encoded string (per the portal's
    /// current wire shape — `TrashgoatSpec.PayloadSpecJSON` is a `string`
    /// field, so Go marshals it as a quoted JSON string literal). The
    /// runner handles both so portal-side shape changes don't break the
    /// runner.
    pub fn parse(raw: &serde_json::Value) -> Result<Self> {
        let v = unwrap_string_payload(raw)?;
        let obj = v
            .as_object()
            .ok_or_else(|| anyhow!("cadence_spec must be a JSON object, got {v:?}"))?;
        let kind = obj
            .get("kind")
            .and_then(|k| k.as_str())
            .ok_or_else(|| anyhow!("cadence_spec.kind missing or not a string"))?;
        match kind {
            "oneshot" => Ok(Cadence::Oneshot),
            "every" => {
                let interval_ms = obj
                    .get("interval_ms")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| anyhow!("cadence_spec.interval_ms missing or not a positive integer"))?;
                if interval_ms == 0 {
                    return Err(anyhow!("cadence_spec.interval_ms must be > 0"));
                }
                Ok(Cadence::Every {
                    interval: Duration::from_millis(interval_ms),
                })
            }
            "burst" => {
                let count = obj
                    .get("count")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| anyhow!("cadence_spec.count missing or not a positive integer"))?;
                if count == 0 {
                    return Err(anyhow!("cadence_spec.count must be > 0"));
                }
                let spacing_ms = obj
                    .get("spacing_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                Ok(Cadence::Burst {
                    count: count as u32,
                    spacing: Duration::from_millis(spacing_ms),
                })
            }
            other => Err(anyhow!(
                "cadence_spec.kind unknown: {other:?} (expected oneshot|every|burst)"
            )),
        }
    }

    /// Clamp the cadence so its instantaneous rate does not exceed
    /// `max_hz`. Returns the (possibly widened) cadence + a `clamped`
    /// flag the caller can log.
    pub fn clamped(self, max_hz: f64) -> (Self, bool) {
        if max_hz <= 0.0 || !max_hz.is_finite() {
            return (self, false);
        }
        let min_period = Duration::from_secs_f64(1.0 / max_hz);
        match self {
            Cadence::Every { interval } if interval < min_period => {
                (Cadence::Every { interval: min_period }, true)
            }
            Cadence::Burst { count, spacing } if spacing < min_period => {
                (
                    Cadence::Burst {
                        count,
                        spacing: min_period,
                    },
                    true,
                )
            }
            other => (other, false),
        }
    }
}

/// Strip a JSON-string wrapper around a payload/cadence_spec. Portal
/// emits the field as `"payload_spec": "{...}"`. If `raw` is already an
/// object we return it as-is; if it's a string we re-parse.
pub(crate) fn unwrap_string_payload(raw: &serde_json::Value) -> Result<serde_json::Value> {
    match raw {
        serde_json::Value::String(s) => serde_json::from_str(s)
            .with_context(|| format!("re-parse string-wrapped JSON ({} bytes)", s.len())),
        other => Ok(other.clone()),
    }
}

/// A running cadence — owns the bookkeeping for how many ticks remain.
/// Constructed from a `Cadence`; yields the wait duration to apply
/// before each publish via `next_wait`. Returns `None` when the cadence
/// is exhausted (oneshot after one tick, burst after `count` ticks).
#[derive(Debug)]
pub struct CadenceTicker {
    inner: TickerState,
}

#[derive(Debug)]
enum TickerState {
    Oneshot { fired: bool },
    Every { interval: Duration, first: bool },
    Burst { remaining: u32, spacing: Duration, first: bool },
}

impl CadenceTicker {
    pub fn new(cadence: Cadence) -> Self {
        let inner = match cadence {
            Cadence::Oneshot => TickerState::Oneshot { fired: false },
            Cadence::Every { interval } => TickerState::Every {
                interval,
                first: true,
            },
            Cadence::Burst { count, spacing } => TickerState::Burst {
                remaining: count,
                spacing,
                first: true,
            },
        };
        Self { inner }
    }

    /// Returns the wait duration to apply BEFORE the next publish, or
    /// `None` when the cadence is exhausted. The first tick of every /
    /// burst / oneshot is immediate (Duration::ZERO).
    pub fn next_wait(&mut self) -> Option<Duration> {
        match &mut self.inner {
            TickerState::Oneshot { fired } => {
                if *fired {
                    None
                } else {
                    *fired = true;
                    Some(Duration::ZERO)
                }
            }
            TickerState::Every { interval, first } => {
                if *first {
                    *first = false;
                    Some(Duration::ZERO)
                } else {
                    Some(*interval)
                }
            }
            TickerState::Burst {
                remaining,
                spacing,
                first,
            } => {
                if *remaining == 0 {
                    return None;
                }
                *remaining -= 1;
                let wait = if *first {
                    *first = false;
                    Duration::ZERO
                } else {
                    *spacing
                };
                Some(wait)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_oneshot() {
        let c = Cadence::parse(&json!({"kind": "oneshot"})).unwrap();
        assert!(matches!(c, Cadence::Oneshot));
    }

    #[test]
    fn parse_every() {
        let c = Cadence::parse(&json!({"kind": "every", "interval_ms": 1500})).unwrap();
        match c {
            Cadence::Every { interval } => {
                assert_eq!(interval, Duration::from_millis(1500));
            }
            other => panic!("expected Every, got {other:?}"),
        }
    }

    #[test]
    fn parse_burst() {
        let c = Cadence::parse(&json!({
            "kind": "burst",
            "count": 5,
            "spacing_ms": 100
        }))
        .unwrap();
        match c {
            Cadence::Burst { count, spacing } => {
                assert_eq!(count, 5);
                assert_eq!(spacing, Duration::from_millis(100));
            }
            other => panic!("expected Burst, got {other:?}"),
        }
    }

    #[test]
    fn parse_string_wrapped() {
        // Portal emits payload_spec / cadence_spec as JSON strings (Go
        // marshals `string` fields as quoted JSON literals). Runner must
        // handle that shape transparently.
        let wrapped = json!(r#"{"kind":"every","interval_ms":250}"#);
        let c = Cadence::parse(&wrapped).unwrap();
        assert!(matches!(c, Cadence::Every { .. }));
    }

    #[test]
    fn parse_unknown_kind_errors() {
        let err = Cadence::parse(&json!({"kind": "forever"})).unwrap_err();
        assert!(err.to_string().contains("unknown"));
    }

    #[test]
    fn clamped_widens_fast_every() {
        let c = Cadence::Every {
            interval: Duration::from_millis(1),
        };
        let (clamped, was_clamped) = c.clamped(100.0);
        assert!(was_clamped);
        match clamped {
            Cadence::Every { interval } => {
                assert_eq!(interval, Duration::from_millis(10));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn clamped_leaves_slow_every_alone() {
        let c = Cadence::Every {
            interval: Duration::from_secs(1),
        };
        let (clamped, was_clamped) = c.clamped(100.0);
        assert!(!was_clamped);
        match clamped {
            Cadence::Every { interval } => {
                assert_eq!(interval, Duration::from_secs(1));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn ticker_oneshot_fires_once() {
        let mut t = CadenceTicker::new(Cadence::Oneshot);
        assert_eq!(t.next_wait(), Some(Duration::ZERO));
        assert_eq!(t.next_wait(), None);
    }

    #[test]
    fn ticker_every_fires_forever() {
        let mut t = CadenceTicker::new(Cadence::Every {
            interval: Duration::from_millis(500),
        });
        assert_eq!(t.next_wait(), Some(Duration::ZERO));
        assert_eq!(t.next_wait(), Some(Duration::from_millis(500)));
        assert_eq!(t.next_wait(), Some(Duration::from_millis(500)));
    }

    #[test]
    fn ticker_burst_exhausts_after_count() {
        let mut t = CadenceTicker::new(Cadence::Burst {
            count: 3,
            spacing: Duration::from_millis(50),
        });
        assert_eq!(t.next_wait(), Some(Duration::ZERO));
        assert_eq!(t.next_wait(), Some(Duration::from_millis(50)));
        assert_eq!(t.next_wait(), Some(Duration::from_millis(50)));
        assert_eq!(t.next_wait(), None);
    }
}
