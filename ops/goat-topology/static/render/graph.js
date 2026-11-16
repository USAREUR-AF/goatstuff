// SPDX-License-Identifier: Apache-2.0
// The orientation device. Deterministic radial layout, NOT a force simulation:
// a layout that settles differently on every refresh makes an operator re-find
// the same node each time. Same input, same picture.
//
// Two kinds of node. A REPORTING router publishes its own snapshot and is drawn
// in full. A router that is NOT REPORTING is known only because another router's
// routing map lists it — a partner's router, or a host the publisher was never
// deployed to. It is drawn smaller, with a dashed outline and its display name,
// so the picture is complete without pretending to know its settings or load.
import { $, esc, displayName, roleOf, labelOf as nameOf, orgLine, ownerTone } from "../lib.js";

const W = 900, H_MIN = 380;
const R_REPORTING = 25, R_SILENT = 15;
let onPick = () => {};

export function onNodeClick(fn) { onPick = fn; }

/** The key a node and its edges share: node name for a reporting router,
 *  full ZID for one that is not reporting (the server keys edges the same). */
export const keyOf = x => x.node || x.zid;
/** What is printed beside a node: the node name; for a router that sends no
 *  report, its mesh peer name; else the derived display name (lib.js). */
export const labelOf = nameOf;

/** Arc length one node needs on the ring. Labels sit outside the ring, so this
 *  is the circle plus the gap two neighbouring labels need not to touch. */
const slot = x => (x.reporting === false ? 72 : 84);

/** The most characters a graph label line carries; the full text is in the
 *  node's tooltip, the detail and the table. */
const MAX_LABEL = 30;
const clip = (t, n = MAX_LABEL) => (t.length > n ? t.slice(0, n - 1) + "…" : t);
/** Height of one stack of label lines, for the stagger offset. */
const STACK = 44;
/** The organization line, clipped in the organization's name so the nation
 *  is never the part cut off. */
function orgShort(x) {
  const r = x.peer && x.peer.owner;
  if (x.owner_state === "recorded" && r) {
    const nation = r.org_nation || "nation not recorded";
    return `${clip(r.org_short_tag || r.org_id || "org not recorded", Math.max(10, MAX_LABEL - nation.length - 3))} · ${nation}`;
  }
  const o = x.owner;
  if (o && x.owner_state === "lead") {
    const nation = o.org_nation || "nation ?";
    return `lead: ${clip(o.org_display_name || o.org || "org ?", Math.max(8, MAX_LABEL - nation.length - 9))} · ${nation}`;
  }
  return orgLine(x);
}

/** A node's label lines as [text, color, font size]. Shared by the layout,
 *  which sizes the picture from them, and the node renderers. */
export function labelLines(x) {
  const org = orgLine(x);
  if (x.reporting === false) {
    const stroke = x.stale ? "var(--status-err-fg)" : "var(--fg-faint)";
    const ls = [[clip(labelOf(x)), "var(--fg-secondary, var(--fg-primary))", 10]];
    if (org) ls.push([orgShort(x), ownerTone(x), 9]);
    if (x.stale) ls.push(["gone quiet", stroke, 9]);
    return ls;
  }
  const role = roleOf(x);
  const tagCol = x.stale ? "var(--status-err-fg)"
    : role === "gateway" ? "var(--c-amber-hi)" : "var(--fg-faint)";
  const ls = [[clip(labelOf(x)), "var(--fg-primary)", 12],
    [x.stale ? "gone quiet" : `${x.site} · ${role}`, tagCol, 10]];
  if (org) ls.push([orgShort(x), ownerTone(x), 9]);
  return ls;
}

/** Reporting routers first, in the server's order; then silent ones by ZID.
 *  A small goatnet keeps the compact ring; past that the radius grows with the
 *  circumference the nodes need, so twenty routers do not overlap. The height
 *  leaves room above and below the ring for two label lines. */
export function layout(routers) {
  const rs = [...routers].sort((a, b) =>
    (a.reporting === false) - (b.reporting === false)
    || (a.reporting === false ? String(a.zid).localeCompare(String(b.zid)) : 0));
  const need = rs.reduce((s, x) => s + slot(x), 0);
  const r = rs.length === 1 ? 0
    : Math.max(Math.min(122, 60 + rs.length * 17), need / (2 * Math.PI));
  // Past a handful of routers the ring becomes an ellipse half again as wide as
  // it is tall: the routers at its top and bottom then sit further apart, which
  // is where neighboring labels would otherwise run into each other.
  const ry = r, rx = rs.length <= 4 ? r : r * 1.5;
  // Room for a staggered label (two label heights) above and below; side
  // labels as wide as the longest one, so a long peer name widens the picture
  // rather than running off its edge.
  const H = Math.max(H_MIN, Math.ceil(2 * ry + 2 * (R_REPORTING + 2 * STACK + 30)));
  const widest = rs.reduce((m, x) => Math.max(m, ...labelLines(x).map(([t, , sz]) => t.length * sz * 0.62)), 0);
  const Wd = Math.max(W, Math.ceil(2 * (rx + R_REPORTING + 9 + widest) + 24));
  const cx = Wd / 2, cy = H / 2 + 2;
  const pos = {};
  let at = 0, i = 0;
  for (const x of rs) {
    const w = slot(x);
    const a = -Math.PI / 2 + (2 * Math.PI * (at + w / 2)) / Math.max(need, 1);
    at += w;
    // Near the top and bottom, every other label moves one label-height
    // further out, so two neighbors' labels sit on different rows.
    const stagger = rs.length > 8 && Math.abs(Math.sin(a)) > 0.75 && i % 2 === 1;
    i++;
    pos[keyOf(x)] = { x: cx + rx * Math.cos(a), y: cy + ry * Math.sin(a), stagger };
  }
  return { pos, cx, cy, r, rx, ry, H, W: Wd };
}

/** Where a router's label lines go (`n` of them): outside the ring, on the side away from
 *  the centre, so a label never lies across a link or a neighbouring router.
 *  A lone router sits at the centre and keeps its name above and tag below.
 *  `rad` is the node's radius; the offsets keep the same gap for either size. */
export function labelAt(p, c, rad = R_REPORTING, n = 2) {
  const dx = p.x - c.cx, dy = p.y - c.cy, len = Math.hypot(dx, dy);
  const at = (x, y0, step, anchor) => ({ x, ys: Array.from({ length: n }, (_, i) => y0 + i * step), anchor });
  if (len < 1) return centred(p, rad, n);
  const ux = dx / len, uy = dy / len;
  // Above or below the ring, lean the text away from the neighbour on the
  // inner side so two adjacent labels do not run into each other.
  const lean = ux > 0.15 ? { x: p.x - 14, anchor: "start" }
    : ux < -0.15 ? { x: p.x + 14, anchor: "end" } : { x: p.x, anchor: "middle" };
  const out = p.stagger ? STACK : 0;
  if (uy < -0.75) return { ...at(lean.x, p.y - (rad + 9) - 14 * (n - 1) - out, 14, lean.anchor), staggered: !!out };
  if (uy > 0.75) return { ...at(lean.x, p.y + rad + 19 + out, 14, lean.anchor), staggered: !!out };
  const x = p.x + (ux > 0 ? rad + 9 : -(rad + 9));
  return at(x, p.y - 1 - 7 * (n - 2), 14, ux > 0 ? "start" : "end");
}

/** A lone router at the center: name above, the other lines below. */
function centred(p, rad, n) {
  const ys = [p.y - (rad + 11)];
  for (let i = 1; i < n; i++) ys.push(p.y + rad + 19 + (i - 1) * 14);
  return { x: p.x, ys, anchor: "middle" };
}

/** A faint line from a node to its label, when the label was staggered one row
 *  out and would otherwise float free of it. */
function leader(p, L, rad) {
  if (!L.staggered) return "";
  const below = L.ys[0] > p.y;
  const y2 = below ? L.ys[0] - 11 : L.ys[L.ys.length - 1] + 4;
  return `<line x1="${p.x}" y1="${p.y + (below ? rad : -rad)}" x2="${L.x}" y2="${y2}"
      stroke="var(--fg-faint)" stroke-width="0.8" opacity=".6"/>`;
}

/** One text line of a node label. */
const line = (L, i, txt, fill, size, extra = "") => `<text x="${L.x}" y="${L.ys[i]}" fill="${fill}"
      font-family="var(--font-mono)" font-size="${size}" ${extra} text-anchor="${L.anchor}">${esc(txt)}</text>`;

const selRing = (p, rad) => `<circle cx="${p.x}" cy="${p.y}" r="${rad + 6}" fill="none"
      stroke="var(--accent-flare)" stroke-width="1.5" stroke-dasharray="3 3"/>`;

function silentNode(x, p, picked, selected, c) {
  const stroke = x.stale ? "var(--status-err-fg)"
    : picked ? "var(--accent-flare)" : "var(--fg-faint)";
  const from = (x.known_from || []).map(displayName).join(", ");
  const lines = labelLines(x);
  const L = labelAt(p, c, R_SILENT, Math.max(lines.length, 2));
  return `<g class="gnode gnode--silent" data-node="${esc(keyOf(x))}">
    <title>${esc(labelOf(x))} · ${esc(x.zid)} — not reporting; listed in the routing map of ${esc(from)}</title>
    ${selected ? selRing(p, R_SILENT) : ""}
    <circle cx="${p.x}" cy="${p.y}" r="${R_SILENT}" fill="var(--bg-card)"
      stroke="${stroke}" stroke-width="1.5" stroke-dasharray="3 3"/>
    <text x="${p.x}" y="${p.y + 4}" fill="${stroke}" font-family="var(--font-mono)"
      font-size="11" text-anchor="middle">?</text>
    ${leader(p, L, R_SILENT)}
    ${lines.map(([t, f, sz], i) => line(L, i, t, f, sz, i ? 'letter-spacing="0.5"' : "")).join("")}
  </g>`;
}

function node(x, p, picked, selected, c) {
  if (x.reporting === false) return silentNode(x, p, picked, selected, c);
  const stroke = x.stale ? "var(--status-err-fg)"
    : picked ? "var(--accent-flare)" : "var(--accent-hi)";
  // Client load as an arc around the node: weight at a glance, and it keeps
  // clients OFF the graph as edges, which is the honest picture — they cost
  // the routing layer nothing.
  const arc = Math.min(1, x.sessions_total / 60), circ = 2 * Math.PI * R_REPORTING;
  // The tag says where the router is and what part it plays, so two routers
  // with similar names are still told apart at a glance.
  const lines = labelLines(x);
  const L = labelAt(p, c, R_REPORTING, lines.length);

  return `<g class="gnode" data-node="${esc(keyOf(x))}">
    ${selected ? selRing(p, R_REPORTING) : ""}
    <circle class="node-ring" cx="${p.x}" cy="${p.y}" r="${R_REPORTING}" fill="none"
      stroke="${stroke}" stroke-width="1" opacity=".3"/>
    <circle cx="${p.x}" cy="${p.y}" r="${R_REPORTING}" fill="none" stroke="var(--c-line-2)" stroke-width="3"/>
    ${arc > 0 ? `<circle cx="${p.x}" cy="${p.y}" r="${R_REPORTING}" fill="none" stroke="var(--c-amber-hi)"
      stroke-width="3" stroke-linecap="round"
      stroke-dasharray="${(circ * arc).toFixed(1)} ${circ}"
      transform="rotate(-90 ${p.x} ${p.y})" opacity=".85"/>` : ""}
    <circle cx="${p.x}" cy="${p.y}" r="18" fill="var(--bg-card)"
      stroke="${stroke}" stroke-width="1.5"/>
    <text x="${p.x}" y="${p.y + 4}" fill="${stroke}" font-family="var(--font-mono)"
      font-size="12" text-anchor="middle">${x.sessions_total}</text>
    <title>${esc(labelOf(x))} · ${esc(x.zid)}${x.owner || x.peer ? " · " + esc(orgLine(x)) : ""}</title>
    ${leader(p, L, R_REPORTING)}
    ${lines.map(([t, f, sz], i) => line(L, i, t, f, sz, i ? 'letter-spacing="0.5"' : "")).join("")}
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

export function renderGraph(d, picks = new Set(), selected = null) {
  const svg = $("#graph"), rs = d.routers;
  if (!rs.length) {
    svg.setAttribute("viewBox", `0 0 ${W} ${H_MIN}`);
    svg.innerHTML = `<text x="${W / 2}" y="${H_MIN / 2}" fill="var(--fg-faint)"
      font-family="var(--font-mono)" font-size="13" text-anchor="middle">
      No router has reported yet.</text>`;
    $("#graph-note").textContent = "nothing reporting";
    return;
  }

  const { pos, cx, cy, rx, ry, H, W: Wd } = layout(rs);
  svg.setAttribute("viewBox", `0 0 ${Wd} ${H}`);
  let g = "";
  for (const d of [66, 34]) {
    g += `<ellipse cx="${cx}" cy="${cy}" rx="${rx + d}" ry="${ry + d}" fill="none"
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
    if (p) g += node(x, p, picks.has(keyOf(x)), selected === keyOf(x), { cx, cy });
  }
  svg.innerHTML = g;

  for (const n of svg.querySelectorAll(".gnode")) {
    n.onclick = () => onPick(n.dataset.node);
  }
  $("#graph-note").textContent = graphNote(d);
}
