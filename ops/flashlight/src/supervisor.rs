// SPDX-License-Identifier: Apache-2.0
//
// Subscription supervisor — keeps the live zenoh subscription running
// and keeps its reported health honest.
//
// A background task that retries `zenoh::open` with backoff fixes the *initial*
// connect race, so a boot race against the router self-heals. It does not cover
// the mid-life case: when a router is recreated under a running flashlight, the
// router logs the peer's transport closed and never sees it again, while
// flashlight's side keeps an ESTABLISHED socket with data stuck unread in its
// receive queue. Ingest stops, and nothing notices.
//
// Nothing notices when every health signal is a proxy for "did the first
// connect succeed":
//
//   - an ingest task that only logs on the `Err` arm dies silently, because a
//     dead subscriber channel ends the loop via `Ok`;
//   - a connect task that `return`s after the first success never reconnects;
//   - `/healthz` and `/api/stats` that only ask whether the broadcast
//     `Sender` exists stay true, because it is set once at boot and never cleared.
//
// Then the surface serves HTTP 200, reports `subscription_active: true`,
// keeps isitup green, and streams an open, permanently silent SSE
// channel to every operator who asks for events, with no alert.
//
// So this has three parts, one per failure above: a supervisor loop that
// reconnects for the whole process lifetime, a liveness check that can
// see a session that is up-but-not-delivering, and clearing the
// subscription slot on loss so the existing health surfaces stop
// reporting a subscription that is gone.
//
// Scope, stated the way F-196 stated its own: this detects a lost
// TRANSPORT, not a refused SUBSCRIPTION. A router that keeps the session
// but declines to deliver — an ACL that does not grant this peer the key
// expression it asked for, say — still reports a router, so the
// supervisor will not trip and should not: reconnecting cannot fix an
// authorization decision, and a reconnect loop against one is a storm.
// That case is what `/api/stats.last_sample_ms_ago` is for. It is the
// honest split: this module answers "am I still connected", the
// timestamp answers "is anything arriving", and neither pretends to be
// the other.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use panoscope_core::topology::Topology;
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::store::SampleStore;
use crate::subscription::SubscriptionPlan;
use crate::zenoh_client::{self, SampleEvent, ZenohConfig};

/// How often the supervisor interrogates the session's transport state.
const ROUTER_POLL_INTERVAL: Duration = Duration::from_secs(10);

/// Consecutive router-less polls tolerated before the subscription is
/// declared lost. A brief transport flap should not churn the
/// subscription, so loss needs `ROUTER_MISS_LIMIT` polls in a row —
/// ~30 s at the interval above.
const ROUTER_MISS_LIMIT: u32 = 3;

/// Backoff bounds for the reconnect loop, matching the F-196 ladder.
const BACKOFF_START_SECS: u64 = 1;
const BACKOFF_CAP_SECS: u64 = 30;

/// Why a live subscription stopped being live. Carried into the log line
/// so the next person reading container logs learns which of the three
/// failure shapes they are looking at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionLost {
    /// An ingest task finished — its subscriber's channel closed.
    IngestEnded,
    /// The session reported itself closed.
    SessionClosed,
    /// The session is nominally open but has no transport to any router.
    /// This is the shape that went undetected: open session, live
    /// socket, no delivery.
    RouterGone { consecutive_misses: u32 },
}

impl SubscriptionLost {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IngestEnded => "ingest-ended",
            Self::SessionClosed => "session-closed",
            Self::RouterGone { .. } => "router-gone",
        }
    }
}

/// Liveness policy over a stream of transport observations.
///
/// Pure and synchronous on purpose: the decision of when a session
/// counts as lost is the part worth testing, and it should not need a
/// router to exercise. The supervisor feeds it observations; it owns no
/// I/O.
#[derive(Debug)]
pub struct RouterLiveness {
    consecutive_misses: u32,
    miss_limit: u32,
}

impl RouterLiveness {
    pub fn new(miss_limit: u32) -> Self {
        Self {
            consecutive_misses: 0,
            miss_limit: miss_limit.max(1),
        }
    }

    /// Fold one observation in. `Some(..)` means the subscription should
    /// be torn down and rebuilt; `None` means carry on.
    ///
    /// A poll that finds routers clears the miss streak, so a single
    /// dropped poll during a flap costs nothing.
    pub fn observe(
        &mut self,
        session_closed: bool,
        router_count: usize,
    ) -> Option<SubscriptionLost> {
        if session_closed {
            return Some(SubscriptionLost::SessionClosed);
        }
        if router_count > 0 {
            self.consecutive_misses = 0;
            return None;
        }
        self.consecutive_misses += 1;
        if self.consecutive_misses >= self.miss_limit {
            return Some(SubscriptionLost::RouterGone {
                consecutive_misses: self.consecutive_misses,
            });
        }
        None
    }
}

/// The subscription slot shared with request handlers. `None` means
/// there is no live subscription right now, which is what `/healthz` and
/// `/api/stats` read.
pub type SubscriptionSlot = Arc<RwLock<Option<Arc<broadcast::Sender<SampleEvent>>>>>;

/// Connect, publish the sender, watch until the subscription dies, clear
/// the sender, reconnect. Runs for the life of the process.
///
/// The slot is only ever `Some` while a subscription is actually
/// established, which is what makes the health surfaces honest — they
/// did not change shape, they just stopped being handed a stale `Some`.
pub async fn supervise(
    cfg: ZenohConfig,
    plan: SubscriptionPlan,
    store: Arc<SampleStore>,
    topology: Arc<RwLock<Topology>>,
    slot: SubscriptionSlot,
) {
    let mut backoff_secs = BACKOFF_START_SECS;
    loop {
        let live = match zenoh_client::run(&cfg, &plan, store.clone(), topology.clone()).await {
            Ok(live) => live,
            Err(e) => {
                warn!(
                    error = %e,
                    backoff_secs,
                    "zenoh connect failed — retrying (HTTP-only meanwhile)"
                );
                tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(BACKOFF_CAP_SECS);
                continue;
            }
        };

        info!("zenoh subscription established");
        set_slot(&slot, Some(Arc::new(live.sender())));
        backoff_secs = BACKOFF_START_SECS;

        let reason = watch(live, &slot).await;

        warn!(
            reason = reason.as_str(),
            detail = ?reason,
            "zenoh subscription lost — reconnecting"
        );
        tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
    }
}

/// Hold a live subscription until it stops being live, then tear it
/// down. Clearing the slot happens the moment loss is detected and
/// before the (awaited) shutdown, so no request is served a sender for a
/// subscription already known to be dead.
async fn watch(
    mut live: zenoh_client::LiveSubscription,
    slot: &SubscriptionSlot,
) -> SubscriptionLost {
    let session = live.session();
    let mut liveness = RouterLiveness::new(ROUTER_MISS_LIMIT);

    let reason = loop {
        tokio::select! {
            // An ingest task ending is itself proof of loss; no poll needed.
            _ = live.ingest_ended() => break SubscriptionLost::IngestEnded,
            _ = tokio::time::sleep(ROUTER_POLL_INTERVAL) => {
                let router_count = session.info().routers_zid().await.count();
                if let Some(lost) = liveness.observe(session.is_closed(), router_count) {
                    break lost;
                }
            }
        }
    };

    set_slot(slot, None);
    live.shutdown().await;
    reason
}

fn set_slot(slot: &SubscriptionSlot, value: Option<Arc<broadcast::Sender<SampleEvent>>>) {
    match slot.write() {
        Ok(mut guard) => *guard = value,
        // A poisoned slot means a handler panicked holding it. Say so
        // rather than carrying on with a subscription state nobody can
        // read or trust.
        Err(_) => warn!("subscription slot lock poisoned — health state is now unreliable"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_polls_never_report_loss() {
        let mut l = RouterLiveness::new(3);
        for _ in 0..10 {
            assert_eq!(l.observe(false, 1), None);
        }
    }

    #[test]
    fn a_closed_session_is_lost_on_the_first_poll() {
        let mut l = RouterLiveness::new(3);
        assert_eq!(l.observe(true, 1), Some(SubscriptionLost::SessionClosed));
    }

    #[test]
    fn router_loss_needs_the_full_miss_streak() {
        // The shape: session open, socket up, no router.
        let mut l = RouterLiveness::new(3);
        assert_eq!(l.observe(false, 0), None);
        assert_eq!(l.observe(false, 0), None);
        assert_eq!(
            l.observe(false, 0),
            Some(SubscriptionLost::RouterGone {
                consecutive_misses: 3
            })
        );
    }

    #[test]
    fn a_flap_shorter_than_the_streak_is_tolerated() {
        // Two misses then a good poll must not tear down a working
        // subscription — a false trip churns the session and is worse
        // than the bug being fixed.
        let mut l = RouterLiveness::new(3);
        assert_eq!(l.observe(false, 0), None);
        assert_eq!(l.observe(false, 0), None);
        assert_eq!(l.observe(false, 1), None);
        assert_eq!(l.observe(false, 0), None);
        assert_eq!(l.observe(false, 0), None);
        assert_eq!(l.observe(false, 1), None);
    }

    #[test]
    fn miss_limit_of_zero_is_clamped_to_one() {
        // Guards against a future config knob set to 0 turning every
        // single dropped poll into a reconnect storm.
        let mut l = RouterLiveness::new(0);
        assert_eq!(
            l.observe(false, 0),
            Some(SubscriptionLost::RouterGone {
                consecutive_misses: 1
            })
        );
    }

    #[test]
    fn loss_reasons_carry_distinct_labels() {
        assert_eq!(SubscriptionLost::IngestEnded.as_str(), "ingest-ended");
        assert_eq!(SubscriptionLost::SessionClosed.as_str(), "session-closed");
        assert_eq!(
            SubscriptionLost::RouterGone {
                consecutive_misses: 3
            }
            .as_str(),
            "router-gone"
        );
    }
}
