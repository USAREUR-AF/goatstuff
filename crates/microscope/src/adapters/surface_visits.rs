//! Surface visits (design source S7): the `ops/audit/surface/<surface>/<actor>`
//! stream, read from the same MinIO audit-integrity tier the fabric and
//! operator adapters use.
//!
//! WHAT THIS ADDS THAT hydra-consents CANNOT. A consent grant is a STANDING
//! record with a moving `handled_at`: it says this person reached this service,
//! and when they last did. Several visits to one surface collapse into one row,
//! so the grant can never answer "how many times" or "on which days". Each row
//! here is ONE request, so counts and a history fall out of the same stream.
//!
//! The two sources are complementary and neither subsumes the other. A consent
//! grant survives a gap in this stream (the emitter down, the router wedged);
//! this stream survives a REVOKED grant, which vanishes from Hydra's API
//! entirely. Read together they distinguish "never came" from "came, and the
//! grant was later revoked" — apart, neither can (the F-177 class).
//!
//! DELIBERATELY NOT DECRYPTED, exactly as operator-audit is not. The payload is
//! age-encrypted to the operator roster and this service holds no roster
//! identity; key custody stays with operators. What the ciphertext still proves,
//! with strong attribution, is carried by the KEY PATH rather than the body:
//! WHICH SURFACE, WHICH PERSON, and WHEN — which is the whole of "who visited
//! what, how often". Only the per-request detail (path, status, duration) needs
//! the roster key, and that is the deliberate follow-on, not a gap.

use std::collections::BTreeMap;

use async_trait::async_trait;
use panoscope_core::AuditEvent;

use crate::adapters::{
    audit_max_rows, rows_clause, tail_from, AdapterOutput, NewestRows, SourceAdapter,
    AUDIT_MAX_OBJECTS,
};
use crate::config::Config;
use crate::event::{Attribution, TimelineEvent};
use crate::spine::EntitySpine;

pub struct SurfaceVisits;

const SOURCE: &str = "surface-visits";
const VISIT_PREFIX: &str = "ops/audit/surface/";

/// Split `ops/audit/surface/<surface>/<actor>` into its two identifying parts.
/// An actor segment may itself contain `/`-safe substitutions made by the
/// emitter, so only the FIRST two segments after the prefix are significant and
/// anything deeper is ignored rather than guessed at.
fn split_key(key: &str) -> Option<(String, String)> {
    let rest = key.strip_prefix(VISIT_PREFIX)?;
    let mut it = rest.splitn(3, '/');
    let surface = it.next()?.to_string();
    let actor = it.next()?.to_string();
    if surface.is_empty() || actor.is_empty() {
        return None;
    }
    Some((surface, actor))
}

/// The fold over the `ops` walk: per-surface counts over EVERY matching visit,
/// and the newest matching visits as bounded timeline rows.
struct VisitFold {
    keys: Vec<String>,
    rows: NewestRows,
    per_surface: BTreeMap<String, usize>,
}

impl VisitFold {
    fn new(keys: Vec<String>, max_rows: usize) -> Self {
        Self {
            keys,
            rows: NewestRows::new(max_rows),
            per_surface: BTreeMap::new(),
        }
    }

    fn observe(&mut self, r: AuditEvent) {
        let Some((surface, actor)) = split_key(&r.key_expr) else {
            return;
        };
        if !self.keys.iter().any(|k| actor.eq_ignore_ascii_case(k)) {
            return;
        }
        *self.per_surface.entry(surface.clone()).or_default() += 1;
        self.rows.offer(crate::timefmt::canonical_str(&r.ts), move |ts| TimelineEvent {
            ts,
            source: SOURCE,
            action: "surface-visit".into(),
            actor: actor.clone(),
            target: Some(surface.clone()),
            summary: format!(
                "visited {surface} ({} B age-encrypted — path and status decrypt operator-side)",
                r.payload_len
            ),
            kind: crate::event::EventKind::classify("surface.visit"),
            outcome: crate::event::Outcome::Unrecorded,
            direction: crate::event::Direction::BySubject,
            detail: None,
            attribution: Attribution::Strong,
            raw: serde_json::to_value(&r).ok(),
        });
    }
}

#[async_trait]
impl SourceAdapter for SurfaceVisits {
    fn name(&self) -> &'static str {
        SOURCE
    }

    async fn collect(&self, cfg: &Config, spine: &EntitySpine) -> AdapterOutput {
        let tail = match tail_from(cfg) {
            Ok(Some(t)) => t,
            Ok(None) => return AdapterOutput::skipped(SOURCE, "MICROSCOPE_AUDIT_* unset"),
            Err(e) => return AdapterOutput::error(SOURCE, e),
        };
        // The stream's first key segment is `ops`, so one vendor-filtered walk
        // narrows to the ops namespace before the prefix match — same shape as
        // operator-audit above it. Rows are folded as they stream: the counts
        // cover every visit in the window, the timeline keeps the newest.
        let keys: Vec<String> = spine.match_keys().iter().map(|k| k.to_lowercase()).collect();
        let mut fold = VisitFold::new(keys, audit_max_rows(cfg));
        let stats = match tail
            .stream_recent_events(cfg.audit_window_hours, Some("ops"), |r| fold.observe(r))
            .await
        {
            Ok(v) => v,
            Err(e) => return AdapterOutput::error(SOURCE, e),
        };
        let partial = stats.objects_fetched >= AUDIT_MAX_OBJECTS;
        let per_surface = fold.per_surface;
        let (events, seen) = fold.rows.finish();

        let mut coverage = if partial {
            format!(
                "PARTIAL {}h (object ceiling — older hours uncovered)",
                cfg.audit_window_hours
            )
        } else {
            format!("last {}h", cfg.audit_window_hours)
        };
        if let Some(c) = rows_clause(events.len(), seen) {
            coverage.push_str(", ");
            coverage.push_str(&c);
        }

        // The count per surface is the answer to the question this source was
        // added for, so it goes in the detail line rather than being left for
        // the reader to tally off the timeline. n=0 is stated as a window
        // result, never as "never visited": absence here means the emitter
        // published nothing in the window, which a stopped emitter also
        // produces.
        let breakdown = if per_surface.is_empty() {
            "no visits in window".to_string()
        } else {
            per_surface
                .iter()
                .map(|(s, n)| format!("{s} x{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        };

        AdapterOutput::ok(
            SOURCE,
            &format!(
                "per-request visits from audit-integrity, {coverage}: {breakdown}. \
                 Counts and times are read from the key path; path and status are \
                 age-encrypted to the operator roster and stay operator-side. \
                 A zero here is a window result, not proof nobody came — pair it \
                 with the standing consent grant in hydra-consents."
            ),
            events,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_paths_split_into_surface_and_actor() {
        assert_eq!(
            split_key("ops/audit/surface/brief/someone@example.invalid"),
            Some(("brief".into(), "someone@example.invalid".into()))
        );
        // Anything deeper than surface/actor is ignored rather than guessed at.
        assert_eq!(
            split_key("ops/audit/surface/demogame/a@b.c/extra/bits"),
            Some(("demogame".into(), "a@b.c".into()))
        );
    }

    #[test]
    fn foreign_and_malformed_keys_are_not_visits() {
        // The operator stream shares the ops/audit prefix and must not be
        // mistaken for a visit.
        assert_eq!(split_key("ops/audit/operator/maintainer-operator"), None);
        assert_eq!(split_key("ops/audit/surface/brief"), None);
        assert_eq!(split_key("ops/audit/surface//actor"), None);
        assert_eq!(split_key("something/else"), None);
    }

    /// Counts cover every visit in a large window; the timeline keeps at most
    /// the ceiling, newest first.
    #[test]
    fn a_large_window_counts_everything_and_keeps_at_most_the_ceiling() {
        use crate::adapters::test_audit_event;
        const N: usize = 200_000;
        const K: usize = 300;
        let mut fold = VisitFold::new(vec!["a@b.c".into()], K);
        for i in 0..N {
            let key = match i % 4 {
                0 => "ops/audit/surface/brief/a@b.c",
                1 => "ops/audit/surface/demogame/A@B.C",
                2 => "ops/audit/surface/brief/other@b.c",
                _ => "ops/audit/operator/a@b.c",
            };
            fold.observe(test_audit_event(key, i));
        }
        assert_eq!(fold.per_surface.get("brief"), Some(&(N / 4)));
        assert_eq!(fold.per_surface.get("demogame"), Some(&(N / 4)));
        assert_eq!(fold.per_surface.len(), 2);
        let (events, seen) = fold.rows.finish();
        assert_eq!(seen, N / 2);
        assert_eq!(events.len(), K);
        let oldest_kept = crate::timefmt::canonical_str(&test_audit_event("x", N - 2 * K).ts);
        assert!(events.iter().all(|e| e.ts >= oldest_kept));
    }
}
