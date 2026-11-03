// SPDX-License-Identifier: Apache-2.0
// The orientation device. Deterministic radial layout, NOT a force simulation:
// a goatnet has a handful of routers, and a layout that settles differently on
// every refresh makes an operator re-find the same node each time. Same input,
// same picture.
import { $, esc } from "../lib.js";

const W = 900, H = 380;
let onPick = () => {};

export function onNodeClick(fn) { onPick = fn; }

function layout(routers) {
  const cx = W / 2, cy = H / 2 - 6;
  const r = routers.length === 1 ? 0 : Math.min(132, 60 + routers.length * 17);
  const pos = {};
  routers.forEach((x, i) => {
    const a = -Math.PI / 2 + (2 * Math.PI * i) / routers.length;
    pos[x.node || x.zid] = { x: cx + r * Math.cos(a), y: cy + r * Math.sin(a) };
  });
  return { pos, cx, cy, r };
}

function node(x, p, picked) {
  const stroke = x.stale ? "var(--status-err-fg)"
    : picked ? "var(--accent-flare)" : "var(--accent-hi)";
  // Client load as an arc around the node: weight at a glance, and it keeps
  // clients OFF the graph as edges, which is the honest picture — they cost
  // the routing layer nothing.
  const arc = Math.min(1, x.sessions_total / 60), circ = 2 * Math.PI * 25;
  const tag = x.stale ? "gone quiet" : x.gateway_mode;
  const tagCol = x.stale ? "var(--status-err-fg)"
    : x.gateway_mode === "auto" ? "var(--fg-faint)" : "var(--c-amber-hi)";

  return `<g class="gnode" data-node="${esc(x.node || x.zid)}">
    <circle class="node-ring" cx="${p.x}" cy="${p.y}" r="25" fill="none"
      stroke="${stroke}" stroke-width="1" opacity=".3"/>
    <circle cx="${p.x}" cy="${p.y}" r="25" fill="none" stroke="var(--c-line-2)" stroke-width="3"/>
    ${arc > 0 ? `<circle cx="${p.x}" cy="${p.y}" r="25" fill="none" stroke="var(--c-amber-hi)"
      stroke-width="3" stroke-linecap="round"
      stroke-dasharray="${(circ * arc).toFixed(1)} ${circ}"
      transform="rotate(-90 ${p.x} ${p.y})" opacity=".85"/>` : ""}
    <circle cx="${p.x}" cy="${p.y}" r="18"
      fill="${x.stale ? "var(--status-err-bg)" : picked ? "var(--accent-deep)" : "var(--bg-card)"}"
      stroke="${stroke}" stroke-width="1.5"/>
    <text x="${p.x}" y="${p.y + 4}" fill="${stroke}" font-family="var(--font-mono)"
      font-size="12" text-anchor="middle">${x.sessions_total}</text>
    <text x="${p.x}" y="${p.y - 36}" fill="var(--fg-primary)" font-family="var(--font-mono)"
      font-size="12" text-anchor="middle">${esc(x.node || x.zid)}</text>
    <text x="${p.x}" y="${p.y + 44}" fill="${tagCol}" font-family="var(--font-mono)"
      font-size="10" letter-spacing="1" text-anchor="middle">${esc(tag)}</text>
  </g>`;
}

export function renderGraph(d, picks = new Set()) {
  const svg = $("#graph"), rs = d.routers;
  if (!rs.length) {
    svg.innerHTML = `<text x="${W / 2}" y="${H / 2}" fill="var(--fg-faint)"
      font-family="var(--font-mono)" font-size="13" text-anchor="middle">
      No router has reported yet.</text>`;
    $("#graph-note").textContent = "nothing reporting";
    return;
  }

  const { pos, cx, cy, r } = layout(rs);
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
    const p = pos[x.node || x.zid];
    if (p) g += node(x, p, picks.has(x.node || x.zid));
  }
  svg.innerHTML = g;

  for (const n of svg.querySelectorAll(".gnode")) {
    n.onclick = () => onPick(n.dataset.node);
  }

  const north = d.edges.filter(e => e.region === "north").length;
  $("#graph-note").textContent =
    `${rs.length} router${rs.length === 1 ? "" : "s"} · ${north} routed link${north === 1 ? "" : "s"} · ${d.counts.clients} applications connected`;
}
