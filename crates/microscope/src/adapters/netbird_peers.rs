//! NetBird peer state (design source S10): connected / last_seen / mesh IP
//! per peer joined by the handle-prefix naming CONVENTION — every event is
//! tagged accordingly (honesty extends to joins).

use async_trait::async_trait;

use panoscope_core::netbird::NetbirdClient;

use crate::adapters::{AdapterOutput, SourceAdapter};
use crate::config::Config;
use crate::event::{Attribution, TimelineEvent};
use crate::spine::EntitySpine;

pub struct NetbirdPeers;

const SOURCE: &str = "netbird-peers";

#[async_trait]
impl SourceAdapter for NetbirdPeers {
    fn name(&self) -> &'static str {
        SOURCE
    }

    async fn collect(&self, cfg: &Config, spine: &EntitySpine) -> AdapterOutput {
        let nb = NetbirdClient::new(cfg.netbird_url.clone(), cfg.netbird_token.clone());
        if !nb.is_enabled() {
            return AdapterOutput::skipped(SOURCE, "MICROSCOPE_NETBIRD_URL/TOKEN unset");
        }
        let subject: std::collections::BTreeSet<&str> = spine
            .netbird_peers
            .iter()
            // All three relations — see the note in graph.rs. Enrolled peers
            // were dropped here too.
            .chain(spine.netbird_peers_enrolled.iter())
            .chain(spine.netbird_peers_administered.iter())
            .map(|s| s.as_str())
            .collect();
        if subject.is_empty() {
            // Say WHY the join found nothing, not just that it did. This
            // adapter joins on the `peer-<handle>-<env>-<date>-<host>` naming
            // CONVENTION, which only holds for peers enrolled through
            // onboarding. Peers named any other way — an operator's own
            // laptop, a host enrolled directly with a setup key — are
            // invisible to it, and the operator using this surface is exactly
            // the person most likely to own such a peer.
            //
            // There is no stronger join available today: NetBird returns a
            // `user_id` field on every peer and it is EMPTY fleet-wide,
            // because peers enrol via setup keys rather than user login. The
            // honest fix is the custody edge (ADR 1096), which records which
            // person is custodian of which machine; until that is wired here,
            // this zero is a limit of the JOIN and must not read as a fact
            // about the entity.
            return AdapterOutput::ok(
                SOURCE,
                "no peer NAME carries this entity's handle prefix — this adapter joins by naming convention only, so peers named any other way (an operator's own laptop, a setup-key enrolment) are invisible to it. NetBird's per-peer user_id is empty fleet-wide, so no stronger join exists yet; ADR 1096 custody edges are the honest source. This says nothing about whether the entity HAS peers.",
                vec![],
            );
        }
        let peers = match nb.list_peers().await {
            Ok(p) => p,
            Err(e) => return AdapterOutput::error(SOURCE, e),
        };
        let mut events = Vec::new();
        for p in peers {
            if !subject.contains(p.name.as_str()) {
                continue;
            }
            // A CONNECTED peer contributes no timeline event, deliberately.
            //
            // Its `last_seen` is approximately now, refreshed continuously, so
            // emitting it as an event put a row reading "something happened
            // just now" permanently at the top of the timeline — re-dated on
            // every page load. That is liveness STATE wearing an event
            // costume, and it is the in-timeline version of the very
            // confusion this surface is supposed to resolve: a reader cannot
            // tell a log entry from a status line. Connected peers stay
            // visible as mesh peers on the spine and capability panes, which
            // is where point-in-time state belongs.
            //
            // A DISCONNECTED peer is different: `last_seen` is then a real
            // instant in the past, and "this peer was last seen at T" is a
            // genuine event worth ordering against console and portal actions.
            if p.connected {
                continue;
            }
            let groups: Vec<String> = p.group_names().iter().map(|s| s.to_string()).collect();
            events.push(TimelineEvent {
                ts: crate::timefmt::canonical_str(&p.last_seen),
                source: SOURCE,
                action: "peer.last_seen".into(),
                actor: p.name.clone(),
                target: None,
                summary: format!(
                    "peer {} last seen, now disconnected (mesh {}, {})",
                    p.name, p.ip, p.os
                ),
            kind: crate::event::EventKind::classify("peer.last_seen"),
            outcome: crate::event::Outcome::Unrecorded,
            direction: crate::event::Direction::Undirected,
            detail: None,
            // Grade by which join actually reached this peer. The audit join
            // is keyed on the Ory UUID, so it is stronger than a name match —
            // and a page that showed both under one grade would understate
            // what it knows about half its rows.
            attribution: if spine.netbird_peers.contains(&p.name) {
                Attribution::Convention
            } else {
                Attribution::Strong
            },
                raw: Some(serde_json::json!({
                    "id": p.id,
                    "name": p.name,
                    "hostname": p.hostname,
                    "ip": p.ip,
                    "connected": p.connected,
                    "last_seen": p.last_seen,
                    "os": p.os,
                    "groups": groups,
                })),
            });
        }
        // n is NOT the peer count and saying "GET /api/peers" invited exactly
        // that reading: `netbird status` shows 14 peers while this row read 3.
        // Both are right — this adapter emits a timeline event only for a
        // DISCONNECTED peer, because a connected one has no event to report
        // (see the note above). Say so, with both numbers.
        AdapterOutput::ok(
            SOURCE,
            &format!(
                "GET /api/peers — {} of this entity's {} peer(s) are disconnected and contribute a last-seen event; connected peers are STATE, not events, and appear on the spine instead",
                events.len(),
                subject.len()
            ),
            events,
        )
    }
}
