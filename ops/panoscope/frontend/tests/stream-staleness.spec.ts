// SPDX-License-Identifier: Apache-2.0
//
// Unit spec for the stream-pane publish-interval helpers (src/freshness.ts)
// and the shared short-id rule (src/ids.ts). Pure arithmetic — no page.
//
// Contract under test:
//   1. streamIntervalMs measures each topic's cadence on its OWN samples.
//      Two 30 s reporters interleaved 4 s apart must yield 30 s, not 4 s —
//      the false "STALE · last seen 20 sec ago" on a healthy merged pane.
//   2. The fastest topic sets the interval; a lone sample contributes none.
//   3. shortId abbreviates only hex ZIDs; `ops/fabric` stays whole.

import { test, expect } from "@playwright/test";
import { medianIntervalMs, streamIntervalMs } from "../src/freshness.js";
import { shortId, looksLikeZid } from "../src/ids.js";

const S = 1_000;

test("merged pane: two 30 s topics offset by 4 s measure 30 s, not 4 s", () => {
  // report at t=0, 30, 60; health at t=4, 34, 64 (newest-first as merged).
  const entries = [
    { topic: "ops/consistency/health/x", ts_ms: 64 * S },
    { topic: "ops/consistency/report/x", ts_ms: 60 * S },
    { topic: "ops/consistency/health/x", ts_ms: 34 * S },
    { topic: "ops/consistency/report/x", ts_ms: 30 * S },
    { topic: "ops/consistency/health/x", ts_ms: 4 * S },
    { topic: "ops/consistency/report/x", ts_ms: 0 },
  ];
  // The old cross-topic measurement collapsed to the 4 s offset.
  expect(medianIntervalMs(entries.map((e) => e.ts_ms))).toBe(4 * S);
  expect(streamIntervalMs(entries)).toBe(30 * S);
  // A 20 s-old pane is inside 2.5× the real cadence.
  expect(20 * S).toBeLessThan(2.5 * streamIntervalMs(entries));
});

test("fastest topic sets the interval; single samples contribute nothing", () => {
  const entries = [
    { topic: "slow", ts_ms: 100 * S },
    { topic: "fast", ts_ms: 99 * S },
    { topic: "fast", ts_ms: 98 * S },
    { topic: "lone", ts_ms: 97 * S },
    { topic: "slow", ts_ms: 70 * S },
    { topic: "fast", ts_ms: 97 * S },
  ];
  expect(streamIntervalMs(entries)).toBe(1 * S);
  expect(streamIntervalMs([{ topic: "lone", ts_ms: 1 }])).toBe(0);
  expect(streamIntervalMs([])).toBe(0);
  expect(streamIntervalMs([{ topic: "a", ts_ms: undefined }, { topic: "a", ts_ms: 5 }])).toBe(0);
});

test("single-topic median is unchanged", () => {
  expect(medianIntervalMs([30 * S, 20 * S, 10 * S, 0])).toBe(10 * S);
  expect(medianIntervalMs([5])).toBe(0);
});

test("shortId cuts hex ZIDs only; topic-prefix ids stay whole", () => {
  expect(shortId("0405fc463154cb88502bc25fa9f0989a")).toBe("a9f0989a");
  expect(shortId("f8ce3c74")).toBe("f8ce3c74");
  expect(shortId("ops/fabric")).toBe("ops/fabric");
  expect(shortId("ops/consistency")).toBe("ops/consistency");
  expect(shortId("caracal/power-demo")).toBe("caracal/power-demo");
  expect(looksLikeZid("mgmt-01")).toBe(false);
});
