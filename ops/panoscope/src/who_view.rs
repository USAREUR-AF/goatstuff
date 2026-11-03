// SPDX-License-Identifier: Apache-2.0
//
// Web-native `goat who` view model (/api/who/*) — the identity COP
// (ADR 1047) rendered in the browser.
//
// GATE: the roster names identities, grants, and reconcile state —
// identity-administration information, NOT mission data. Access is a
// platform-group check (`RbacConfig::allows_who`, default
// `ADMINS,IAM-ADMIN`), independent of the ADR 1074 data-access lattice:
// a `data.view.all` holder deliberately does not see the roster. Denials
// are explicit 403s.
//
// Data: one bulk `GET /api/access/roster` portal call (built for
// goat-who 0.4.9 — no N+1 on the mesh), cached briefly server-side so a
// page of viewers doesn't hammer the portal; live overlay (presence /
// footprint / band) joins from the same firehose-buffer beats the /api/top
// path decodes. The cross-layer STATE column is derived here, mirroring
// crates/goat-cli/src/tui/who.rs::status_of — the wire `state` is only
// active/cut_off/out_of_band/unknown.

use serde::Serialize;

use crate::health_wire::{fmt_age, EnvDetail};
use crate::top_view::{shares_label_of, TopSources};
use panoscope_core::alias::AliasRegistry;
use panoscope_core::envelope::EnvelopeRegistry;
use panoscope_core::portal::{AccessRoster, AccessRosterEntry};

/// Presence beats stale past 3× their window, same rule as the chips.
const STALE_FACTOR: u64 = 3;

#[derive(Serialize)]
pub struct WhoRow {
    pub anchor: String,
    /// Resolved display name: slot alias (ADR 1039) → roster label →
    /// truncated UUID. Never invented.
    pub name: String,
    pub kind: String,
    pub affiliation: String,
    pub nation: String,
    /// Wire reconcile state (active/cut_off/out_of_band/unknown), verbatim.
    pub wire_state: String,
    /// Why a stray didn't reconcile (untracked / unregistered-key /
    /// no-declared-member); empty for non-strays. `unregistered-key` is
    /// the verify-first class — the frontend renders `stray?`.
    pub recon_reason: String,
    /// Derived cross-layer STATE: cut-off / no-grants / online / on-mesh /
    /// stray / active.
    pub status: &'static str,
    pub status_color: &'static str,
    pub on_mesh: bool,
    pub connected: bool,
    pub last_seen_unix: i64,
    pub writes: usize,
    pub reads: usize,
    /// Furthest onboarding step with positive evidence, verbatim from the
    /// portal roster. Empty = the portal reported none (no progress row, or a
    /// portal predating the field) — the frontend renders that as `unknown`,
    /// NEVER as "did not start". Carried so an admin can tell someone quiet
    /// because they finished and stopped from someone quiet because they never
    /// finished arriving.
    pub onboarding_stage: String,
    /// Honest liveness from the presence beat: "alive"/"dropped"/"unmetered"
    /// (a stale or absent beat reads unmetered, never re-asserted live).
    pub live: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_age: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop_count: Option<u64>,
    /// Live producing rate off the footprint beat; absent when the anchor
    /// has no fresh footprint (rendered "—", not zero).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byte_rate: Option<f64>,
    pub shares: String,
    pub near_limit: bool,
    /// Envelope band (ADR 1045) against the live footprint; "—" when no
    /// envelope or no live rate.
    pub band_label: String,
    pub band_color: String,
}

#[derive(Serialize)]
pub struct WhoTotals {
    pub rows: usize,
    pub online: usize,
    pub strays: usize,
    pub cut_off: usize,
    pub no_grants: usize,
    /// How many rows the portal positively reports as short of `publishing`.
    /// Rows with no reported stage are NOT counted — unknown is not unfinished.
    pub unfinished_onboarding: usize,
}

#[derive(Serialize)]
pub struct WhoView {
    /// Portal fetch state: "ok" / "stale" (serving last-good after a failed
    /// refresh) / "disabled" (no portal configured at this deployment).
    pub portal_state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roster_age: Option<String>,
    pub rows: Vec<WhoRow>,
    pub totals: WhoTotals,
    pub notes: Vec<String>,
    /// Declared-registry provenance, passed through when the portal
    /// stamps it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freshness: Option<Freshness>,
    pub generated_at_ms: u64,
}

#[derive(Serialize)]
pub struct Freshness {
    pub generated_at: String,
    pub member_count: i64,
}

/// The derived cross-layer STATE (label, honesty color) — the mirror of
/// tui/who.rs::status_of, in its priority order.
pub fn status_of(e: &AccessRosterEntry) -> (&'static str, &'static str) {
    match e.state.as_str() {
        "cut_off" | "cut-off" | "revoked" => return ("cut-off", "red"),
        "out_of_band" | "out-of-band" => return ("stray", "amber"),
        _ => {}
    }
    if e.writes == 0 && e.reads == 0 {
        return ("no-grants", "amber");
    }
    if e.on_mesh && e.connected {
        return ("online", "green");
    }
    if e.on_mesh {
        return ("on-mesh", "cyan");
    }
    // Registry says active and grants exist, but nothing has seen this
    // identity on the mesh or the fabric. Green is reserved for a live
    // signal; this one has none, so it renders dim (F-318 honesty: on a
    // per-identity slot pool most rows sit here, and painting them green
    // read as "everyone is here").
    ("active", "")
}

/// Project the roster + live beats into the who view. Pure over inputs.
pub fn build_who_view(
    roster: &AccessRoster,
    portal_state: &'static str,
    roster_age: Option<String>,
    src: &TopSources,
    aliases: &AliasRegistry,
    envelopes: &EnvelopeRegistry,
    now_ms: u64,
) -> WhoView {
    // Presence beat, only if fresh — stale reads unmetered for everyone.
    let presence = src.presence.as_ref().filter(|b| {
        let w = (b.msg.window_secs as u64).max(1);
        now_ms.saturating_sub(b.seen_ms) < w * STALE_FACTOR * 1000
    });
    let presence_detail = presence.and_then(|b| match &b.msg.detail {
        Some(EnvDetail::Presence(d)) => Some((d, b.msg.ts_unix_ms)),
        _ => None,
    });

    let rows: Vec<WhoRow> = roster
        .rows
        .iter()
        .map(|e| {
            let anchor = if !e.write_anchor.is_empty() { &e.write_anchor } else { &e.id };
            let (status, status_color) = status_of(e);
            let name = aliases.alias_of(anchor).unwrap_or_else(|| {
                if !e.label.is_empty() { e.label.clone() } else { aliases.render(anchor) }
            });

            let (live, live_age, drop_count) = match presence_detail {
                None => ("unmetered", None, None),
                Some((d, beat_ts)) => match d.peers.iter().find(|p| p.write_root == *anchor) {
                    None => ("unmetered", None, None),
                    Some(p) => {
                        let age = fmt_age(beat_ts.saturating_sub(p.since_unix_ms));
                        if p.alive {
                            ("alive", Some(age), None)
                        } else {
                            ("dropped", Some(age), Some(p.drop_count))
                        }
                    }
                },
            };

            let fp = src.footprints.iter().find(|b| b.msg.peer == *anchor).map(|b| &b.msg);
            let (band_label, band_color) = match fp
                .and_then(|f| envelopes.band(anchor, f.sample_rate, f.byte_rate))
            {
                Some(band) => (band.label().to_string(), band.color().to_string()),
                None => ("—".to_string(), String::new()),
            };

            WhoRow {
                anchor: anchor.clone(),
                name,
                kind: e.kind.clone(),
                affiliation: e.affiliation.clone(),
                nation: e.nation.clone(),
                wire_state: e.state.clone(),
                recon_reason: e.recon_reason.clone(),
                status,
                status_color,
                on_mesh: e.on_mesh,
                connected: e.connected,
                last_seen_unix: e.last_seen_unix,
                writes: e.writes,
                reads: e.reads,
                onboarding_stage: e.onboarding_stage.clone(),
                live,
                live_age,
                drop_count,
                sample_rate: fp.map(|f| f.sample_rate),
                byte_rate: fp.map(|f| f.byte_rate),
                shares: fp.map(shares_label_of).unwrap_or_else(|| "—".to_string()),
                near_limit: fp.map(|f| f.near_limit).unwrap_or(false),
                band_label,
                band_color,
            }
        })
        .collect();

    let totals = WhoTotals {
        rows: rows.len(),
        online: rows.iter().filter(|r| r.status == "online").count(),
        strays: rows.iter().filter(|r| r.status == "stray").count(),
        cut_off: rows.iter().filter(|r| r.status == "cut-off").count(),
        no_grants: rows.iter().filter(|r| r.status == "no-grants").count(),
        // Rows the portal positively reports as short of publishing. Rows with
        // an EMPTY stage are excluded deliberately: empty means the portal told
        // us nothing, and counting unknowns as unfinished would invent a
        // number. An undercount is honest here; an overcount is not.
        unfinished_onboarding: rows
            .iter()
            .filter(|r| !r.onboarding_stage.is_empty() && r.onboarding_stage != "publishing")
            .count(),
    };

    WhoView {
        portal_state,
        roster_age,
        rows,
        totals,
        notes: roster.notes.clone(),
        freshness: roster.freshness.as_ref().map(|f| Freshness {
            generated_at: f.generated_at.clone(),
            member_count: f.member_count,
        }),
        generated_at_ms: now_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(state: &str, writes: usize, reads: usize, on_mesh: bool, connected: bool) -> AccessRosterEntry {
        AccessRosterEntry {
            id: "abc123".into(),
            state: state.into(),
            writes,
            reads,
            on_mesh,
            connected,
            ..Default::default()
        }
    }

    /// The onboarding column must never turn "the portal told us nothing" into
    /// "they never started". An empty stage is unknown, and unknown must not be
    /// counted as unfinished — an undercount is honest, an overcount invents a
    /// claim about a person.
    #[test]
    fn unreported_onboarding_stage_is_unknown_not_unfinished() {
        let mut reported = entry("active", 2, 1, true, true);
        reported.onboarding_stage = "mesh-enrolled".into();
        let mut finished = entry("active", 2, 1, true, true);
        finished.onboarding_stage = "publishing".into();
        // Empty: a portal predating the field, or no progress row.
        let silent = entry("active", 2, 1, true, true);
        assert_eq!(silent.onboarding_stage, "");

        let roster = AccessRoster {
            rows: vec![reported, finished, silent],
            ..Default::default()
        };
        let view = build_who_view(
            &roster,
            "ok",
            None,
            &TopSources::default(),
            &AliasRegistry::default(),
            &EnvelopeRegistry::default(),
            0,
        );
        // Only the one row positively short of publishing counts.
        assert_eq!(view.totals.unfinished_onboarding, 1);
        // And the stage rides through verbatim — no substitution, no default.
        assert_eq!(view.rows.iter().filter(|r| r.onboarding_stage.is_empty()).count(), 1);
    }

    #[test]
    fn status_mirrors_the_tui_priority_order() {
        assert_eq!(status_of(&entry("cut_off", 5, 5, true, true)).0, "cut-off");
        assert_eq!(status_of(&entry("out_of_band", 5, 5, true, true)).0, "stray");
        assert_eq!(status_of(&entry("active", 0, 0, true, true)).0, "no-grants");
        assert_eq!(status_of(&entry("active", 2, 1, true, true)).0, "online");
        assert_eq!(status_of(&entry("active", 2, 1, true, false)).0, "on-mesh");
        assert_eq!(status_of(&entry("active", 2, 1, false, false)).0, "active");
    }

    /// Green means a live signal. `online` has one; `active` is registry
    /// state with no presence behind it and must not share the colour.
    #[test]
    fn active_without_a_live_signal_is_not_green() {
        assert_eq!(status_of(&entry("active", 2, 1, true, true)).1, "green");
        assert_eq!(status_of(&entry("active", 2, 1, true, false)).1, "cyan");
        assert_eq!(status_of(&entry("active", 2, 1, false, false)).1, "");
    }

    #[test]
    fn view_counts_and_unmetered_liveness_without_presence_beat() {
        let roster = AccessRoster {
            rows: vec![
                entry("active", 2, 1, true, true),
                entry("out_of_band", 1, 0, true, false),
            ],
            notes: vec![],
            freshness: None,
        };
        let v = build_who_view(
            &roster,
            "ok",
            None,
            &TopSources::default(),
            &AliasRegistry::default(),
            &EnvelopeRegistry::empty(),
            1_000,
        );
        assert_eq!(v.totals.rows, 2);
        assert_eq!(v.totals.online, 1);
        assert_eq!(v.totals.strays, 1);
        // No presence beat ⇒ honest unmetered, never "offline".
        assert!(v.rows.iter().all(|r| r.live == "unmetered"));
        // No footprint ⇒ absent rates (rendered —), not zero.
        assert!(v.rows.iter().all(|r| r.sample_rate.is_none()));
        assert_eq!(v.rows[0].band_label, "—");
    }
}
