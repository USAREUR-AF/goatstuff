// SPDX-License-Identifier: Apache-2.0
//
// Unit spec for the dangling-edge guard (src/edge-guard.ts). Pure data —
// no page, no fetch mock, no d3 — so the graceful-degradation contract is
// locked deterministically.
//
// Regression target (docs/findings/F-210): a snapshot edge whose endpoint
// has no node made d3-force throw "node not found: <id>" at .links(), which
// ws.ts swallowed as "Handler error on snapshot" and blanked the whole
// canvas (0/0 peers). The guard must DROP that edge and
// keep the rest — degrade the slice, never the surface.

import { test, expect } from "@playwright/test";
import { partitionDanglingEdges, edgeEndpointId } from "../src/edge-guard.js";

test("the ZAGROS failure: a single dangling edge is dropped, the rest survive", () => {
  const nodeIds = new Set(["alpha", "beta"]);
  const edges = [
    { source: "alpha", target: "beta" }, // both resolve — kept
    { source: "alpha", target: "ba5225f8d046bce3cd2d6782764b94d8c" }, // sink has no node
  ];
  const { kept, dropped, missingIds } = partitionDanglingEdges(nodeIds, edges);
  expect(kept).toEqual([{ source: "alpha", target: "beta" }]);
  expect(dropped).toHaveLength(1);
  expect(missingIds).toEqual(["ba5225f8d046bce3cd2d6782764b94d8c"]);
});

test("all edges resolve → nothing dropped, no warn payload", () => {
  const nodeIds = new Set(["a", "b", "c"]);
  const edges = [
    { source: "a", target: "b" },
    { source: "b", target: "c" },
  ];
  const { kept, dropped, missingIds } = partitionDanglingEdges(nodeIds, edges);
  expect(kept).toHaveLength(2);
  expect(dropped).toHaveLength(0);
  expect(missingIds).toEqual([]);
});

test("a missing SOURCE is caught, not just a missing target", () => {
  const nodeIds = new Set(["b"]);
  const { kept, dropped, missingIds } = partitionDanglingEdges(nodeIds, [
    { source: "ghost", target: "b" },
  ]);
  expect(kept).toHaveLength(0);
  expect(dropped).toHaveLength(1);
  expect(missingIds).toEqual(["ghost"]);
});

test("both endpoints missing report both ids, deduped", () => {
  const nodeIds = new Set<string>();
  const { missingIds } = partitionDanglingEdges(nodeIds, [
    { source: "x", target: "y" },
    { source: "x", target: "z" }, // x repeats — must not duplicate
  ]);
  expect([...missingIds].sort()).toEqual(["x", "y", "z"]);
});

test("d3 mutates string endpoints into node objects after the first tick — both shapes resolve", () => {
  const nodeIds = new Set(["a", "b"]);
  // After d3 initializes, source/target are the node objects, not ids.
  const edges = [{ source: { id: "a" }, target: { id: "b" } }];
  const { kept, dropped } = partitionDanglingEdges(nodeIds, edges);
  expect(kept).toHaveLength(1);
  expect(dropped).toHaveLength(0);
  expect(edgeEndpointId({ id: "a" })).toBe("a");
  expect(edgeEndpointId("a")).toBe("a");
});

test("empty inputs are a no-op, never throw", () => {
  const { kept, dropped, missingIds } = partitionDanglingEdges(new Set(), []);
  expect(kept).toEqual([]);
  expect(dropped).toEqual([]);
  expect(missingIds).toEqual([]);
});
