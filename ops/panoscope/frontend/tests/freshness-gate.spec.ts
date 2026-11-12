// SPDX-License-Identifier: Apache-2.0
//
// Unit spec for the latency-aware fleet-bar freshness gate
// (src/freshness.ts). Pure arithmetic — no page, no fetch mock, no
// wall-clock timing — so the staleness contract is locked deterministically.
//
// Contract under test (docs/design/panoscope.md freshness-gate notes):
//   1. On a FAST link (emaFetchMs ≈ 0) the floor is exactly the legacy
//      4× poll, so fast-link behavior is byte-identical to before.
//   2. On a SLOW link the floor widens with the observed fetch EMA, so an
//      age that WOULD have tripped the flat 4× gate does NOT read STALE —
//      a slow link alone must not read as a stale feed.
//   3. The hard `consecutiveFailures >= 2` trigger still trips instantly,
//      independent of link latency (a real outage is never masked).
//   4. The EMA initializes on its first sample rather than decaying from 0.

import { test, expect } from "@playwright/test";
import { staleAgeFloorMs, isStale, foldFetchEma } from "../web/src/freshness.js";

const LIVE_POLL = 1_500; // POLICY.live.snapshotPollMs

test("fast link: floor collapses to the legacy 4× poll", () => {
  expect(staleAgeFloorMs(LIVE_POLL, 0)).toBe(4 * LIVE_POLL); // 6_000
});

test("slow link: floor widens to clear several fetch-durations of slack", () => {
  // ema ≈ 2 s fetch → 2*1500 + 4*2000 = 11_000, dominates the 6_000 floor.
  expect(staleAgeFloorMs(LIVE_POLL, 2_000)).toBe(11_000);
  expect(staleAgeFloorMs(LIVE_POLL, 2_000)).toBeGreaterThan(staleAgeFloorMs(LIVE_POLL, 0));
});

test("slow-link single-slow-cycle that tripped the flat gate no longer reads STALE", () => {
  // A ~9 s age: PAST the flat 6 s gate (would have flickered STALE), but a
  // slow link whose fetches run ~2 s has an 11 s floor → still live.
  const ageMs = 9_000;
  expect(ageMs).toBeGreaterThan(4 * LIVE_POLL); // would trip the old gate
  expect(isStale({ consecutiveFailures: 0, ageMs, pollMs: LIVE_POLL, emaFetchMs: 2_000 })).toBe(false);
  // Same age on a fast link IS stale — the feed genuinely stopped.
  expect(isStale({ consecutiveFailures: 0, ageMs, pollMs: LIVE_POLL, emaFetchMs: 0 })).toBe(true);
});

test("genuinely stopped feed still reads STALE even on a slow link", () => {
  // Age far past even the widened floor → real staleness, correctly flagged.
  expect(isStale({ consecutiveFailures: 0, ageMs: 30_000, pollMs: LIVE_POLL, emaFetchMs: 2_000 })).toBe(true);
});

test("hard real-outage trigger fires on two failures regardless of latency", () => {
  expect(isStale({ consecutiveFailures: 2, ageMs: 0, pollMs: LIVE_POLL, emaFetchMs: 9_999 })).toBe(true);
  expect(isStale({ consecutiveFailures: 1, ageMs: 0, pollMs: LIVE_POLL, emaFetchMs: 0 })).toBe(false);
});

test("EMA initializes on first sample, then decays", () => {
  const first = foldFetchEma(0, 2_000);
  expect(first).toBe(2_000); // initialized outright, not 0.3*2000
  // A subsequent faster sample pulls it down but not all the way.
  const second = foldFetchEma(first, 1_000);
  expect(second).toBeCloseTo(0.3 * 1_000 + 0.7 * 2_000, 6); // 1_700
});
