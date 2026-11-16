// SPDX-License-Identifier: Apache-2.0
// One router, everything the page holds about it, in one place: identity,
// declared mode beside running state, its neighbours, the findings that name
// it and the changes it made. Selecting a router opens this; nothing here
// fetches — it is a second reading of the payload the page already has, so a
// drill-down costs the fabric nothing (ADR 1123).
import { $, esc, ageSecs, severityChip, displayName, roleOf, labelOf, labelFor, NOT_RECORDED } from "../lib.js";

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
    const hop = ((self && self.next_hops) || []).find(h => h.to === other);
    return {
      other,
      routes: hop ? hop.routes : null,
      label: labelFor(d, other),
      reporting: d.routers.some(r => key(r) === other && r.reporting !== false),
      kind: routed ? "routed" : "terminated",
      reads: routed ? "same routing group"
        : gateway ? "sits below this router" : "this router sits below it",
    };
  }).sort((a, b) => a.kind.localeCompare(b.kind) || a.other.localeCompare(b.other));
}

/** Who operates a router: the mesh peer it connects from and what the
 *  identity registry records for that peer. A person's nationality and an
 *  organisation's nation are two facts and are shown as two rows. */
export function ownerFacts(x, d = {}, now = Date.now()) {
  const rm = (d.directory || {}).roster_modified_ms;
  const rosterAge = rm ? ` in the roster written ${new Date(rm).toISOString().slice(0, 10)}` : "";
  const nr = `<span class="sub">${NOT_RECORDED}</span>`;
  const v = s => (s ? esc(s) : nr);
  const derived = `<dt>Display name</dt><dd>${esc(displayName(x.zid))} <span class="sub">derived from the ZID</span></dd>`;
  if (!x.peer) {
    return `${x.node ? "" : derived}
      ${x.address ? `<dt>Connects from</dt><dd>${esc(x.address)}</dd>` : ""}
      <dt>Operated by</dt><dd><span class="sub">not known — ${esc(x.name_note || "no reporting router sees its session")}</span></dd>`;
  }
  const p = x.peer, o = x.owner;
  const who = !o ? `<dt>Organisation</dt><dd>${nr} <span class="sub">— the peer name leads to no roster handle${rosterAge}</span></dd>`
    : `<dt>Organisation</dt><dd>${v(o.org_display_name || o.org)}${o.org_display_name ? ` <span class="sub">${esc(o.org)}</span>` : ""}</dd>
       <dt>Organisation nation</dt><dd>${v(o.org_nation)}</dd>
       <dt>${o.kind === "machine" ? "Enrolled by" : "Individual"}</dt><dd>${v(o.individual)}</dd>
       <dt>${o.kind === "machine" ? "Their nationality" : "Nationality"}</dt><dd>${o.individual ? v(o.individual_nationality) : nr}${
         o.kind === "machine" ? ` <span class="sub">a machine has none; this is the person who enrolled it</span>` : ""}</dd>
       <dt>Found by</dt><dd><span class="sub">the peer name led to roster handle ${esc(o.handle)}; a lead, not the recorded enrolment link</span></dd>`;
  return `${x.node ? "" : derived}
    <dt>Mesh peer</dt><dd>${esc(p.name)}</dd>
    <dt>Hostname</dt><dd>${v(p.hostname)}</dd>
    <dt>Groups</dt><dd>${p.groups && p.groups.length ? esc(p.groups.join(", ")) : `<span class="sub">none</span>`}</dd>
    <dt>Connects from</dt><dd>${esc(x.address)}</dd>
    ${who}`;
}

/** What a router carries: its own census, or what a reporter attributes to it. */
export function carriesFacts(x, now = Date.now()) {
  const keys = (ks, more) => ks && ks.length
    ? `<div class="sub">${ks.map(esc).join("<br>")}${more ? "<br>…" : ""}</div>` : "";
  const c = x.census;
  let out = "";
  if (c && c.read_error) {
    out += `<dt>Declarations</dt><dd style="color:var(--status-err-fg)">${esc(c.read_error)}</dd>`;
  } else if (c) {
    const age = Math.max(0, Math.round((now - c.read_unix_ms) / 1000));
    out += `<dt>Subscriptions</dt><dd>${c.session_subscriptions} by its applications, on ${c.session_subscribed_keys} keys</dd>
      <dt>Publishers</dt><dd>${c.session_publishers} by its applications, on ${c.session_published_keys} keys</dd>
      <dt>Topics</dt><dd>${c.topics}${c.capped ? " or more (entry cap hit)" : ""} <span class="sub">distinct key expressions, including other routers'</span>${keys(c.sample_keys, c.sample_truncated)}</dd>
      <dt>Router itself</dt><dd>${c.own_process_subscribed_keys} keys <span class="sub">its own adminspace and plugins</span></dd>
      <dt>Census</dt><dd>read ${age}s ago in ${c.read_ms} ms · every ${c.interval_secs}s</dd>`;
  }
  if (x.held && !(c && !c.read_error)) {
    out += `<dt>Held, as ${esc(x.held.by)} sees it</dt><dd>${x.held.subscribed_keys} subscribed keys
      <span class="sub">its applications', and anything below it; its publishers are not visible from another router</span>${keys(x.held.sample_keys, x.held.sample_truncated)}</dd>`;
  }
  if (!out) out = `<dt>Declarations</dt><dd><span class="sub">${x.reporting === false
    ? "no reporting router in its routing group has a census" : "this publisher sends no census"}</span></dd>`;
  return out;
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
  if (!x || x.reporting === false) {
    // Known from another router's link table, never from its own report. The
    // server lists such a router with `reporting: false` and the reporters
    // whose maps name it; an older server leaves it out of `routers` entirely.
    const ns = neighbours(d, selected, null);
    const from = (x?.known_from || []).map(displayName);
    box.innerHTML = `<div class="d-head"><span class="d-name">${esc(x ? labelOf(x) : displayName(selected))}</span>
        <span class="chip chip--muted">not reporting</span></div>
      <p class="d-plain">No report has arrived from this router. ${from.length
        ? `It is known only because the routing map of ${esc(from.join(", "))} lists it.`
        : `It is known only because ${ns.length} reporting router${ns.length === 1 ? "" : "s"} list${ns.length === 1 ? "s" : ""} a link to it.`}</p>
      <dl class="d-facts"><dt>ZID</dt><dd>${esc(selected)}</dd>
        ${x ? ownerFacts(x, d, now) : ""}
        ${x ? carriesFacts(x, now) : ""}
        ${x?.stale ? `<dt>State</dt><dd style="color:var(--status-err-fg)">every router that lists it has gone quiet</dd>` : ""}</dl>
      ${linkList(ns)}`;
    wire(box);
    $("#detail-note").textContent = x ? labelOf(x) : displayName(selected);
    return;
  }

  const role = roleOf(x);
  const unread = Boolean(x.route_table_note);
  const mism = !unread && x.route_successor_count !== x.route_successor_expected;
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
          <dt>Route entries</dt><dd${mism ? ' style="color:var(--status-warn-fg)"' : ""}>${unread
            ? `<span class="sub">${esc(x.route_table_note)}</span>`
            : `${x.route_successor_count}${mism ? ` (predicted ${x.route_successor_expected})` : ""}`}</dd>
          <dt>Routing groups</dt><dd>${regions}</dd>
          ${x.peer ? ownerFacts(x, d, now) : ""}
          ${carriesFacts(x, now)}
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
      <a href="#detail" class="d-link" data-select="${esc(n.other)}">${esc(n.label)}</a>
      <span class="sub">${esc(n.kind)} · ${esc(n.reads)}${n.reporting ? "" : " · not reporting"}${
        n.routes != null ? ` · ${n.routes} route entr${n.routes === 1 ? "y" : "ies"} via it` : ""}</span>
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
