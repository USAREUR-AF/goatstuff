// SPDX-License-Identifier: Apache-2.0
//
// Layout settle gate — decide whether a freshly-applied snapshot represents a
// real topology change (peer/topic added or removed, a filter collapse/expand
// altering the derived set) or just a data-only refresh (rates, counts, labels
// arriving on the ~1.5s snapshot poll).
//
// Why this exists (docs/design/panoscope.md §"Layout settle"; follow-on to the
// F-210 render-resilience fix): applySnapshot() used to call
// `simulation.alpha(0.6).restart()` UNCONDITIONALLY on every poll. With a
// stable topology the layout re-heated to alpha 0.6 every ~1.5s and could never
// cool to alphaMin, so the force graph jiggled forever instead of settling.
// The churn was pre-existing (PR #69) but invisible until panoscope rendered
// again after F-210. We re-heat only when the topology signature changes.
//
// Pure data — no d3, no page — so the contract is locked deterministically,
// mirroring src/edge-guard.ts.

/** Minimal node shape: only the id participates in the topology signature. */
export interface SigNode {
  id: string;
}

/** Minimal edge shape. source/target may be plain id strings (as built each
 *  poll) or the node objects d3's forceLink rewrites them into after .links().
 *  Both shapes resolve to the same id so the signature is stable across the
 *  d3 mutation, exactly like edge-guard's edgeEndpointId. */
export interface SigEdge {
  source: string | { id: string };
  target: string | { id: string };
  topic: string;
}

function endpointId(ep: string | { id: string }): string {
  return typeof ep === "string" ? ep : ep.id;
}

/** A stable, order-independent signature of the rendered node-id set and the
 *  edge source|target|topic set. Sorted so node/edge ordering churn (which is
 *  not a topology change) doesn't trip a reheat. */
export function topologySignature(
  nodes: readonly SigNode[],
  edges: readonly SigEdge[],
): { nodeSig: string; edgeSig: string } {
  const nodeSig = nodes
    .map((n) => n.id)
    .sort()
    .join("\n");
  const edgeSig = edges
    .map((e) => `${endpointId(e.source)}|${endpointId(e.target)}|${e.topic}`)
    .sort()
    .join("\n");
  return { nodeSig, edgeSig };
}

/** True when either signature differs from the previously-rendered one. An
 *  empty previous signature (first frame) always reads as changed → reheats. */
export function topologyChanged(
  prev: { nodeSig: string; edgeSig: string },
  next: { nodeSig: string; edgeSig: string },
): boolean {
  return prev.nodeSig !== next.nodeSig || prev.edgeSig !== next.edgeSig;
}
