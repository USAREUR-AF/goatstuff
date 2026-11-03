// SPDX-License-Identifier: Apache-2.0
//
// Unit spec for the layout settle gate (src/layout-settle.ts). Pure data —
// no page, no d3 — so the "re-heat only on topology change" contract is locked
// deterministically, mirroring tests/edge-guard.spec.ts.
//
// Regression target (docs/design/panoscope.md §"Layout settle"; follow-on to
// F-210): applySnapshot() re-heated the force sim to alpha 0.6 on EVERY ~1.5s
// snapshot poll, so a stable topology never cooled and the graph jiggled
// forever. The gate must report "unchanged" for a data-only refresh and
// "changed" for any node/edge set difference (incl. the first frame and a
// filter collapse/expand that alters the derived set).

import { test, expect } from "@playwright/test";
import { topologySignature, topologyChanged } from "../src/layout-settle.js";

const EMPTY = { nodeSig: "", edgeSig: "" };

test("the bug: two identical snapshots → NOT changed (sim is allowed to settle)", () => {
  const nodes = [{ id: "a" }, { id: "b" }];
  const edges = [{ source: "a", target: "b", topic: "t/1" }];
  const first = topologySignature(nodes, edges);
  // Same topology, different data (rates/counts would differ on the real wire,
  // but they aren't part of the signature) — the second poll must read stable.
  const second = topologySignature(
    [{ id: "b" }, { id: "a" }], // even reordered
    [{ source: "a", target: "b", topic: "t/1" }],
  );
  expect(topologyChanged(first, second)).toBe(false);
});

test("first frame: empty previous signature always reads as changed → reheats", () => {
  const sig = topologySignature([{ id: "a" }], []);
  expect(topologyChanged(EMPTY, sig)).toBe(true);
});

test("a new peer appearing is a topology change", () => {
  const before = topologySignature([{ id: "a" }], []);
  const after = topologySignature([{ id: "a" }, { id: "b" }], []);
  expect(topologyChanged(before, after)).toBe(true);
});

test("a peer dropping is a topology change", () => {
  const before = topologySignature([{ id: "a" }, { id: "b" }], []);
  const after = topologySignature([{ id: "a" }], []);
  expect(topologyChanged(before, after)).toBe(true);
});

test("a new edge (same nodes) is a topology change", () => {
  const nodes = [{ id: "a" }, { id: "b" }];
  const before = topologySignature(nodes, [
    { source: "a", target: "b", topic: "t/1" },
  ]);
  const after = topologySignature(nodes, [
    { source: "a", target: "b", topic: "t/1" },
    { source: "a", target: "b", topic: "t/2" },
  ]);
  expect(topologyChanged(before, after)).toBe(true);
});

test("a filter collapse/expand changing the derived set re-heats", () => {
  // Same raw snapshot, but a collapse folds two peers into one super-node.
  const expanded = topologySignature(
    [{ id: "a" }, { id: "b" }, { id: "topic:t" }],
    [
      { source: "a", target: "topic:t", topic: "t" },
      { source: "b", target: "topic:t", topic: "t" },
    ],
  );
  const collapsed = topologySignature(
    [{ id: "super:grp" }, { id: "topic:t" }],
    [{ source: "super:grp", target: "topic:t", topic: "t" }],
  );
  expect(topologyChanged(expanded, collapsed)).toBe(true);
});

test("d3 rewrites string endpoints into node objects after a tick — signature is stable across the mutation", () => {
  const nodes = [{ id: "a" }, { id: "b" }];
  const stringEdges = [{ source: "a", target: "b", topic: "t/1" }];
  const objectEdges = [
    { source: { id: "a" }, target: { id: "b" }, topic: "t/1" },
  ];
  const a = topologySignature(nodes, stringEdges);
  const b = topologySignature(nodes, objectEdges);
  expect(topologyChanged(a, b)).toBe(false);
});

test("empty inputs are a no-op, never throw", () => {
  const sig = topologySignature([], []);
  expect(sig).toEqual({ nodeSig: "", edgeSig: "" });
  expect(topologyChanged(EMPTY, sig)).toBe(false);
});
