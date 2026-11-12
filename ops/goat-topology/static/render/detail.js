// SPDX-License-Identifier: Apache-2.0
// One router, everything the page holds about it, in one place: identity,
// declared mode beside running state, its neighbours, the findings that name
// it and the changes it made. Selecting a router opens this; nothing here
// fetches — it is a second reading of the payload the page already has, so a
// drill-down costs the fabric nothing (ADR 1123).
import { $, esc, ageSecs, severityChip, displayName, roleOf } from "../lib.js";

let onSelect = () => {};
let onCarve = () => {};
export function onDetailSelect(fn) { onSelect = fn; }
export function onDetailCarve(fn) { onCarve = fn; }

const key = x => x.node || x.zid;

/** Every link that touches `id`, read from this router's side. */
function neighbours(d, id, self) {
  const gateway = self && roleOf(self) === "gateway";
  return d.edges.filter(e => e.a === id || e.b === id).map(e => {
    const other = e.a === id ? e.b : e.a;
    const routed = e.region === "north";
    return {
      other,
      reporting: d.routers.some(r => key(r) === other),
      kind: routed ? "routed" : "terminated",
      reads: routed ? "same routing group"
        : gateway ? "sits below this router" : "this router sits below it",
    };
  }).sort((a, b) => a.kind.localeCompare(b.kind) || a.other.localeCompare(b.other));
}

/** A finding names one router, a pair ("a ↔ b"), or the environment. */
const names = (f, id) => String(f.subject || "").split(" ↔ ").map(s => s.trim()).includes(id);

export function renderDetail(d, selected, picks = new Set(), now = Date.now()) {
  const box = $("#detail");
  if (!selected) {
    box.innerHTML = `<div class="quiet">Select a router in the graph or in the Routers table
      to see its identity, neighbours, findings and changes together.</div>`;
    $("#detail-note").textContent = "nothing selected";
    return;
  }
  const x = d.routers.find(r => key(r) === selected);
  if (!x) {
    // Known from another router's link table, never from its own report.
    const ns = neighbours(d, selected, null);
    box.innerHTML = `<div class="d-head"><span class="d-name">${esc(displayName(selected))}</span>
        <span class="chip chip--muted">not reporting</span></div>
      <p class="d-plain">No report has arrived from this router. It is known only because
        ${ns.length} reporting router${ns.length === 1 ? "" : "s"} list${ns.length === 1 ? "s" : ""} a link to it.</p>
      <dl class="d-facts"><dt>ZID</dt><dd>${esc(selected)}</dd></dl>
      ${linkList(ns)}`;
    wire(box);
    $("#detail-note").textContent = displayName(selected);
    return;
  }

  const role = roleOf(x);
  const mism = x.route_successor_count !== x.route_successor_expected;
  const ns = neighbours(d, selected, x);
  const fs = d.findings.filter(f => names(f, selected));
  const cs = d.changes.filter(c => c.node === selected);
  const regions = (x.regions || []).map(r =>
    `<div class="regionline"><b>${esc(r.name)}</b> ${r.link_state_v} routers / ${r.link_state_e} links`
    + (r.link_state_e ? "" : ` <span class="dead">· carries no routes</span>`) + `</div>`).join("")
    || `<span class="sub">none reported</span>`;
  const errs = [x.observe_error, x.gateway_read_error].filter(Boolean)
    .map(e => `<div class="sub" style="color:var(--status-err-fg)">${esc(e)}</div>`).join("");

  box.innerHTML = `
    <div class="d-head">
      <span class="d-name">${esc(displayName(selected))}</span>
      <span class="sub">${esc(x.site)}</span>
      <span class="chip chip--${role === "gateway" ? "warn" : "muted"}">${esc(role)}</span>
      ${x.stale ? `<span class="chip chip--err">gone quiet</span>` : ""}
      <button type="button" class="pick${picks.has(selected) ? " on" : ""}" data-carve="${esc(selected)}">
        ${picks.has(selected) ? "in the carve plan" : "add to the carve plan"}</button>
    </div>
    <div class="d-grid">
      <div>
        <h3 class="t-label">Identity and state</h3>
        <dl class="d-facts">
          <dt>Site / node</dt><dd>${esc(x.site)} / ${esc(x.node || "—")}</dd>
          <dt>ZID</dt><dd>${esc(x.zid)}</dd>
          <dt>Zenoh</dt><dd>${esc(x.zenoh_version || "—")}</dd>
          <dt>Declared mode</dt><dd>${esc(x.gateway_mode)}</dd>
          <dt>Last report</dt><dd${x.stale ? ' style="color:var(--status-err-fg)"' : ""}>${ageSecs(x.received_ms, now)}s ago</dd>
          <dt>Applications</dt><dd>${x.sessions_total} connections held</dd>
          <dt>Route entries</dt><dd${mism ? ' style="color:var(--status-warn-fg)"' : ""}>${x.route_successor_count}${
            mism ? ` (predicted ${x.route_successor_expected})` : ""}</dd>
          <dt>Routing groups</dt><dd>${regions}</dd>
        </dl>${errs}
      </div>
      <div>
        <h3 class="t-label">Connected to</h3>
        ${linkList(ns)}
      </div>
    </div>
    <h3 class="t-label">Findings that name this router</h3>
    ${fs.length ? fs.map(f => `<div class="d-row">
        <span class="chip chip--${severityChip(f.severity)}">${esc(f.severity)}</span>
        <span class="f-id">${esc(f.id)}</span><span>${esc(f.headline)}</span></div>`).join("")
      : `<div class="sub">none</div>`}
    <h3 class="t-label">Changes it made</h3>
    ${cs.length ? cs.map(c => `<div class="d-row"><time class="sub">${new Date(c.ts_unix_ms).toLocaleTimeString()}</time>
        <span>${esc(c.what)}</span></div>`).join("")
      : `<div class="sub">none since this surface started watching</div>`}`;
  wire(box);
  $("#detail-note").textContent = `${displayName(selected)} · ${ns.length} link${ns.length === 1 ? "" : "s"} · ${fs.length} finding${fs.length === 1 ? "" : "s"}`;
}

function linkList(ns) {
  if (!ns.length) return `<div class="sub">no router links reported</div>`;
  return ns.map(n => `<div class="d-row">
      <i class="${n.kind === "routed" ? "k-north" : "k-south"}"></i>
      <a href="#detail" class="d-link" data-select="${esc(n.other)}">${esc(displayName(n.other))}</a>
      <span class="sub">${esc(n.kind)} · ${esc(n.reads)}${n.reporting ? "" : " · not reporting"}</span>
    </div>`).join("");
}

function wire(box) {
  for (const a of box.querySelectorAll("[data-select]")) {
    a.onclick = ev => { ev.preventDefault(); onSelect(a.dataset.select); };
  }
  for (const b of box.querySelectorAll("[data-carve]")) {
    b.onclick = () => onCarve(b.dataset.carve);
  }
}
