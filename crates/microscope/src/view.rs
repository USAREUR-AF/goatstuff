//! The entity view: spine + capability + merged timeline + per-source
//! status. One view-model — the JSON API, the future web page, and the
//! goat-cli lens all render this same shape (goat-top's one-view-model
//! discipline).

use serde::Serialize;

use crate::adapters;
use crate::config::Config;
use crate::event::{SourceStatus, TimelineEvent};
use crate::spine::{self, EntitySpine};

#[derive(Debug, Clone, Serialize)]
pub struct CapabilityPane {
    /// Ory groups claim → gated-surface admission (rendered from the claim,
    /// not re-derived; the matrix stays the PDP).
    pub ory_groups: Vec<String>,
    /// Zenoh write anchors (each = cert CN = ACL subject = namespace).
    pub write_anchors: Vec<String>,
    /// Peers currently enrolled under the entity's handle prefix.
    pub mesh_peers: Vec<String>,
    /// Whether Kratos answered. Empty-and-read renders "(none)"; empty-and-
    /// unread must NOT, or the page invents a false negative about a person.
    pub ory_resolved: bool,
    /// Whether the portal answered, same distinction for write anchors.
    pub anchors_resolved: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntityView {
    pub spine: EntitySpine,
    pub capability: CapabilityPane,
    /// Merged, time-ordered (newest first) across all sources.
    pub timeline: Vec<TimelineEvent>,
    /// One row per adapter — `ok`/`skipped`/`error`, always all of them.
    pub sources: Vec<SourceStatus>,
    /// Layers that refuse silently on this deployment (RCA honesty banner).
    pub silent_layers: Vec<&'static str>,
}

pub async fn build(cfg: &Config, selector: &str) -> EntityView {
    let spine = spine::resolve(cfg, selector).await;

    // CONCURRENTLY, not in sequence. Eight adapters each doing network I/O,
    // awaited one at a time, would make the page cost the SUM of every source's
    // latency even when no individual upstream is slow.
    //
    // Order independence is not an assumption here — the timeline is sorted
    // by timestamp immediately below, and each adapter returns its own
    // SourceStatus, so neither result depends on which finished first. The
    // sources list is re-sorted by name afterwards so the page does not
    // reshuffle its own diagnostics run to run based on network timing.
    let mut timeline: Vec<TimelineEvent> = Vec::new();
    let mut sources: Vec<SourceStatus> = Vec::new();
    let collected = futures::future::join_all(
        adapters::all()
            .into_iter()
            .map(|adapter| {
                let spine = &spine;
                async move { adapter.collect(cfg, spine).await }
            }),
    )
    .await;
    for out in collected {
        timeline.extend(out.events);
        sources.push(out.status);
    }
    sources.sort_by(|a, b| a.source.cmp(b.source));
    // Newest first. Every adapter normalizes through `timefmt` to ONE
    // fixed-width UTC form, so byte order IS chronological order — that is
    // the property this sort depends on, and it is enforced at ingest rather
    // than assumed here. Undated events (empty string) sort last, which is
    // where an event we cannot place in time belongs: visible, at the bottom,
    // labelled unknown rather than silently dated.
    timeline.sort_by(|a, b| b.ts.cmp(&a.ts));

    let capability = CapabilityPane {
        ory_resolved: spine.ory_resolved,
        anchors_resolved: spine.anchors_resolved,
        ory_groups: spine.ory_groups.clone(),
        write_anchors: spine.anchors.clone(),
        mesh_peers: spine.netbird_peers.clone(),
    };

    EntityView {
        capability,
        timeline,
        sources,
        // Constant for now — these two layers record no per-denial evidence
        // anywhere in the fleet. Becomes
        // deployment-derived when ADR 1022's denial capture lands (gap G2).
        silent_layers: vec!["zenoh-write", "netbird-data-plane"],
        spine,
    }
}
