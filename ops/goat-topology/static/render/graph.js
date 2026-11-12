// SPDX-License-Identifier: Apache-2.0
// The orientation device. Deterministic radial layout, NOT a force simulation:
// a goatnet has a handful of routers, and a layout that settles differently on
// every refresh makes an operator re-find the same node each time. Same input,
// same picture.
import { $, esc, displayName, roleOf } from "../lib.js";

const W = 900, H = 380;
let onPick = () => {};

export function onNodeClick(fn) { onPick = fn; }

function layout(routers) {
  // The ring leaves room above and below for two label lines.
  const cx = W / 2, cy = H / 2 + 2;
  const r = routers.length === 1 ? 0 : Math.min(122, 60 + routers.length * 17);
  const pos = {};
  routers.forEach((x, i) => {
    const a = -Math.PI / 2 + (2 * Math.PI * i) / routers.length;
    pos[x.node || x.zid] = { x: cx + r * Math.cos(a), y: cy + r * Math.sin(a) };
  });
  return { pos, cx, cy, r };
}

/** Where a router's two label lines go: outside the ring, on the side away from
 *  the centre, so a label never lies across a link or a neighbouring router.
 *  A lone router sits at the centre and keeps its name above and tag below. */
function labelAt(p, c) {
  const dx = p.x - c.cx, dy = p.y - c.cy, len = Math.hypot(dx, dy);
  if (len < 1) return { nx: p.x, ny: p.y - 36, tx: p.x, ty: p.y + 44, anchor: "middle" };
  const ux = dx / len, uy = dy / len;
  // Above or below the ring, lean the text away from the neighbour on the
  // inner side so two adjacent labels do not run into each other.
  const lean = ux > 0.15 ? { x: p.x - 14, anchor: "start" }
    : ux < -0.15 ? { x: p.x + 14, anchor: "end" } : { x: p.x, anchor: "middle" };
  if (uy < -0.75) return { nx: lean.x, ny: p.y - 48, tx: lean.x, ty: p.y - 34, anchor: lean.anchor };
  if (uy > 0.75) return { nx: lean.x, ny: p.y + 44, tx: lean.x, ty: p.y + 58, anchor: lean.anchor };
  const x = p.x + (ux > 0 ? 34 : -34);
  return { nx: x, ny: p.y - 1, tx: x, ty: p.y + 13, anchor: ux > 0 ? "start" : "end" };
}

function node(x, p, picked, selected, c) {
  const stroke = x.stale ? "var(--status-err-fg)"
    : picked ? "var(--accent-flare)" : "var(--accent-hi)";
  // Client load as an arc around the node: weight at a glance, and it keeps
  // clients OFF the graph as edges, which is the honest picture — they cost
  // the routing layer nothing.
  const arc = Math.min(1, x.sessions_total / 60), circ = 2 * Math.PI * 25;
  // The tag says where the router is and what part it plays, so two routers
  // with similar names are still told apart at a glance.
  const role = roleOf(x);
  const L = labelAt(p, c);
  const tag = x.stale ? "gone quiet" : `${x.site} · ${role}`;
  const tagCol = x.stale ? "var(--status-err-fg)"
    : role === "gateway" ? "var(--c-amber-hi)" : "var(--fg-faint)";

  return `<g class="gnode" data-node="${esc(x.node || x.zid)}">
    ${selected ? `<circle cx="${p.x}" cy="${p.y}" r="31" fill="none"
      stroke="var(--accent-flare)" stroke-width="1.5" stroke-dasharray="3 3"/>` : ""}
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
    <text x="${L.nx}" y="${L.ny}" fill="var(--fg-primary)" font-family="var(--font-mono)"
      font-size="12" text-anchor="${L.anchor}">${esc(displayName(x.node || x.zid))}</text>
    <text x="${L.tx}" y="${L.ty}" fill="${tagCol}" font-family="var(--font-mono)"
      font-size="10" letter-spacing="1" text-anchor="${L.anchor}">${esc(tag)}</text>
  </g>`;
}

export function renderGraph(d, picks = new Set(), selected = null) {
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
    if (p) g += node(x, p, picks.has(x.node || x.zid), selected === (x.node || x.zid), { cx, cy });
  }
  svg.innerHTML = g;

  for (const n of svg.querySelectorAll(".gnode")) {
    n.onclick = () => onPick(n.dataset.node);
  }

  const north = d.edges.filter(e => e.region === "north").length;
  $("#graph-note").textContent =
    `${rs.length} router${rs.length === 1 ? "" : "s"} · ${north} routed link${north === 1 ? "" : "s"} · ${d.counts.clients} applications connected`;
}
