// SPDX-License-Identifier: Apache-2.0
//
// The clock detector: per-peer sliding windows over uhlc rejections, graded
// before anyone is blamed (goatcore docs/design/peer-fault-handling.md, stage 1).
//
// Time bases. A rejection is placed in the window at the router's own "now"
// from the line (the router and this reader share a host, so a clock), which
// keeps a replayed history in its real place instead of counting it as new.
// The window is cut, and silence is judged, against the reader clock passed to
// `tick`. Every function takes "now" as an argument; nothing here reads a clock.
//
// The verdicts, in the order they are decided:
//
//   unmeasured           no log line of any kind for `silence_after`, or the
//                        input ended. Colour `unmetered`, never green, and no
//                        peer is cleared while we cannot see.
//   local_clock_suspect  our host clock is not synchronized (or the check could
//                        not run), or more than half of the known peers show an
//                        offset in the same direction. Amber; no peer is marked
//                        at fault and no per-peer verdict is published.
//   peer_at_fault        one or more peers had a timestamp rejected in the
//                        window. Red; a verdict is published for each.
//   recovering           peers are still tracked but none was rejected in the
//                        window and the clean period has not passed. Amber.
//   clean                nothing tracked. Green, or amber when the window held
//                        uhlc lines this parser could not read.

use crate::clock::LocalClock;
use crate::parse::{classify, normalize_zid, LineKind, Rejection};
use crate::wire::{EnvDetail, EnvHealthV1, PeerFaultDetailV1, PeerFaultKindV1, PeerFaultV1};
use crate::{Attribution, VerdictDoc};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};

#[derive(Debug, Clone)]
pub struct Config {
    pub site: String,
    pub window_ms: i64,
    pub clean_after_ms: i64,
    pub silence_after_ms: i64,
    /// Declared lower bound for the number of peers this router serves; the
    /// denominator of the majority rule. 0 = only what the reader has seen.
    pub peer_population: u32,
    /// Our router's Zenoh ID, when the operator states it. Otherwise it is
    /// learned from the router's "Using ZID:" start-up line, when present.
    pub observed_by: Option<String>,
    /// Set when this reader sits beside one per-partner edge router: every
    /// rejection is attributed to that peering. The Zenoh IDs then all belong
    /// to one partner network, so the "most peers in one direction" rule does
    /// not apply (it would read one fast partner network as our slow clock);
    /// the local sync check is the guard.
    pub peering: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            site: "example-site".to_string(),
            window_ms: 60_000,
            clean_after_ms: 600_000,
            silence_after_ms: 900_000,
            peer_population: 0,
            observed_by: None,
            peering: None,
        }
    }
}

#[derive(Debug, Clone)]
struct PeerTrack {
    zid: String,
    /// (router time ms, offset seconds), oldest first.
    events: VecDeque<(i64, f64)>,
    latest_offset: f64,
    limit_seconds: f64,
    first_seen_ms: i64,
    last_seen_ms: i64,
    /// A verdict naming this peer at fault was published.
    told: bool,
}

/// What one tick produces: the per-site envelope and the per-peer verdicts.
#[derive(Debug, Clone)]
pub struct Reading {
    pub envelope: EnvHealthV1,
    /// (subkey, verdict) for each subject to tell; published on
    /// ops/peer/verdict/<subkey>, where subkey is a Zenoh ID or `peering/<name>`.
    pub verdicts: Vec<(String, VerdictDoc)>,
}

impl Reading {
    pub fn detail(&self) -> &PeerFaultDetailV1 {
        match &self.envelope.detail {
            Some(EnvDetail::Peer(d)) => d,
            None => unreachable!("a reading always carries the peer detail"),
        }
    }
}

pub struct Detector {
    cfg: Config,
    attribution: Attribution,
    peers: BTreeMap<String, PeerTrack>,
    router_zid: Option<String>,
    last_line_ms: Option<i64>,
    input_closed: bool,
    unparsed: VecDeque<i64>,
    /// Normalized Zenoh IDs holding a session with our router; None = unknown.
    sessions: Option<HashSet<String>>,
    peering_told: bool,
}

impl Detector {
    pub fn new(cfg: Config, attribution: Attribution) -> Self {
        Self {
            cfg,
            attribution,
            peers: BTreeMap::new(),
            router_zid: None,
            last_line_ms: None,
            input_closed: false,
            unparsed: VecDeque::new(),
            sessions: None,
            peering_told: false,
        }
    }

    /// Take one raw log line that arrived at `arrival_ms` (reader clock).
    pub fn ingest(&mut self, line: &str, arrival_ms: i64) {
        self.last_line_ms = Some(arrival_ms);
        match classify(line) {
            LineKind::Rejection(r) => self.record(r),
            LineKind::RouterZid(z) => self.router_zid = Some(z),
            LineKind::Unparsed => self.unparsed.push_back(arrival_ms),
            LineKind::Other => {}
        }
    }

    /// The Zenoh IDs that currently hold a session with our router (from the
    /// sessions file), or None when that is not known.
    pub fn set_sessions(&mut self, zids: Option<Vec<String>>) {
        self.sessions = zids.map(|v| v.iter().map(|z| normalize_zid(z)).collect());
    }

    /// The input ended; from now on the reading is unmeasured.
    pub fn mark_input_closed(&mut self) {
        self.input_closed = true;
    }

    fn record(&mut self, r: Rejection) {
        let at = r.router_unix_ns.div_euclid(1_000_000);
        let offset = r.offset_seconds();
        let key = normalize_zid(&r.zid);
        let t = self.peers.entry(key).or_insert_with(|| PeerTrack {
            zid: r.zid.clone(),
            events: VecDeque::new(),
            latest_offset: offset,
            limit_seconds: r.limit_seconds,
            first_seen_ms: at,
            last_seen_ms: at,
            told: false,
        });
        // Lines can arrive slightly out of order across router threads; keep
        // the deque sorted so the window cut stays a prefix.
        let idx = t.events.partition_point(|&(ts, _)| ts <= at);
        t.events.insert(idx, (at, offset));
        if at >= t.last_seen_ms {
            t.last_seen_ms = at;
            t.latest_offset = offset;
            t.limit_seconds = r.limit_seconds;
        }
        t.first_seen_ms = t.first_seen_ms.min(at);
    }

    fn observed_by(&self) -> String {
        self.cfg
            .observed_by
            .clone()
            .or_else(|| self.router_zid.clone())
            .unwrap_or_default()
    }

    /// Cut the windows at `now_ms`, grade, and build the reading.
    pub fn tick(&mut self, now_ms: i64, local: &LocalClock) -> Reading {
        let window_start = now_ms - self.cfg.window_ms;
        for t in self.peers.values_mut() {
            while t.events.front().is_some_and(|&(ts, _)| ts < window_start) {
                t.events.pop_front();
            }
        }
        while self.unparsed.front().is_some_and(|&ts| ts < window_start) {
            self.unparsed.pop_front();
        }

        let last_line_age_ms = self.last_line_ms.map_or(-1, |l| (now_ms - l).max(0));
        let unmeasured_reason = if self.input_closed {
            Some("the log input ended; nothing is being read".to_string())
        } else if self.last_line_ms.is_none() {
            Some("no router log line has been read yet".to_string())
        } else if last_line_age_ms > self.cfg.silence_after_ms {
            Some(format!(
                "no router log line for {} s",
                last_line_age_ms / 1000
            ))
        } else {
            None
        };

        // Clear peers clean for the whole clean period, but only while we can see.
        let mut cleared: Vec<PeerTrack> = Vec::new();
        if unmeasured_reason.is_none() {
            let clean_after = self.cfg.clean_after_ms;
            let gone: Vec<String> = self
                .peers
                .iter()
                .filter(|(_, t)| now_ms - t.last_seen_ms >= clean_after)
                .map(|(k, _)| k.clone())
                .collect();
            for k in gone {
                if let Some(t) = self.peers.remove(&k) {
                    cleared.push(t);
                }
            }
        }

        let active: Vec<&PeerTrack> = self
            .peers
            .values()
            .filter(|t| !t.events.is_empty())
            .collect();
        let active_count = active.len();
        let mut known: BTreeSet<String> = self.peers.keys().cloned().collect();
        known.extend(self.attribution.zids());
        // A core router with many sessions and two fast peers must blame the
        // two peers: every Zenoh ID holding a session is part of the population.
        if let Some(sessions) = &self.sessions {
            known.extend(sessions.iter().cloned());
        }
        let peers_known = (known.len() as u32).max(self.cfg.peer_population);
        let ahead = active.iter().filter(|t| t.latest_offset > 0.0).count() as u32;
        let behind = active.iter().filter(|t| t.latest_offset < 0.0).count() as u32;
        let same_direction = ahead.max(behind);
        let majority =
            self.cfg.peering.is_none() && peers_known >= 2 && same_direction * 2 > peers_known;

        let suspect_reason = if !local.is_synced() {
            Some(format!("our host clock is not grounded ({local})"))
        } else if majority {
            Some(format!(
                "{same_direction} of {peers_known} known peers show an offset in the same direction; our clock is the likelier fault"
            ))
        } else {
            None
        };

        let (verdict, color, reason) = if let Some(why) = &unmeasured_reason {
            ("unmeasured", "unmetered", why.clone())
        } else if let Some(why) = suspect_reason {
            ("local_clock_suspect", "amber", why)
        } else if active_count > 0 {
            (
                "peer_at_fault",
                "red",
                format!(
                    "{active_count} peer(s) had timestamps rejected in the last {} s",
                    self.cfg.window_ms / 1000
                ),
            )
        } else if !self.peers.is_empty() {
            (
                "recovering",
                "amber",
                format!(
                    "{} peer(s) rejected earlier, none in the last {} s; cleared after {} s clean",
                    self.peers.len(),
                    self.cfg.window_ms / 1000,
                    self.cfg.clean_after_ms / 1000
                ),
            )
        } else if !self.unparsed.is_empty() {
            (
                "clean",
                "amber",
                format!(
                    "{} uhlc line(s) in the window did not parse; the log shape may have changed",
                    self.unparsed.len()
                ),
            )
        } else {
            ("clean", "green", "no timestamp rejected".to_string())
        };

        let observed_by = self.observed_by();
        let window_ms = self.cfg.window_ms.max(1);
        let mut rows: Vec<PeerFaultV1> = Vec::new();
        let mut verdicts: Vec<(String, VerdictDoc)> = Vec::new();
        let observed_at = crate::rfc3339_ms(now_ms);
        let mut peering_at_fault: Vec<(String, f64, f64)> = Vec::new();

        for t in self.peers.values_mut() {
            let n = t.events.len() as u64;
            let rate = n as f64 * 60_000.0 / window_ms as f64;
            let (grade, row_color) = match verdict {
                "unmeasured" => ("unmeasured", "unmetered"),
                "local_clock_suspect" if n > 0 => ("local_clock_suspect", "idle"),
                "peer_at_fault" if n > 0 => ("peer_at_fault", "red"),
                _ => ("quiet", "amber"),
            };
            let (min, max) = if n > 0 {
                t.events
                    .iter()
                    .fold((f64::MAX, f64::MIN), |(lo, hi), &(_, o)| {
                        (lo.min(o), hi.max(o))
                    })
            } else {
                (t.latest_offset, t.latest_offset)
            };
            let attr = self.attribution.lookup(&t.zid);
            let direct_session = self
                .sessions
                .as_ref()
                .map(|s| s.contains(&normalize_zid(&t.zid)));
            rows.push(PeerFaultV1 {
                zid: t.zid.clone(),
                kind: PeerFaultKindV1::ClockAhead as i32,
                offset_seconds: t.latest_offset,
                min_offset_seconds: min,
                max_offset_seconds: max,
                rejections_per_minute: rate,
                rejections_in_window: n,
                first_seen_unix_ms: t.first_seen_ms,
                last_seen_unix_ms: t.last_seen_ms,
                observed_by: observed_by.clone(),
                attribution_name: attr
                    .and_then(|a| a.name.clone())
                    .or_else(|| self.cfg.peering.clone()),
                attribution_owner: attr.and_then(|a| a.owner.clone()),
                grade: grade.to_string(),
                color: row_color.to_string(),
                limit_seconds: t.limit_seconds,
                direct_session,
            });
            if grade == "peer_at_fault" {
                t.told = true;
                peering_at_fault.push((t.zid.clone(), max, t.limit_seconds));
                verdicts.push((
                    t.zid.clone(),
                    VerdictDoc::at_fault(
                        t.latest_offset,
                        t.limit_seconds,
                        rate,
                        &observed_at,
                        &observed_by,
                    ),
                ));
            }
        }
        for t in &cleared {
            let attr = self.attribution.lookup(&t.zid);
            let direct_session = self
                .sessions
                .as_ref()
                .map(|s| s.contains(&normalize_zid(&t.zid)));
            rows.push(PeerFaultV1 {
                zid: t.zid.clone(),
                kind: PeerFaultKindV1::ClockAhead as i32,
                offset_seconds: t.latest_offset,
                min_offset_seconds: t.latest_offset,
                max_offset_seconds: t.latest_offset,
                rejections_per_minute: 0.0,
                rejections_in_window: 0,
                first_seen_unix_ms: t.first_seen_ms,
                last_seen_unix_ms: t.last_seen_ms,
                observed_by: observed_by.clone(),
                attribution_name: attr
                    .and_then(|a| a.name.clone())
                    .or_else(|| self.cfg.peering.clone()),
                attribution_owner: attr.and_then(|a| a.owner.clone()),
                grade: "cleared".to_string(),
                color: "green".to_string(),
                limit_seconds: t.limit_seconds,
                direct_session,
            });
            if t.told {
                verdicts.push((
                    t.zid.clone(),
                    VerdictDoc::cleared(
                        t.limit_seconds,
                        self.cfg.clean_after_ms,
                        &observed_at,
                        &observed_by,
                    ),
                ));
            }
        }
        rows.sort_by(|a, b| a.zid.cmp(&b.zid));

        // Peering roll-up and its one verdict.
        let peering = self.cfg.peering.clone().unwrap_or_default();
        let peering_rate: f64 = rows
            .iter()
            .filter(|r| r.grade != "cleared")
            .map(|r| r.rejections_per_minute)
            .sum();
        let peering_max = rows
            .iter()
            .filter(|r| r.rejections_in_window > 0)
            .map(|r| r.max_offset_seconds)
            .fold(0.0_f64, f64::max);
        if !peering.is_empty() {
            if !peering_at_fault.is_empty() {
                self.peering_told = true;
                let limit = peering_at_fault
                    .iter()
                    .map(|p| p.2)
                    .fold(f64::MAX, f64::min);
                let zids = peering_at_fault.iter().map(|p| p.0.clone()).collect();
                verdicts.push((
                    format!("peering/{peering}"),
                    VerdictDoc::peering_at_fault(
                        &peering,
                        zids,
                        peering_max,
                        limit,
                        peering_rate,
                        &observed_at,
                        &observed_by,
                    ),
                ));
            } else if self.peering_told && self.peers.is_empty() && unmeasured_reason.is_none() {
                self.peering_told = false;
                let limit = cleared.first().map_or(0.5, |t| t.limit_seconds);
                let mut doc =
                    VerdictDoc::cleared(limit, self.cfg.clean_after_ms, &observed_at, &observed_by);
                doc.peering = Some(peering.clone());
                verdicts.push((format!("peering/{peering}"), doc));
            }
        }

        let envelope = EnvHealthV1 {
            domain: "peer".to_string(),
            scope: self.cfg.site.clone(),
            ts_unix_ms: now_ms,
            overall_color: color.to_string(),
            sample_count: peers_known as u64,
            window_secs: (self.cfg.window_ms / 1000).max(1) as u32,
            detail: Some(EnvDetail::Peer(PeerFaultDetailV1 {
                peers: rows,
                verdict: verdict.to_string(),
                verdict_reason: reason,
                local_clock: local.label().to_string(),
                observed_by,
                peers_known,
                last_line_age_ms,
                unparsed_lines_window: self.unparsed.len() as u64,
                peering,
                peering_rejections_per_minute: peering_rate,
                peering_max_offset_seconds: peering_max,
            })),
        };
        Reading { envelope, verdicts }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER_A: &str = "3c9a51e0d2b7f4a6c18e05d9b3a7f2e1";
    const PEER_B: &str = "a1";
    const T0: i64 = 1_794_575_921_000; // 2026-11-13T13:18:41Z

    fn line(zid: &str, router_ms: i64, offset_ms: i64) -> String {
        format!(
            "WARN uhlc: incoming timestamp from {zid} exceeding delta 500ms is rejected: {} vs. now: {}",
            crate::rfc3339_ms(router_ms + offset_ms),
            crate::rfc3339_ms(router_ms)
        )
    }

    fn det(cfg: Config) -> Detector {
        Detector::new(cfg, Attribution::default())
    }

    fn feed(d: &mut Detector, zid: &str, at: i64, offset_ms: i64) {
        d.ingest(&line(zid, at, offset_ms), at);
    }

    #[test]
    fn window_math() {
        let mut d = det(Config::default());
        for (i, off) in [17_000, 18_000, 16_000, 17_500].iter().enumerate() {
            feed(&mut d, PEER_A, T0 + i as i64 * 10_000, *off);
        }
        // At T0+60s the first event (T0) has left the 60 s window.
        let r = d.tick(T0 + 60_001, &LocalClock::Synced);
        let p = &r.detail().peers[0];
        assert_eq!(p.rejections_in_window, 3);
        assert_eq!(p.rejections_per_minute, 3.0);
        assert_eq!(p.offset_seconds, 17.5);
        assert_eq!(p.min_offset_seconds, 16.0);
        assert_eq!(p.max_offset_seconds, 18.0);
        assert_eq!(p.first_seen_unix_ms, T0);
        assert_eq!(p.last_seen_unix_ms, T0 + 30_000);
        assert_eq!(p.limit_seconds, 0.5);
        assert_eq!(p.kind, PeerFaultKindV1::ClockAhead as i32);
    }

    #[test]
    fn a_window_of_two_minutes_halves_the_rate() {
        let mut d = det(Config {
            window_ms: 120_000,
            ..Config::default()
        });
        for i in 0..4 {
            feed(&mut d, PEER_A, T0 + i * 1000, 17_000);
        }
        let r = d.tick(T0 + 5_000, &LocalClock::Synced);
        assert_eq!(r.detail().peers[0].rejections_per_minute, 2.0);
        assert_eq!(r.envelope.window_secs, 120);
    }

    #[test]
    fn one_peer_ahead_with_a_synced_clock_is_that_peers_fault() {
        let mut d = det(Config {
            peer_population: 4,
            ..Config::default()
        });
        feed(&mut d, PEER_A, T0, 17_700);
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        assert_eq!(r.detail().verdict, "peer_at_fault");
        assert_eq!(r.envelope.overall_color, "red");
        assert_eq!(r.detail().peers[0].grade, "peer_at_fault");
        assert_eq!(r.verdicts.len(), 1);
        assert_eq!(r.verdicts[0].0, PEER_A);
        assert_eq!(
            r.verdicts[0].1.sentence,
            "the router sees your clock 17.7 s ahead of its own; the limit is 0.5 s"
        );
    }

    #[test]
    fn an_unsynced_local_clock_blames_nobody() {
        let mut d = det(Config {
            peer_population: 4,
            ..Config::default()
        });
        feed(&mut d, PEER_A, T0, 17_700);
        let r = d.tick(T0 + 1_000, &LocalClock::Unsynced("test".into()));
        assert_eq!(r.detail().verdict, "local_clock_suspect");
        assert_eq!(r.envelope.overall_color, "amber");
        assert_eq!(r.detail().local_clock, "unsynced");
        assert_eq!(r.detail().peers[0].grade, "local_clock_suspect");
        assert!(r.verdicts.is_empty());
        // An unknown clock is not grounded either.
        let r = d.tick(T0 + 2_000, &LocalClock::Unknown("no check".into()));
        assert_eq!(r.detail().verdict, "local_clock_suspect");
        assert!(r.verdicts.is_empty());
    }

    #[test]
    fn an_unsynced_local_clock_with_no_rejections_is_still_suspect() {
        let mut d = det(Config::default());
        d.ingest("INFO something", T0);
        let r = d.tick(T0 + 1_000, &LocalClock::Unsynced("test".into()));
        assert_eq!(r.detail().verdict, "local_clock_suspect");
        assert_eq!(r.envelope.overall_color, "amber");
    }

    #[test]
    fn most_peers_ahead_means_our_clock_is_behind() {
        // Three of four known peers ahead: more than half.
        let mut d = det(Config {
            peer_population: 4,
            ..Config::default()
        });
        for z in ["a1", "b2", "c3"] {
            feed(&mut d, z, T0, 2_000);
        }
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        assert_eq!(r.detail().verdict, "local_clock_suspect");
        assert!(r.detail().verdict_reason.contains("3 of 4"));
        assert!(r.verdicts.is_empty());

        // Two of four is not more than half: the peers are at fault.
        let mut d = det(Config {
            peer_population: 4,
            ..Config::default()
        });
        for z in ["a1", "b2"] {
            feed(&mut d, z, T0, 2_000);
        }
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        assert_eq!(r.detail().verdict, "peer_at_fault");
        assert_eq!(r.verdicts.len(), 2);
    }

    #[test]
    fn the_population_includes_the_peers_file() {
        let attr = Attribution::from_json(
            r#"{"a1":{"name":"x"},"b2":{},"c3":{},"10.0.0.7":{"owner":"y"}}"#,
        )
        .unwrap();
        let mut d = Detector::new(Config::default(), attr);
        feed(&mut d, PEER_B, T0, 2_000);
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        assert_eq!(r.detail().peers_known, 3);
        assert_eq!(r.detail().verdict, "peer_at_fault");
        assert_eq!(r.detail().peers[0].attribution_name.as_deref(), Some("x"));
        assert_eq!(r.detail().peers[0].attribution_owner, None);
    }

    #[test]
    fn a_single_known_peer_is_not_a_majority_test() {
        // One peer, nothing else known: the local sync check is the guard.
        let mut d = det(Config::default());
        feed(&mut d, PEER_A, T0, 17_700);
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        assert_eq!(r.detail().peers_known, 1);
        assert_eq!(r.detail().verdict, "peer_at_fault");
    }

    #[test]
    fn quiet_then_cleared_once() {
        let mut d = det(Config::default());
        feed(&mut d, PEER_A, T0, 17_700);
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        assert_eq!(r.verdicts.len(), 1);

        // Out of the window but inside the clean period: recovering, still listed.
        d.ingest("INFO keepalive", T0 + 120_000);
        let r = d.tick(T0 + 120_000, &LocalClock::Synced);
        assert_eq!(r.detail().verdict, "recovering");
        assert_eq!(r.detail().peers[0].grade, "quiet");
        assert_eq!(r.detail().peers[0].rejections_per_minute, 0.0);
        assert_eq!(r.detail().peers[0].offset_seconds, 17.7);
        assert!(r.verdicts.is_empty());

        // Ten minutes clean: reported once as cleared, with a closing verdict.
        d.ingest("INFO keepalive", T0 + 600_000);
        let r = d.tick(T0 + 600_000, &LocalClock::Synced);
        assert_eq!(r.detail().peers.len(), 1);
        assert_eq!(r.detail().peers[0].grade, "cleared");
        assert_eq!(r.detail().peers[0].color, "green");
        assert_eq!(r.verdicts.len(), 1);
        assert_eq!(r.verdicts[0].1.state, "cleared");
        assert_eq!(r.envelope.overall_color, "green");

        // And then gone.
        let r = d.tick(T0 + 630_000, &LocalClock::Synced);
        assert!(r.detail().peers.is_empty());
        assert!(r.verdicts.is_empty());
        assert_eq!(r.detail().verdict, "clean");
    }

    #[test]
    fn a_peer_never_told_clears_without_a_verdict() {
        let mut d = det(Config::default());
        feed(&mut d, PEER_A, T0, 17_700);
        let _ = d.tick(T0 + 1_000, &LocalClock::Unsynced("test".into()));
        d.ingest("INFO keepalive", T0 + 600_000);
        let r = d.tick(T0 + 600_000, &LocalClock::Synced);
        assert_eq!(r.detail().peers[0].grade, "cleared");
        assert!(r.verdicts.is_empty());
    }

    #[test]
    fn a_fault_that_returns_before_clearing_keeps_its_first_seen() {
        let mut d = det(Config::default());
        feed(&mut d, PEER_A, T0, 17_700);
        let _ = d.tick(T0 + 1_000, &LocalClock::Synced);
        feed(&mut d, PEER_A, T0 + 300_000, 18_000);
        let r = d.tick(T0 + 300_500, &LocalClock::Synced);
        assert_eq!(r.detail().peers[0].first_seen_unix_ms, T0);
        assert_eq!(r.detail().peers[0].rejections_in_window, 1);
    }

    #[test]
    fn unmeasured_states_are_never_green_and_never_clear() {
        let mut d = det(Config::default());
        let r = d.tick(T0, &LocalClock::Synced);
        assert_eq!(r.detail().verdict, "unmeasured");
        assert_eq!(r.envelope.overall_color, "unmetered");
        assert_eq!(r.detail().last_line_age_ms, -1);

        feed(&mut d, PEER_A, T0, 17_700);
        // Silence past the period (15 min default) also passes the clean period:
        // the peer must NOT be cleared, because we cannot see.
        let r = d.tick(T0 + 900_001, &LocalClock::Synced);
        assert_eq!(r.detail().verdict, "unmeasured");
        assert_eq!(r.detail().peers[0].grade, "unmeasured");
        assert!(r.verdicts.is_empty());

        // Input closed.
        let mut d = det(Config::default());
        d.ingest("INFO x", T0);
        d.mark_input_closed();
        let r = d.tick(T0 + 1, &LocalClock::Synced);
        assert_eq!(r.detail().verdict, "unmeasured");
        assert!(r.detail().verdict_reason.contains("ended"));
    }

    #[test]
    fn clean_is_green_unless_lines_did_not_parse() {
        let mut d = det(Config::default());
        d.ingest("INFO hello", T0);
        let r = d.tick(T0 + 1, &LocalClock::Synced);
        assert_eq!(r.detail().verdict, "clean");
        assert_eq!(r.envelope.overall_color, "green");
        d.ingest("uhlc: incoming timestamp from ZZZ garbage", T0 + 2);
        let r = d.tick(T0 + 3, &LocalClock::Synced);
        assert_eq!(r.envelope.overall_color, "amber");
        assert_eq!(r.detail().unparsed_lines_window, 1);
        let r = d.tick(T0 + 61_000, &LocalClock::Synced);
        assert_eq!(r.detail().unparsed_lines_window, 0);
    }

    #[test]
    fn direct_session_is_unknown_without_a_sessions_file() {
        let mut d = det(Config::default());
        feed(&mut d, PEER_A, T0, 17_700);
        feed(&mut d, PEER_B, T0, 2_000);
        let r = d.tick(T0 + 1, &LocalClock::Synced);
        assert!(r.detail().peers.iter().all(|p| p.direct_session.is_none()));
        d.set_sessions(Some(vec!["00a1".to_string()]));
        let r = d.tick(T0 + 2, &LocalClock::Synced);
        let by: BTreeMap<_, _> = r
            .detail()
            .peers
            .iter()
            .map(|p| (p.zid.as_str(), p.direct_session))
            .collect();
        assert_eq!(by[PEER_A], Some(false), "stamped behind another router");
        assert_eq!(by[PEER_B], Some(true));
    }

    #[test]
    fn a_peering_owns_every_rejection_and_skips_the_majority_rule() {
        let cfg = Config {
            peering: Some("partner-one".into()),
            ..Config::default()
        };
        let mut d = det(cfg);
        // A whole partner network running fast: three stamping routers.
        for (z, off) in [("a1", 17_000), ("b2", 17_500), ("c3", 16_000)] {
            feed(&mut d, z, T0, off);
        }
        let r = d.tick(T0 + 1, &LocalClock::Synced);
        let det_ = r.detail();
        assert_eq!(det_.verdict, "peer_at_fault");
        assert_eq!(det_.peering, "partner-one");
        assert_eq!(det_.peering_rejections_per_minute, 3.0);
        assert_eq!(det_.peering_max_offset_seconds, 17.5);
        assert!(det_
            .peers
            .iter()
            .all(|p| p.attribution_name.as_deref() == Some("partner-one")));
        let (key, doc) = r
            .verdicts
            .iter()
            .find(|(k, _)| k.starts_with("peering/"))
            .unwrap();
        assert_eq!(key, "peering/partner-one");
        assert_eq!(doc.zids.len(), 3);
        assert!(doc.sentence.contains("17.5 s ahead"), "{}", doc.sentence);
        // Clean period passes: the peering is cleared once.
        d.ingest("INFO keepalive", T0 + 600_000);
        let r = d.tick(T0 + 600_000, &LocalClock::Synced);
        let (_, doc) = r
            .verdicts
            .iter()
            .find(|(k, _)| k.starts_with("peering/"))
            .unwrap();
        assert_eq!(doc.state, "cleared");
        assert_eq!(doc.peering.as_deref(), Some("partner-one"));
        let r = d.tick(T0 + 630_000, &LocalClock::Synced);
        assert!(r.verdicts.is_empty());
    }

    #[test]
    fn the_population_includes_the_sessions_file() {
        let sessions: Vec<String> = (1..=100).map(|i| format!("{i:x}0")).collect();
        let feed_n = |n: usize| {
            let mut d = det(Config::default());
            d.set_sessions(Some(sessions.clone()));
            for z in sessions.iter().take(n) {
                feed(&mut d, z, T0, 2_000);
            }
            d.tick(T0 + 1_000, &LocalClock::Synced)
        };
        // 2 of 100 ahead: the two peers are at fault.
        let r = feed_n(2);
        assert_eq!(r.detail().peers_known, 100);
        assert_eq!(r.detail().verdict, "peer_at_fault");
        assert_eq!(r.verdicts.len(), 2);
        // 60 of 100 ahead: our clock is the likelier fault.
        let r = feed_n(60);
        assert_eq!(r.detail().peers_known, 100);
        assert_eq!(r.detail().verdict, "local_clock_suspect");
        assert!(r.detail().verdict_reason.contains("60 of 100"));
        assert!(r.verdicts.is_empty());
        // Without the sessions file the same 2 fast peers are all it knows.
        let mut d = det(Config::default());
        for z in sessions.iter().take(2) {
            feed(&mut d, z, T0, 2_000);
        }
        assert_eq!(
            d.tick(T0 + 1_000, &LocalClock::Synced).detail().verdict,
            "local_clock_suspect"
        );
    }

    #[test]
    fn observed_by_from_the_router_or_the_operator() {
        let mut d = det(Config::default());
        d.ingest("INFO zenoh::net::runtime: Using ZID: 5be08d2f", T0);
        feed(&mut d, PEER_A, T0, 17_700);
        let r = d.tick(T0 + 1, &LocalClock::Synced);
        assert_eq!(r.detail().observed_by, "5be08d2f");
        assert_eq!(r.detail().peers[0].observed_by, "5be08d2f");
        let mut d = det(Config {
            observed_by: Some("77".into()),
            ..Config::default()
        });
        d.ingest("INFO zenoh::net::runtime: Using ZID: 5be08d2f", T0);
        let r = d.tick(T0 + 1, &LocalClock::Synced);
        assert_eq!(r.detail().observed_by, "77");
    }
}
