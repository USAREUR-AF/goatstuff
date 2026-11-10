// SPDX-License-Identifier: Apache-2.0
//
// Dangling-edge guard for the force-graph render path.
//
// Why this exists (the durable lesson, see docs/findings/F-210):
// d3-force's `forceLink.initialize()` resolves every link's source/target
// id against the node map and throws a SYNCHRONOUS `Error("node not found:
// <id>")` the instant a link references an id with no node. That throw
// happens inside `.links()`, propagates out of the snapshot handler, and
// `ws.ts` catches it as "Handler error on snapshot" — aborting the ENTIRE
// frame before `render()` runs. Result: one malformed edge blanks the whole
// canvas (0/0 peers), which is exactly the fail-open behavior we observed on
// ZAGROS panoscope.
//
// A snapshot can legitimately carry an edge whose endpoint peer has no
// NodeView (a sink-only peer, a peer pruned mid-snapshot, a slot anchor that
// published but never declared a node). The renderer must DEGRADE that slice
// — drop the unresolvable edges, draw everything else — not fail the surface.
//
// Pure + framework-free so the contract is locked by edge-guard.spec.ts
// without a DOM, d3, or the heavyweight render-internal types.

/**
 * Minimal structural view of a graph node — only the id is load-bearing
 * for the guard. The render path's RNode satisfies this.
 * @typedef {Object} GuardNode
 * @property {string} id
 */

/**
 * Minimal structural view of a graph edge. d3 mutates source/target from
 * the wire `string` id into a resolved node object after the first tick, so
 * both shapes must be handled. The render path's REdge satisfies this.
 * @typedef {Object} GuardEdge
 * @property {string | GuardNode} source
 * @property {string | GuardNode} target
 */

/** Resolve an edge endpoint to its node id, whether it's still a raw wire
 *  id string or an already-resolved node object. */
export function edgeEndpointId(end) {
  return typeof end === "string" ? end : end.id;
}

/**
 * @template {GuardEdge} E
 * @typedef {Object} EdgePartition
 * @property {E[]} kept Edges whose source AND target both resolve to a node in the set.
 * @property {E[]} dropped Edges with at least one unresolvable endpoint — must NOT reach d3.
 * @property {string[]} missingIds The distinct missing node ids the dropped edges referenced (for a
 *   console.warn so the contract violation is visible, not silent).
 */

/**
 * Split `edges` into those safe to hand to d3-force and those that would
 * make it throw. `nodeIds` is the id set of every node that will be passed
 * to `simulation.nodes()`.
 */
export function partitionDanglingEdges(
  nodeIds,
  edges,
) {
  const kept = [];
  const dropped = [];
  const missing = new Set();
  for (const e of edges) {
    const s = edgeEndpointId(e.source);
    const t = edgeEndpointId(e.target);
    const sMissing = !nodeIds.has(s);
    const tMissing = !nodeIds.has(t);
    if (sMissing || tMissing) {
      dropped.push(e);
      if (sMissing) missing.add(s);
      if (tMissing) missing.add(t);
    } else {
      kept.push(e);
    }
  }
  return { kept, dropped, missingIds: Array.from(missing) };
}
