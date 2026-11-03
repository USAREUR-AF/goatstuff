# panoscope history lens — operator-selectable time window

**Status:** design-of-record, building. **Surface:** panoscope. **Companion:** `panoscope.md`.

## Problem

panoscope's COP shows publishers that are currently or very recently active. When a publisher goes quiet and the container later restarts, it can vanish — the operator has no control over how far back the view reaches. Three concrete facts (all verified in code):

1. **Replay stamps wall-clock, not event time.** `minio_replay::apply_one` calls `apply_audit_event(..., now_ms)` with the replay wall-clock (`snapshot_view`/`minio_replay.rs:272`). So every peer seeded by the boot replay gets `last_seen_ms ≈ boot time` — they all look "just seen." (This is why a 48h replay still rendered every node as <1h old.)
2. **Topology never prunes.** `panoscope_core::topology::Topology.peers` is a `HashMap` with no eviction; `upsert_peer` only ever sets `last_seen_ms = now`. Peers accumulate for the life of the process; live ones keep refreshing, quiet ones keep their stamp.
3. **The only time-horizon knob is the boot-replay window** (`panoscope_minio_window_hours`, a group_var). It runs once at container start, and at depth it is heavy: a 48h replay read 42,505 objects / 6M events / 125s and churned the force layout during the ~2-min boot (the overlap the operator saw). Bumping it 6h→48h did **not** surface quiet publishers (replay stamps now, so they're not distinguishable by age) and made the boot heavy — pure cost.

So: no operator-selectable horizon; quiet-but-recent publishers can't be brought into view honestly; the one knob is boot-only, heavy, and stamps away the age signal it would need.

## What we build

A **history lens**: the operator picks a time window, and the COP shows everything active within it — live peers bright, quiet-but-within-window peers **faded + age-labeled**, nothing older.

1. **Stamp real last-published time.** Replay applies each event with the event's own `ts` (the `AuditEvent.ts` already carried), not the replay wall-clock. Live tail keeps stamping `now` (correct — a live sample *is* now). Result: every peer's `last_seen_ms` reflects when it truly last published, so age is real and windowing is meaningful. *(This is the load-bearing fix — without it the selector can't distinguish live from historical.)*
2. **Windowed snapshot.** `GET /api/snapshot?window_hours=N` filters rendered nodes to `now - last_seen_ms ≤ N·3600s` (live peers always included; their last_seen is fresh). `N` omitted ⇒ a default (the live-ish window). Cheap — a filter over the in-RAM Topology, identical cost for 1h or 7d.
3. **Client selector.** A dropdown in the COP toolbar: `7d / 3d / 48h / 24h / 12h / 6h / 1h`. Changing it re-requests the windowed snapshot and re-renders. Persists in the URL/localStorage so a reload keeps the operator's choice.
4. **Aged rendering (honesty).** Nodes whose `last_seen` is older than a freshness threshold (e.g. > 2× poll, or simply "not live") render **dimmed**, with a `last seen Xh ago` affordance. A faded node must never read as live — same honesty discipline as the env-health surfaces. Live nodes render normally.

### The window-depth ↔ seed-cost tradeoff (stated honestly)

The selector filters whatever the in-RAM index *contains*. The index is filled by (a) the live tail since boot and (b) the one-time boot replay. So:
- The selector is **free** (in-RAM filter) for any depth.
- The **maximum reachable depth right after a restart** = the boot-replay window. Selecting a window deeper than the boot seed shows "everything observed so far," which fills in over time as the live tail runs.
- Reaching a true 7d immediately after a cold start would require a 7d boot replay (heavy). v1 keeps the boot window **modest** (so startup stays fast and un-churny) and lets the deeper selector windows fill from live observation. A future increment can make a deep window trigger a **lightweight last-seen-only scan** (distinct `peer→max(ts)`, no full event/flow apply — far cheaper than today's replay) on demand.

So v1 deliberately separates two things the single replay-window conflated: **how fast/clean we boot** (small boot replay) vs. **how far back the operator can look** (the selector over the accumulating index).

## Result

- The operator picks a horizon from a dropdown; the COP instantly shows everything active within it, quiet ones faded and age-labeled — no redeploy, no per-click replay.
- Quiet-but-recent publishers reappear when the window widens, clearly marked historical, and drop when it narrows.
- Boot stays fast/clean (small boot replay) — the heavy-replay churn goes away.
- Age is honest end to end: live = bright, historical = faded + "last seen Xh ago", nothing beyond the window.

## v1.1 follow-ons (built)

Two of the three v1 limitations are now closed; both are wired through the `panoscope` role (`PANOSCOPE_MINIO_DEEP_WINDOW_HOURS`, `PANOSCOPE_PEER_RETENTION_HOURS`, both 168h/7d by default and trivially changed per-host).

- **Deep on-demand last-seen scan.** After the fast boot seed (`PANOSCOPE_MINIO_WINDOW_HOURS`) finishes, a SECOND replay pass runs lazily in a background task at the deep window (`PANOSCOPE_MINIO_DEEP_WINDOW_HOURS`, 7d) — staggered 30s so the fast seed and live tail populate first. It is non-blocking for load-up: the HTTP listener is already bound and the COP already serving when it starts. So deeper lens horizons (3d/7d) fill in soon after a cold start instead of only from live observation. It reuses the full `replay_into_topology` (not the cheaper distinct-`peer→max(ts)` scan the v1 doc imagined) — simpler, and since it's off the critical path the extra cost is invisible to the operator. Skipped when the deep window isn't deeper than the boot window.
- **In-RAM peer evictor.** A periodic task (hourly) drops peers whose real `last_seen_ms` is older than `PANOSCOPE_PEER_RETENTION_HOURS` (7d), bounding the otherwise upsert-only peer table over long uptimes. Retention is kept `>=` the deepest selectable lens window so eviction never races the lens (it removes only what the lens would already hide). `Topology::evict_stale(cutoff_ms)` in `panoscope-core`, unit-tested.

## Remaining v1 limitation

- Aged nodes are dimmed, not removed from the force layout; a very deep window on a churny fabric can still be busy. The **aggressive simplified view** (the per-publisher fanout rollup toggle — see `panoscope.md` §"Aggressive simplified view") addresses wide-shallow density: it collapses each publisher's whole fanout into one bundle node, drill-in preserved.
