// SPDX-License-Identifier: Apache-2.0
//
// Latency-aware freshness gate for the fleet-bar staleness surface.
//
// Pure, side-effect-free arithmetic extracted from installFleetBar()
// (main.ts) so the staleness decision is unit-testable without a DOM,
// fetch mocking, or wall-clock timing. See docs/design/panoscope.md
// §"Self-aware degradation" / the freshness-gate notes for the why.
//
// The problem this solves: the original gate flagged STALE when the last
// successful poll was older than a FLAT `4× poll interval`. That assumes
// the fetch itself is ~free — true on a fast link, false on a relay-routed
// / high-RTT mesh path (e.g. the operator reaching a site through a distant
// relay). There the effective refresh cycle is `poll + fetchDuration`, so
// steady-state age already sits near `poll + fetch`; when fetch ≈ 2 s the
// flat 6 s gate leaves under one fetch-duration of headroom and a SINGLE
// slow or timed-out cycle trips STALE even though the feed is live. That
// false positive is the "panoscope goes stale so much" symptom.
//
// Fix: widen the age floor to also clear a few OBSERVED fetch-durations of
// slack (tracked as an EMA of successful round-trips). STALE then means
// "the feed stopped," not "the network to this observer is slow." On a fast
// link emaFetchMs ≈ 0 and the floor collapses to the original 4× poll, so
// fast-link behavior is unchanged.

/** EMA smoothing factor for the fetch-latency estimate. */
export const FETCH_EMA_ALPHA = 0.3;

/**
 * Age (ms since last successful poll) beyond which the fleet bar reads
 * STALE. `max` of the legacy fast-link floor (4× poll) and a latency-aware
 * floor (2× poll of cadence slack + 4× the observed fetch EMA). With
 * emaFetchMs = 0 this equals 4× poll exactly.
 */
export function staleAgeFloorMs(pollMs, emaFetchMs) {
  return Math.max(4 * pollMs, 2 * pollMs + 4 * emaFetchMs);
}

/**
 * The staleness decision. Two independent triggers:
 *  - `consecutiveFailures >= 2` — the hard real-outage signal (two
 *    back-to-back failed/timed-out fetches), independent of link latency.
 *  - `ageMs > staleAgeFloorMs(...)` — the "alive but pathologically slow /
 *    hung without throwing" backstop, now latency-aware.
 */
export function isStale(opts) {
  return (
    opts.consecutiveFailures >= 2 ||
    opts.ageMs > staleAgeFloorMs(opts.pollMs, opts.emaFetchMs)
  );
}

/**
 * Fold one observed successful round-trip into the fetch-latency EMA. The
 * first sample (prevEmaMs === 0) initializes outright so the floor reflects
 * the real link immediately instead of decaying up from a meaningless 0.
 */
export function foldFetchEma(
  prevEmaMs,
  sampleMs,
  alpha = FETCH_EMA_ALPHA,
) {
  return prevEmaMs === 0 ? sampleMs : alpha * sampleMs + (1 - alpha) * prevEmaMs;
}

// ---------------------------------------------------------------------
// Stream-pane staleness: the publish interval the threshold adapts to.
//
// The pane reads STALE once the newest sample is ~2.5× the topic's own
// observed publish interval old (applyStreamStaleness in main.ts). For a
// MERGED pane — several topics interleaved newest-first — the interval
// must be measured per topic. Measured across the merged list it collapses
// to the gap BETWEEN publishers: two 30 s-cadence reporters that fire 4 s
// apart give a 4 s "interval", a 10 s threshold, and a healthy 20 s-old
// pane reads STALE (seen on ops/consistency).

/** Observed median inter-sample gap (ms) from a newest-first timestamp
 *  list — one topic's natural publish interval. 0 when <2 samples. */
export function medianIntervalMs(tsList) {
  const ts = tsList.filter((t) => typeof t === "number");
  if (ts.length < 2) return 0;
  const gaps = [];
  for (let i = 0; i < ts.length - 1; i++) gaps.push(ts[i] - ts[i + 1]);
  gaps.sort((a, b) => a - b);
  return gaps[Math.floor(gaps.length / 2)];
}

/** Publish interval for a merged multi-topic pane: the median gap of
 *  each topic on its own, then the FASTEST topic's interval — the
 *  quickest publisher sets the bar, since the pane should read STALE
 *  only when even it has missed a beat. Topics with a single sample
 *  contribute nothing; 0 when no topic has two samples, which the
 *  caller maps to the unknown-interval floor. Input order is free. */
export function streamIntervalMs(
  entries,
) {
  const byTopic = new Map();
  for (const e of entries) {
    if (typeof e.ts_ms !== "number") continue;
    const list = byTopic.get(e.topic);
    if (list) list.push(e.ts_ms);
    else byTopic.set(e.topic, [e.ts_ms]);
  }
  let fastest = 0;
  for (const ts of byTopic.values()) {
    ts.sort((a, b) => b - a); // newest-first, as medianIntervalMs expects
    const iv = medianIntervalMs(ts);
    if (iv > 0 && (fastest === 0 || iv < fastest)) fastest = iv;
  }
  return fastest;
}
