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
//
// The trust detector. A client that trusts a root the router no longer presents
// ends the TLS handshake; the router logs one fatal-alert line per ended
// handshake, with no client address and no Zenoh ID. Alerts are counted per
// name in the same window. Its own verdict (`trust_verdict`):
//
//   unmeasured           the clock reading is unmeasured (same rule), or the
//                        router may not log refused handshakes at all: the
//                        deployer did not declare `tls_alerts_logged` and no
//                        alert line has been seen since the reader started. A
//                        router build that logs the alert only at debug logs
//                        nothing at info, so silence is not proof of clean.
//   clean                no trust-refusal alert in the window.
//   candidates           trust-refusal alerts in the window AND declared peers
//                        (peers file) hold no session (sessions file). One
//                        amber row per absent declared peer, grade `candidate`,
//                        and no per-peer verdict document: a candidate is not
//                        a verdict.
//   unattributed         trust-refusal alerts in the window and no declared peer
//                        can be shown absent. Site-level counts only.
//
// Absence alone (no alerts) is not this fault and produces no row.
//
// Combining the two: unmeasured overrides all; otherwise the worse color wins
// (red > amber > green). When the trust reading is not clean and the clock
// verdict is `clean`, the trust verdict word replaces it; on any other clock
// verdict the clock word is kept, and the reason names both. A trust reading
// that is not measured turns a clean green into `trust_unmeasured`, unmetered:
// never green while half the reading cannot see.

use crate::clock::LocalClock;
use crate::parse::{classify, is_trust_refusal, is_zid, normalize_zid, LineKind, Rejection};
use crate::wire::{
    EnvDetail, EnvHealthV1, PeerFaultDetailV1, PeerFaultKindV1, PeerFaultV1, TlsAlertV1,
};
use crate::{address_host, Attribution, VerdictDoc};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

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
    /// The deployer declares that this router logs refused TLS handshakes at
    /// its log level (`received fatal alert`). When false, the trust reading is
    /// not measured until the first alert line is seen.
    pub tls_alerts_logged: bool,
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
            tls_alerts_logged: false,
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
    /// Source-address hosts from the sessions file (entries that are not
    /// Zenoh IDs). Empty = the writer gives no addresses, so a peers-file
    /// entry keyed by address cannot be judged present or absent.
    session_hosts: HashSet<String>,
    peering_told: bool,
    /// TLS alert name -> event times (ms), oldest first.
    alerts: BTreeMap<String, VecDeque<i64>>,
    /// Peers-file key -> the first tick at which it was judged absent.
    absent_since: HashMap<String, i64>,
    /// A TLS alert line has been read since the reader started: the router
    /// logs refused handshakes at its level.
    tls_alert_seen: bool,
}

/// One declared peer judged absent at this tick.
struct Absent {
    key: String,
    name: Option<String>,
    owner: Option<String>,
    since_ms: i64,
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
            session_hosts: HashSet::new(),
            peering_told: false,
            alerts: BTreeMap::new(),
            absent_since: HashMap::new(),
            tls_alert_seen: false,
        }
    }

    /// Take one raw log line that arrived at `arrival_ms` (reader clock).
    pub fn ingest(&mut self, line: &str, arrival_ms: i64) {
        self.last_line_ms = Some(arrival_ms);
        match classify(line) {
            LineKind::Rejection(r) => self.record(r),
            LineKind::RouterZid(z) => self.router_zid = Some(z),
            LineKind::TlsAlert { name, at_ms } => {
                self.tls_alert_seen = true;
                let at = at_ms.unwrap_or(arrival_ms);
                let q = self.alerts.entry(name).or_default();
                let idx = q.partition_point(|&ts| ts <= at);
                q.insert(idx, at);
            }
            LineKind::Unparsed => self.unparsed.push_back(arrival_ms),
            LineKind::Other => {}
        }
    }

    /// The Zenoh IDs that currently hold a session with our router (from the
    /// sessions file), or None when that is not known.
    /// Entries that are not Zenoh IDs are read as session source addresses.
    pub fn set_sessions(&mut self, zids: Option<Vec<String>>) {
        self.session_hosts.clear();
        self.sessions = zids.map(|v| {
            let mut set = HashSet::new();
            for e in v {
                if is_zid(&e.trim().to_ascii_lowercase()) {
                    set.insert(normalize_zid(&e));
                } else {
                    self.session_hosts.insert(address_host(&e));
                }
            }
            set
        });
    }

    /// Judge each declared peer present, absent or unknown at `now_ms`, keep
    /// the absent-since times, and return the absent ones and how many were
    /// judged at all.
    fn judge_absence(&mut self, now_ms: i64) -> (Vec<Absent>, usize) {
        let Some(sessions) = &self.sessions else {
            self.absent_since.clear();
            return (Vec::new(), 0);
        };
        let mut judged = 0;
        let mut keep: HashMap<String, i64> = HashMap::new();
        let mut absent = Vec::new();
        for (key, owner) in self.attribution.entries() {
            let present = if is_zid(key) {
                Some(sessions.contains(key))
            } else if self.session_hosts.is_empty() {
                None
            } else {
                Some(self.session_hosts.contains(&address_host(key)))
            };
            match present {
                None | Some(true) => {
                    judged += usize::from(present.is_some());
                }
                Some(false) => {
                    judged += 1;
                    let since = *self.absent_since.get(key).unwrap_or(&now_ms);
                    keep.insert(key.clone(), since);
                    absent.push(Absent {
                        key: key.clone(),
                        name: owner.name.clone(),
                        owner: owner.owner.clone(),
                        since_ms: since,
                    });
                }
            }
        }
        self.absent_since = keep;
        (absent, judged)
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
        for q in self.alerts.values_mut() {
            while q.front().is_some_and(|&ts| ts < window_start) {
                q.pop_front();
            }
        }
        self.alerts.retain(|_, q| !q.is_empty());

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
        let has_sessions = self.sessions.is_some();
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

        let (clock_verdict, clock_color, clock_reason) = if let Some(why) = &unmeasured_reason {
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
                    "{} uhlc or TLS alert line(s) in the window did not parse; the log shape may have changed",
                    self.unparsed.len()
                ),
            )
        } else {
            ("clean", "green", "no timestamp rejected".to_string())
        };

        // The trust detector.
        let window_ms = self.cfg.window_ms.max(1);
        let tls_alerts: Vec<TlsAlertV1> = self
            .alerts
            .iter()
            .map(|(name, q)| TlsAlertV1 {
                name: name.clone(),
                count_in_window: q.len() as u64,
                per_minute: q.len() as f64 * 60_000.0 / window_ms as f64,
                trust_refusal: is_trust_refusal(name),
                last_seen_unix_ms: q.back().copied().unwrap_or_default(),
            })
            .collect();
        let refusals: Vec<&TlsAlertV1> = tls_alerts.iter().filter(|a| a.trust_refusal).collect();
        let refusal_count: u64 = refusals.iter().map(|a| a.count_in_window).sum();
        let peers_file_empty = self.attribution.is_empty();
        let window_s = self.cfg.window_ms / 1000;
        let (absent, judged) = self.judge_absence(now_ms);
        let declared_peers_absent = (judged > 0).then_some(absent.len() as u32);
        let trust_blind = !self.cfg.tls_alerts_logged && !self.tls_alert_seen;
        let trust_verdict = if unmeasured_reason.is_some() || trust_blind {
            "unmeasured"
        } else if refusal_count == 0 {
            "clean"
        } else if !absent.is_empty() {
            "candidates"
        } else {
            "unattributed"
        };
        let alert_summary = || {
            let names: Vec<String> = refusals
                .iter()
                .map(|a| format!("{} x{}", a.name, a.count_in_window))
                .collect();
            format!(
                "{refusal_count} TLS handshake(s) ended in the last {window_s} s with a trust-refusal alert ({})",
                names.join(", ")
            )
        };
        let trust_reason = match trust_verdict {
            "candidates" => Some(format!(
                "{}; {} declared peer(s) hold no session and are candidates, not confirmed: the router's log cannot name the refusing client",
                alert_summary(),
                absent.len()
            )),
            "unattributed" => {
                let why = if peers_file_empty {
                    "no peers file declares the expected peers"
                } else if !has_sessions {
                    "no sessions file says who holds a session"
                } else if judged == 0 {
                    "no declared peer can be judged (address entries need source addresses in the sessions file)"
                } else {
                    "every declared peer holds a session"
                };
                Some(format!(
                    "{}: a client is refusing the router's certificate and cannot be named from the router's log; {why}",
                    alert_summary()
                ))
            }
            "unmeasured" if unmeasured_reason.is_none() => Some(
                "trust refusal is not measured: no TLS fatal-alert line has been read and the router is not declared to log one (PEER_HEALTH_TLS_ALERTS_LOGGED); a router that logs refused handshakes only at debug logs nothing at info".to_string(),
            ),
            _ => None,
        };

        // Combine: unmeasured overrides; the worse color wins; a clean clock
        // verdict gives way to the trust verdict word; the reason names both.
        let (verdict, color, reason) = match &trust_reason {
            Some(tr) if clock_verdict != "unmeasured" => {
                let (trust_word, color) = match trust_verdict {
                    "candidates" => ("trust_refused_candidates", "amber"),
                    "unattributed" => ("trust_refused_unattributed", "amber"),
                    // Not measured: no worse than what the clock says, never green.
                    _ => ("trust_unmeasured", "unmetered"),
                };
                let color = match (clock_color, color) {
                    ("red", _) => "red",
                    ("amber", _) => "amber",
                    (_, c) => c,
                };
                if clock_verdict == "clean" {
                    let reason = if clock_color == "green" {
                        tr.clone()
                    } else {
                        format!("{tr}; also: {clock_reason}")
                    };
                    (trust_word, color, reason)
                } else {
                    (clock_verdict, color, format!("{clock_reason}; also: {tr}"))
                }
            }
            _ => (clock_verdict, clock_color, clock_reason),
        };

        let observed_by = self.observed_by();
        let mut rows: Vec<PeerFaultV1> = Vec::new();
        let mut verdicts: Vec<(String, VerdictDoc)> = Vec::new();
        let observed_at = crate::rfc3339_ms(now_ms);
        let mut peering_at_fault: Vec<(String, f64, f64)> = Vec::new();

        for t in self.peers.values_mut() {
            let n = t.events.len() as u64;
            let rate = n as f64 * 60_000.0 / window_ms as f64;
            let (grade, row_color) = match clock_verdict {
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
                source_address: None,
                absent_since_unix_ms: None,
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
                source_address: None,
                absent_since_unix_ms: None,
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
        // Candidate rows: one per absent declared peer while trust-refusal
        // alerts arrive. No verdict document: a candidate is not a verdict.
        if trust_verdict == "candidates" {
            for a in &absent {
                let (zid, source_address) = if is_zid(&a.key) {
                    (a.key.clone(), None)
                } else {
                    (String::new(), Some(a.key.clone()))
                };
                rows.push(PeerFaultV1 {
                    zid,
                    kind: PeerFaultKindV1::TrustRootRefusedCandidate as i32,
                    first_seen_unix_ms: a.since_ms,
                    last_seen_unix_ms: now_ms,
                    observed_by: observed_by.clone(),
                    attribution_name: a.name.clone().or_else(|| self.cfg.peering.clone()),
                    attribution_owner: a.owner.clone(),
                    grade: "candidate".to_string(),
                    color: "amber".to_string(),
                    direct_session: Some(false),
                    source_address,
                    absent_since_unix_ms: Some(a.since_ms),
                    ..PeerFaultV1::default()
                });
            }
        }
        rows.sort_by(|a, b| {
            (&a.zid, &a.source_address, a.kind).cmp(&(&b.zid, &b.source_address, b.kind))
        });

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
                tls_alerts,
                trust_verdict: trust_verdict.to_string(),
                declared_peers_absent,
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

    /// The router is declared to log refused handshakes: the trust reading is
    /// measured from the first line, so a quiet log can grade green.
    fn logged() -> Config {
        Config {
            tls_alerts_logged: true,
            ..Config::default()
        }
    }

    #[test]
    fn quiet_then_cleared_once() {
        let mut d = det(logged());
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
        let mut d = det(logged());
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

    // ---- trust detector -------------------------------------------------

    const ABSENT_ZID: &str = "7e41c0d9";

    fn alert_line(name: &str, at_ms: i64) -> String {
        format!(
            "{}  WARN acc-0 ThreadId(05) zenoh_link_tls::unicast: received fatal alert: {name}. Hint: increase the system open file limit.",
            crate::rfc3339_ms(at_ms)
        )
    }

    fn alert(d: &mut Detector, name: &str, at: i64) {
        d.ingest(&alert_line(name, at), at);
    }

    fn declared() -> Attribution {
        Attribution::from_json(&format!(
            r#"{{"{ABSENT_ZID}":{{"name":"lab-router-d","owner":"example partner"}},"b2":{{"name":"lab-router-e"}},"10.0.0.7":{{"name":"lab-router-c"}}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn alerts_are_counted_per_name_in_the_window() {
        let mut d = det(Config::default());
        for i in 0..4 {
            alert(&mut d, "UnknownCA", T0 + i * 10_000);
        }
        alert(&mut d, "CertificateExpired", T0);
        let r = d.tick(T0 + 60_001, &LocalClock::Synced);
        let a = &r.detail().tls_alerts;
        assert_eq!(a.len(), 1, "CertificateExpired left the window: {a:?}");
        assert_eq!(a[0].name, "UnknownCA");
        assert_eq!(a[0].count_in_window, 3);
        assert_eq!(a[0].per_minute, 3.0);
        assert!(a[0].trust_refusal);
        assert_eq!(a[0].last_seen_unix_ms, T0 + 30_000);

        // A two-minute window halves the rate; names sort.
        let mut d = det(Config {
            window_ms: 120_000,
            ..Config::default()
        });
        for i in 0..4 {
            alert(&mut d, "UnknownCA", T0 + i * 1000);
        }
        alert(&mut d, "BadCertificate", T0);
        alert(&mut d, "CertificateExpired", T0);
        let r = d.tick(T0 + 5_000, &LocalClock::Synced);
        let a = &r.detail().tls_alerts;
        let names: Vec<_> = a.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["BadCertificate", "CertificateExpired", "UnknownCA"]);
        assert_eq!(a[2].per_minute, 2.0);
        assert!(!a[1].trust_refusal);
    }

    #[test]
    fn an_alert_line_without_a_time_lands_at_arrival() {
        let mut d = det(Config::default());
        d.ingest(
            "WARN zenoh_link_tls::unicast: received fatal alert: UnknownCA",
            T0 + 5,
        );
        let r = d.tick(T0 + 10, &LocalClock::Synced);
        assert_eq!(r.detail().tls_alerts[0].last_seen_unix_ms, T0 + 5);
        // A broken alert line is unparsed: never green.
        d.ingest(
            "WARN zenoh_link_tls::unicast: received fatal alert: ???",
            T0 + 20,
        );
        let r = d.tick(T0 + 30, &LocalClock::Synced);
        assert_eq!(r.detail().unparsed_lines_window, 1);
        assert_eq!(r.envelope.overall_color, "amber");
    }

    #[test]
    fn alerts_and_absent_declared_peers_give_candidates_not_verdicts() {
        let mut d = Detector::new(Config::default(), declared());
        // b2 holds a session; the writer gives addresses, and 10.0.0.7 is not among them.
        d.set_sessions(Some(vec!["b2".into(), "10.0.0.9:7447".into()]));
        alert(&mut d, "UnknownCA", T0);
        alert(&mut d, "UnknownCA", T0 + 1_000);
        let r = d.tick(T0 + 2_000, &LocalClock::Synced);
        let det_ = r.detail();
        assert_eq!(det_.verdict, "trust_refused_candidates");
        assert_eq!(det_.trust_verdict, "candidates");
        assert_eq!(r.envelope.overall_color, "amber");
        assert_eq!(det_.declared_peers_absent, Some(2));
        assert!(
            det_.verdict_reason.contains("candidates, not confirmed"),
            "{}",
            det_.verdict_reason
        );
        assert!(r.verdicts.is_empty(), "a candidate is not a verdict");
        assert_eq!(det_.peers.len(), 2);
        for p in &det_.peers {
            assert_eq!(p.kind, PeerFaultKindV1::TrustRootRefusedCandidate as i32);
            assert_eq!(p.grade, "candidate");
            assert_eq!(p.color, "amber");
            assert_eq!(p.direct_session, Some(false));
            assert_eq!(p.absent_since_unix_ms, Some(T0 + 2_000));
        }
        // Sorted: the address entry (zid "") first.
        let by_addr = &det_.peers[0];
        assert_eq!(by_addr.zid, "");
        assert_eq!(by_addr.source_address.as_deref(), Some("10.0.0.7"));
        assert_eq!(by_addr.attribution_name.as_deref(), Some("lab-router-c"));
        let by_zid = &det_.peers[1];
        assert_eq!(by_zid.zid, ABSENT_ZID);
        assert_eq!(by_zid.source_address, None);
        assert_eq!(by_zid.attribution_owner.as_deref(), Some("example partner"));

        // Absent-since holds across ticks.
        alert(&mut d, "BadCertificate", T0 + 30_000);
        let r = d.tick(T0 + 31_000, &LocalClock::Synced);
        assert!(r
            .detail()
            .peers
            .iter()
            .all(|p| p.absent_since_unix_ms == Some(T0 + 2_000)));
    }

    #[test]
    fn an_address_entry_is_not_judged_without_addresses_in_the_sessions_file() {
        let mut d = Detector::new(Config::default(), declared());
        d.set_sessions(Some(vec!["b2".into()]));
        alert(&mut d, "UnknownCA", T0);
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        assert_eq!(r.detail().declared_peers_absent, Some(1));
        assert_eq!(r.detail().peers.len(), 1);
        assert_eq!(r.detail().peers[0].zid, ABSENT_ZID);
    }

    #[test]
    fn alerts_with_no_peers_file_are_unattributed() {
        let mut d = det(Config::default());
        d.set_sessions(Some(vec!["b2".into()]));
        alert(&mut d, "UnknownCA", T0);
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        let det_ = r.detail();
        assert_eq!(det_.verdict, "trust_refused_unattributed");
        assert_eq!(det_.trust_verdict, "unattributed");
        assert_eq!(r.envelope.overall_color, "amber");
        assert!(det_.peers.is_empty());
        assert!(r.verdicts.is_empty());
        assert_eq!(det_.declared_peers_absent, None);
        assert_eq!(det_.tls_alerts.len(), 1);
        assert!(
            det_.verdict_reason.contains(
                "a client is refusing the router's certificate and cannot be named from the router's log"
            ),
            "{}",
            det_.verdict_reason
        );
        assert!(det_.verdict_reason.contains("no peers file"));
    }

    #[test]
    fn alerts_with_every_declared_peer_present_are_unattributed() {
        let attr = Attribution::from_json(r#"{"b2":{},"c3":{}}"#).unwrap();
        let mut d = Detector::new(Config::default(), attr);
        d.set_sessions(Some(vec!["b2".into(), "c3".into(), "d4".into()]));
        alert(&mut d, "CertificateUnknown", T0);
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        assert_eq!(r.detail().trust_verdict, "unattributed");
        assert_eq!(r.detail().declared_peers_absent, Some(0));
        assert!(r.detail().peers.is_empty());
        assert!(r
            .detail()
            .verdict_reason
            .contains("every declared peer holds a session"));

        // A peers file but no sessions file: absence cannot be judged.
        let mut d = Detector::new(Config::default(), declared());
        alert(&mut d, "UnknownCA", T0);
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        assert_eq!(r.detail().trust_verdict, "unattributed");
        assert_eq!(r.detail().declared_peers_absent, None);
        assert!(r.detail().peers.is_empty());
        assert!(r.detail().verdict_reason.contains("no sessions file"));
    }

    #[test]
    fn absence_alone_is_not_this_fault() {
        let mut d = Detector::new(logged(), declared());
        d.set_sessions(Some(vec!["b2".into()]));
        d.ingest("INFO keepalive", T0);
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        assert_eq!(r.detail().verdict, "clean");
        assert_eq!(r.detail().trust_verdict, "clean");
        assert_eq!(r.envelope.overall_color, "green");
        assert!(r.detail().peers.is_empty());
        assert_eq!(r.detail().declared_peers_absent, Some(1));

        // An alert outside the trust-refusal set is counted and grades nothing.
        alert(&mut d, "CertificateExpired", T0 + 2_000);
        let r = d.tick(T0 + 3_000, &LocalClock::Synced);
        assert_eq!(r.detail().trust_verdict, "clean");
        assert_eq!(r.envelope.overall_color, "green");
        assert!(r.detail().peers.is_empty());
        assert_eq!(r.detail().tls_alerts.len(), 1);
        assert!(!r.detail().tls_alerts[0].trust_refusal);

        // Refusals that left the window: back to clean, no rows.
        alert(&mut d, "UnknownCA", T0 + 4_000);
        assert_eq!(
            d.tick(T0 + 5_000, &LocalClock::Synced)
                .detail()
                .trust_verdict,
            "candidates"
        );
        d.ingest("INFO keepalive", T0 + 70_000);
        let r = d.tick(T0 + 70_000, &LocalClock::Synced);
        assert_eq!(r.detail().trust_verdict, "clean");
        assert!(r.detail().peers.is_empty());
    }

    #[test]
    fn absent_since_resets_when_the_peer_returns_or_sessions_are_unknown() {
        let mut d = Detector::new(Config::default(), declared());
        d.set_sessions(Some(vec!["b2".into()]));
        alert(&mut d, "UnknownCA", T0);
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        assert_eq!(r.detail().peers[0].absent_since_unix_ms, Some(T0 + 1_000));
        // It holds a session for one tick.
        d.set_sessions(Some(vec!["b2".into(), ABSENT_ZID.into()]));
        let r = d.tick(T0 + 2_000, &LocalClock::Synced);
        assert!(r.detail().peers.is_empty());
        d.set_sessions(Some(vec!["b2".into()]));
        let r = d.tick(T0 + 3_000, &LocalClock::Synced);
        assert_eq!(r.detail().peers[0].absent_since_unix_ms, Some(T0 + 3_000));
        // The sessions file went unreadable: cannot judge, then starts over.
        d.set_sessions(None);
        let r = d.tick(T0 + 4_000, &LocalClock::Synced);
        assert_eq!(r.detail().trust_verdict, "unattributed");
        d.set_sessions(Some(vec!["b2".into()]));
        let r = d.tick(T0 + 5_000, &LocalClock::Synced);
        assert_eq!(r.detail().peers[0].absent_since_unix_ms, Some(T0 + 5_000));
    }

    #[test]
    fn a_clock_fault_and_candidates_read_red_with_both_reasons() {
        let mut d = Detector::new(
            Config {
                peer_population: 4,
                ..Config::default()
            },
            declared(),
        );
        d.set_sessions(Some(vec!["b2".into(), PEER_A.into()]));
        feed(&mut d, PEER_A, T0, 17_700);
        alert(&mut d, "UnknownCA", T0);
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        let det_ = r.detail();
        assert_eq!(det_.verdict, "peer_at_fault");
        assert_eq!(r.envelope.overall_color, "red");
        assert_eq!(det_.trust_verdict, "candidates");
        assert!(det_.verdict_reason.contains("timestamps rejected"));
        assert!(det_.verdict_reason.contains("; also: 1 TLS handshake(s)"));
        let kinds: BTreeSet<i32> = det_.peers.iter().map(|p| p.kind).collect();
        assert_eq!(kinds.len(), 2);
        // Only the clock fault gets a verdict document.
        assert_eq!(r.verdicts.len(), 1);
        assert_eq!(r.verdicts[0].0, PEER_A);
    }

    #[test]
    fn the_clock_verdict_word_is_kept_on_a_tie() {
        // Our clock suspect (amber) and unattributed refusals (amber).
        let mut d = det(Config::default());
        alert(&mut d, "UnknownCA", T0);
        let r = d.tick(T0 + 1_000, &LocalClock::Unsynced("test".into()));
        assert_eq!(r.detail().verdict, "local_clock_suspect");
        assert_eq!(r.envelope.overall_color, "amber");
        assert_eq!(r.detail().trust_verdict, "unattributed");
        assert!(r.detail().verdict_reason.contains("; also:"));

        // Recovering (amber) and refusals: still recovering.
        let mut d = det(Config::default());
        feed(&mut d, PEER_A, T0, 17_700);
        let _ = d.tick(T0 + 1_000, &LocalClock::Synced);
        alert(&mut d, "UnknownCA", T0 + 120_000);
        let r = d.tick(T0 + 120_000, &LocalClock::Synced);
        assert_eq!(r.detail().verdict, "recovering");
        assert_eq!(r.envelope.overall_color, "amber");
        assert_eq!(r.detail().trust_verdict, "unattributed");
    }

    #[test]
    fn a_clean_clock_gives_way_to_the_trust_verdict() {
        // Clean but amber from an unparsed uhlc line: the trust word, both reasons.
        let mut d = det(Config::default());
        d.ingest("uhlc: incoming timestamp from ZZZ garbage", T0);
        alert(&mut d, "UnknownCA", T0);
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        assert_eq!(r.detail().verdict, "trust_refused_unattributed");
        assert_eq!(r.envelope.overall_color, "amber");
        assert!(r.detail().verdict_reason.contains("did not parse"));
    }

    #[test]
    fn unmeasured_overrides_the_trust_reading() {
        let mut d = Detector::new(Config::default(), declared());
        d.set_sessions(Some(vec!["b2".into()]));
        alert(&mut d, "UnknownCA", T0);
        d.mark_input_closed();
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        assert_eq!(r.detail().verdict, "unmeasured");
        assert_eq!(r.envelope.overall_color, "unmetered");
        assert_eq!(r.detail().trust_verdict, "unmeasured");
        assert!(r.detail().peers.is_empty(), "no candidates while blind");
        // The counts that were read are still reported.
        assert_eq!(r.detail().tls_alerts.len(), 1);
    }

    #[test]
    fn a_router_not_known_to_log_alerts_is_not_measured_for_trust() {
        // Not declared, no alert line yet: never clean, never green.
        let mut d = Detector::new(Config::default(), declared());
        d.set_sessions(Some(vec!["b2".into()]));
        d.ingest("INFO keepalive", T0);
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        assert_eq!(r.detail().trust_verdict, "unmeasured");
        assert_eq!(r.detail().verdict, "trust_unmeasured");
        assert_eq!(r.envelope.overall_color, "unmetered");
        assert!(r
            .detail()
            .verdict_reason
            .contains("PEER_HEALTH_TLS_ALERTS_LOGGED"));
        assert!(r.detail().peers.is_empty());

        // A clock fault still reads red, the trust part named in the reason.
        feed(&mut d, PEER_A, T0 + 2_000, 17_700);
        let r = d.tick(T0 + 3_000, &LocalClock::Synced);
        assert_eq!(r.detail().verdict, "peer_at_fault");
        assert_eq!(r.envelope.overall_color, "red");
        assert!(r
            .detail()
            .verdict_reason
            .contains("; also: trust refusal is not measured"));

        // One alert line proves the router logs them, from then on.
        let mut d = det(Config::default());
        alert(&mut d, "CertificateExpired", T0);
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        assert_eq!(r.detail().trust_verdict, "clean");
        assert_eq!(r.envelope.overall_color, "green");
        d.ingest("INFO keepalive", T0 + 120_000);
        let r = d.tick(T0 + 120_000, &LocalClock::Synced);
        assert!(r.detail().tls_alerts.is_empty());
        assert_eq!(r.detail().trust_verdict, "clean", "seen once is enough");

        // Declared by the deployer: measured with no line at all.
        let mut d = det(logged());
        d.ingest("INFO keepalive", T0);
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        assert_eq!(r.detail().trust_verdict, "clean");
        assert_eq!(r.envelope.overall_color, "green");
    }

    #[test]
    fn the_debug_shape_counts_and_accepted_lines_do_not() {
        let mut d = det(Config::default());
        d.ingest(
            "2026-11-13T13:18:41.000000Z DEBUG acc-0 ThreadId(05) zenoh_link_tls::unicast: Accepted TLS connection on 10.0.0.2:7447: 10.0.0.3:41234. Common Name: some-client.",
            T0,
        );
        let r = d.tick(T0 + 1, &LocalClock::Synced);
        assert_eq!(r.detail().unparsed_lines_window, 0);
        assert_eq!(
            r.detail().trust_verdict,
            "unmeasured",
            "an accept is not an alert"
        );
        d.ingest(
            "2026-11-13T13:18:41.500000Z DEBUG acc-0 ThreadId(05) zenoh_link_tls::unicast: TLS handshake failed on accept: received fatal alert: UnknownCA",
            T0 + 500,
        );
        let r = d.tick(T0 + 1_000, &LocalClock::Synced);
        assert_eq!(r.detail().tls_alerts[0].name, "UnknownCA");
        assert_eq!(r.detail().tls_alerts[0].last_seen_unix_ms, T0 + 500);
        assert_eq!(r.detail().trust_verdict, "unattributed");
    }
}
