// SPDX-License-Identifier: Apache-2.0
//
// Unit spec for the signed-out rules (src/session.ts).
// Contract, from F-523: a 401 ends polling, a 403 does not, failures back off
// with jitter but never below the caller's own cadence, a hidden tab does not
// poll, and the sign-in reload happens once rather than in a loop.

import { test, expect } from "@playwright/test";
import {
  backoffMs,
  isSignedOut,
  pacingFailures,
  pollWithBackoff,
  shouldPoll,
  shouldReload,
} from "../web/src/session.js";

test("401 is signed out; 403 and the rest are not", () => {
  expect(isSignedOut(401)).toBe(true);
  // A role that covers nothing here is a live session (ADR 1073): reloading
  // into sign-in would send a signed-in person round a pointless loop.
  expect(isSignedOut(403)).toBe(false);
  expect(isSignedOut(200)).toBe(false);
  expect(isSignedOut(500)).toBe(false);
});

test("back-off grows, caps, and never drops below the caller's cadence", () => {
  const base = 2_000;
  expect(backoffMs(base, 0)).toBe(base);
  // No jitter: pin rand() to the midpoint so the growth is exact.
  const mid = () => 0.5;
  expect(backoffMs(base, 1, 60_000, mid)).toBe(4_000);
  expect(backoffMs(base, 3, 60_000, mid)).toBe(16_000);
  expect(backoffMs(base, 20, 60_000, mid)).toBe(60_000);
  // Jitter at its extremes still respects both bounds.
  for (const r of [() => 0, () => 1]) {
    const v = backoffMs(base, 5, 60_000, r);
    expect(v).toBeGreaterThanOrEqual(base);
    expect(v).toBeLessThanOrEqual(60_000 * 1.25);
  }
});

test("a hidden tab and a signed-out page both stop polling", () => {
  expect(shouldPoll(false, false)).toBe(true);
  expect(shouldPoll(true, false)).toBe(false);
  expect(shouldPoll(false, true)).toBe(false);
  expect(shouldPoll(true, true)).toBe(false);
});

test("the sign-in reload happens once, not in a loop", () => {
  const now = 1_000_000;
  // Never tried before.
  expect(shouldReload(now, 0)).toBe(true);
  // Just tried: a session still refused right after a sign-in round-trip will
  // not be fixed by another reload, so the page must show the banner instead.
  expect(shouldReload(now, now - 5_000)).toBe(false);
  // Long enough ago that this is a fresh expiry, not the same one.
  expect(shouldReload(now, now - 120_000)).toBe(true);
});

test("a failing poller is rescheduled with back-off, not at its base cadence", async () => {
  // The gap this closes: backoffMs existed and was tested, but every poller ran
  // on setInterval, whose period is fixed at creation — so a failing page kept
  // requesting every base tick forever, from every open tab.
  const delays: number[] = [];
  let pending: (() => void) | null = null;
  const setTimer = (fn: () => void, ms: number): number => {
    delays.push(ms);
    pending = fn;
    return delays.length;
  };
  const run = async (): Promise<void> => {
    const fn = pending;
    pending = null;
    if (fn) fn();
    await Promise.resolve();
  };

  let failures = 0;
  let failing = true;
  const stop = pollWithBackoff(
    2_000,
    () => {
      // Mirrors what the real pollers do: bump on failure, reset on success.
      failures = failing ? failures + 1 : 0;
    },
    () => failures,
    60_000,
    setTimer,
    () => {},
  );

  expect(delays[0]).toBe(2_000); // first scheduled run is the plain cadence
  await run(); // fails once
  await run(); // fails twice
  await run(); // fails three times

  // Each successive gap is strictly larger than the base cadence.
  for (const d of delays.slice(1)) expect(d).toBeGreaterThan(2_000);
  // And the schedule is monotonic while failures keep accumulating (jitter is
  // ±25%, so doubling always wins).
  expect(delays[delays.length - 1]).toBeGreaterThan(delays[1]);

  // A success collapses it straight back to the caller's own cadence.
  failing = false;
  await run();
  expect(delays[delays.length - 1]).toBe(2_000);

  stop();
});

test("a refused viewer is paced like a failing one, without reading as unreachable", async () => {
  // 403 is the one status that cannot succeed on the next tick: the viewer's
  // role does not cover the view, and a role does not change between two polls.
  // The pages keep it OUT of consecutiveFailures so the banner says
  // "restricted", not "unreachable" — so the schedule has to read both counts.
  expect(pacingFailures(0, 0)).toBe(0);
  expect(pacingFailures(0, 3)).toBe(3);
  expect(pacingFailures(2, 0)).toBe(2);
  expect(pacingFailures(2, 5)).toBe(5);

  const delays: number[] = [];
  let pending: (() => void) | null = null;
  const run = async (): Promise<void> => {
    const fn = pending;
    pending = null;
    if (fn) fn();
    await Promise.resolve();
  };

  const consecutiveFailures = 0; // a 403 is an answer, not a failed request
  let restrictedStreak = 0;
  let restricted = true;
  const stop = pollWithBackoff(
    2_000,
    () => {
      // Mirrors top.ts and who.ts: bump on 403, reset on success.
      restrictedStreak = restricted ? restrictedStreak + 1 : 0;
    },
    () => pacingFailures(consecutiveFailures, restrictedStreak),
    60_000,
    (fn, ms) => {
      delays.push(ms);
      pending = fn;
      return delays.length;
    },
    () => {},
  );

  await run();
  await run();
  await run();
  for (const d of delays.slice(1)) expect(d).toBeGreaterThan(2_000);

  // The role was granted: the next answer is a 200 and the cadence is back.
  restricted = false;
  await run();
  expect(delays[delays.length - 1]).toBe(2_000);

  stop();
});
