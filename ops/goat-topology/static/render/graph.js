// SPDX-License-Identifier: Apache-2.0
// The orientation device. Deterministic radial layout, NOT a force simulation:
// a layout that settles differently on every refresh makes an operator re-find
// the same node each time. Same input, same picture.
//
// Two kinds of node. A REPORTING router publishes its own snapshot and is drawn
// in full. A router that is NOT REPORTING is known only because another router's
// routing map lists it — a partner's router, or a host the publisher was never
// deployed to. It is drawn smaller, with a dashed outline and its short ZID, so
// the picture is complete without pretending to know its settings or load.
import { $, esc } from "../lib.js";

const W = 900, H_MIN = 380;
const R_REPORTING = 25, R_SILENT = 15;
let onPick = () => {};

export function onNodeClick(fn) { onPick = fn; }

/** The key a node and its edges share: node name for a reporting router,
 *  full ZID for one that is not reporting (the server keys edges the same). */
export const keyOf = x => x.node || x.zid;
/** What is printed under a node. A full ZID is 32 hex characters; eight is how
 *  zenoh's own logs abbreviate it, and enough to match one against a log. */
export const labelOf = x => x.node || String(x.zid || "").slice(0, 8);

/** Arc length one node needs: room for its circle AND its label, which is
 *  wider than the circle for an eight-character ZID. */
const slot = x => (x.reporting === false ? 64 : 84);

/** Reporting routers first, in the server's order; then silent ones by ZID.
 *  The radius grows with the circumference the nodes need, so twenty routers
 *  do not overlap the way they would on a fixed circle. */
export function layout(routers) {
  const rs = [...routers].sort((a, b) =>
    (a.reporting === false) - (b.reporting === false)
    || (a.reporting === false ? String(a.zid).localeCompare(String(b.zid)) : 0));
  const need = rs.reduce((s, x) => s + slot(x), 0);
  const r = rs.length === 1 ? 0 : Math.max(132, need / (2 * Math.PI));
  const H = Math.max(H_MIN, Math.ceil(2 * r + 150));
  const cx = W / 2, cy = H / 2 - 6;
  const pos = {};
  let at = 0;
  for (const x of rs) {
    const w = slot(x);
    const a = -Math.PI / 2 + (2 * Math.PI * (at + w / 2)) / Math.max(need, 1);
    at += w;
    pos[keyOf(x)] = { x: cx + r * Math.cos(a), y: cy + r * Math.sin(a) };
  }
  return { pos, cx, cy, r, H };
}

function silentNode(x, p, picked) {
  const stroke = x.stale ? "var(--status-err-fg)"
    : picked ? "var(--accent-flare)" : "var(--fg-faint)";
  const from = (x.known_from || []).join(", ");
  return `<g class="gnode gnode--silent" data-node="${esc(keyOf(x))}">
    <title>${esc(x.zid)} — not reporting; listed in the routing map of ${esc(from)}</title>
    <circle cx="${p.x}" cy="${p.y}" r="${R_SILENT}" fill="var(--bg-card)"
      stroke="${stroke}" stroke-width="1.5" stroke-dasharray="3 3"/>
    <text x="${p.x}" y="${p.y + 4}" fill="${stroke}" font-family="var(--font-mono)"
      font-size="11" text-anchor="middle">?</text>
    <text x="${p.x}" y="${p.y - R_SILENT - 7}" fill="var(--fg-secondary, var(--fg-primary))"
      font-family="var(--font-mono)" font-size="10" text-anchor="middle">${esc(labelOf(x))}</text>
    ${x.stale ? `<text x="${p.x}" y="${p.y + R_SILENT + 14}" fill="${stroke}" font-family="var(--font-mono)"
      font-size="9" letter-spacing="1" text-anchor="middle">gone quiet</text>` : ""}
  </g>`;
}

function node(x, p, picked) {
  if (x.reporting === false) return silentNode(x, p, picked);
  const stroke = x.stale ? "var(--status-err-fg)"
    : picked ? "var(--accent-flare)" : "var(--accent-hi)";
  // Client load as an arc around the node: weight at a glance, and it keeps
  // clients OFF the graph as edges, which is the honest picture — they cost
  // the routing layer nothing.
  const arc = Math.min(1, x.sessions_total / 60), circ = 2 * Math.PI * R_REPORTING;
  const tag = x.stale ? "gone quiet" : x.gateway_mode;
  const tagCol = x.stale ? "var(--status-err-fg)"
    : x.gateway_mode === "auto" ? "var(--fg-faint)" : "var(--c-amber-hi)";

  return `<g class="gnode" data-node="${esc(keyOf(x))}">
    <circle class="node-ring" cx="${p.x}" cy="${p.y}" r="${R_REPORTING}" fill="none"
      stroke="${stroke}" stroke-width="1" opacity=".3"/>
    <circle cx="${p.x}" cy="${p.y}" r="${R_REPORTING}" fill="none" stroke="var(--c-line-2)" stroke-width="3"/>
    ${arc > 0 ? `<circle cx="${p.x}" cy="${p.y}" r="${R_REPORTING}" fill="none" stroke="var(--c-amber-hi)"
      stroke-width="3" stroke-linecap="round"
      stroke-dasharray="${(circ * arc).toFixed(1)} ${circ}"
      transform="rotate(-90 ${p.x} ${p.y})" opacity=".85"/>` : ""}
    <circle cx="${p.x}" cy="${p.y}" r="18"
      fill="${x.stale ? "var(--status-err-bg)" : picked ? "var(--accent-deep)" : "var(--bg-card)"}"
      stroke="${stroke}" stroke-width="1.5"/>
    <text x="${p.x}" y="${p.y + 4}" fill="${stroke}" font-family="var(--font-mono)"
      font-size="12" text-anchor="middle">${x.sessions_total}</text>
    <text x="${p.x}" y="${p.y - 36}" fill="var(--fg-primary)" font-family="var(--font-mono)"
      font-size="12" text-anchor="middle">${esc(labelOf(x))}</text>
    <text x="${p.x}" y="${p.y + 44}" fill="${tagCol}" font-family="var(--font-mono)"
      font-size="10" letter-spacing="1" text-anchor="middle">${esc(tag)}</text>
  </g>`;
}

/** The sentence under the graph. Exported so a test can hold it to the counts. */
export function graphNote(d) {
  const rs = d.routers;
  const silent = rs.filter(x => x.reporting === false).length;
  const routed = d.edges.filter(e => e.region === "north").length;
  const term = d.edges.length - routed;
  const s = n => (n === 1 ? "" : "s");
  return `${rs.length} router${s(rs.length)}`
    + (silent ? ` (${rs.length - silent} reporting, ${silent} not reporting)` : "")
    + ` · ${routed} routed link${s(routed)} · ${term} terminated link${s(term)}`
    + ` · ${d.counts.clients} applications connected`;
}

export function renderGraph(d, picks = new Set()) {
  const svg = $("#graph"), rs = d.routers;
  if (!rs.length) {
    svg.setAttribute("viewBox", `0 0 ${W} ${H_MIN}`);
    svg.innerHTML = `<text x="${W / 2}" y="${H_MIN / 2}" fill="var(--fg-faint)"
      font-family="var(--font-mono)" font-size="13" text-anchor="middle">
      No router has reported yet.</text>`;
    $("#graph-note").textContent = "nothing reporting";
    return;
  }

  const { pos, cx, cy, r, H } = layout(rs);
  svg.setAttribute("viewBox", `0 0 ${W} ${H}`);
  let g = "";
  for (const rr of [r + 66, r + 34]) {
    g += `<circle cx="${cx}" cy="${cy}" r="${rr}" fill="none"
           stroke="var(--c-line)" stroke-width="1" opacity=".45"/>`;
  }
  for (const e of d.edges) {
    const A = pos[e.a], B = pos[e.b];
    if (!A || !B) continue;
    // Only the routed domain gets the live flow. Animating everything would
    // say nothing.
    const col = e.region === "north" ? "var(--c-blue-hi)" : "var(--c-amber-hi)";
    g += `<line class="edge" x1="${A.x}" y1="${A.y}" x2="${B.x}" y2="${B.y}"
            stroke="${col}" stroke-width="1.6" opacity=".26"/>
          <line class="edge edge-flow" x1="${A.x}" y1="${A.y}" x2="${B.x}" y2="${B.y}"
            stroke="${col}" stroke-width="1.6" opacity=".9"/>`;
  }
  for (const x of rs) {
    const p = pos[keyOf(x)];
    if (p) g += node(x, p, picks.has(keyOf(x)));
  }
  svg.innerHTML = g;

  for (const n of svg.querySelectorAll(".gnode")) {
    n.onclick = () => onPick(n.dataset.node);
  }
  $("#graph-note").textContent = graphNote(d);
}
