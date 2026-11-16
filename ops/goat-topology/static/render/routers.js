// SPDX-License-Identifier: Apache-2.0
// The detail table: declared configuration beside what is actually running.
import { $, esc, ageSecs, displayName, roleOf, labelOf, orgLine, NOT_RECORDED } from "../lib.js";

let onPick = () => {};
export function onRouterClick(fn) { onPick = fn; }

function modeChip(x) {
  if (x.reporting === false) return `<span class="chip chip--muted">not reporting</span>`;
  if (x.stale) return `<span class="chip chip--err">stale</span>`;
  if (x.gateway_mode === "auto") return `<span class="chip chip--muted">auto</span>`;
  if (x.gateway_mode === "custom") return `<span class="chip chip--warn">custom</span>`;
  return `<span class="chip chip--err">${esc(x.gateway_mode)}</span>`;
}

export function renderRouters(d, now = Date.now()) {
  // Organisation and the ORGANISATION's nation; the individual is only in the
  // detail a click opens.
  const org = x => orgLine(x) ? esc(orgLine(x)) : `<span class="sub">${x.reporting === false ? "not known" : NOT_RECORDED}</span>`;
  const head = `<thead><tr><th>Router</th><th>Organisation <span class="sub">· nation</span></th><th>Mode</th>
    <th>Regions <span class="sub">V/E</span></th>
    <th class="num">Routes</th><th class="num">Age</th></tr></thead>`;

  $("#routers").innerHTML = head + "<tbody>" + d.routers.map(x => {
    // Known from another router's routing map only: say so, and leave every
    // number blank rather than print a zero that would read as a measurement.
    if (x.reporting === false) {
      return `<tr class="enter rowlink" data-node="${esc(x.zid)}">
        <td><div class="name">${esc(labelOf(x))}</div>
            <div class="sub">${x.peer ? esc(x.zid.slice(0, 8)) + " · " : ""}listed by ${esc((x.known_from || []).map(displayName).join(", "))}; sends no report</div></td>
        <td>${org(x)}</td>
        <td>${modeChip(x)}</td>
        <td><span class="sub">—</span></td>
        <td class="num"><span class="sub">—</span></td>
        <td class="num"${x.stale ? ' style="color:var(--status-err-fg)"' : ""}>${x.stale ? "stale" : "—"}</td>
      </tr>`;
    }
    const regions = (x.regions || []).map(r =>
      `<div class="regionline"><b>${esc(r.name)}</b> ${r.link_state_v}/${r.link_state_e}`
      + (r.link_state_e ? "" : ` <span class="dead">· no links, carries no routes</span>`)
      + `</div>`).join("") || `<span class="sub">—</span>`;

    const mism = x.route_successor_count !== x.route_successor_expected;
    const errs = [x.observe_error, x.gateway_read_error].filter(Boolean)
      .map(e => `<div class="sub" style="color:var(--status-err-fg)">${esc(e)}</div>`).join("");
    const age = ageSecs(x.received_ms, now);

    return `<tr class="enter rowlink" data-node="${esc(x.node || x.zid)}">
      <td><div class="name">${esc(displayName(x.node || x.zid))}</div>
          <div class="sub">${esc(x.site)} · ${esc(roleOf(x))}</div>${errs}</td>
      <td>${org(x)}</td>
      <td>${modeChip(x)}</td>
      <td>${regions}</td>
      <td class="num"${mism ? ' style="color:var(--status-warn-fg)"' : ""}>${x.route_successor_count}${
        mism ? `<div class="sub">pred ${x.route_successor_expected}</div>` : ""}</td>
      <td class="num"${x.stale ? ' style="color:var(--status-err-fg)"' : ""}>${age}s</td>
    </tr>`;
  }).join("") + "</tbody>";
  $("#names-note").textContent = namesNote(d, now);
  for (const tr of $("#routers").querySelectorAll("tr[data-node]")) {
    tr.onclick = () => onPick(tr.dataset.node);
  }
}

/** A roster older than this is called out: anyone enrolled since it was written
 *  has no roster entry, so their routers show a name and no owner. */
const ROSTER_OLD_DAYS = 7;
/** The peer directory is rewritten every few minutes; a day without is news. */
const PEERS_OLD_DAYS = 1;
const days = (ms, now) => Math.floor((now - ms) / 86400000);

/** Where names and owners come from, said once. Exported for a test. */
export function namesNote(d, now = Date.now()) {
  const dir = d.directory || {};
  const parts = [];
  if (!dir.peers_configured) parts.push("No peer directory is configured, so routers that send no report keep names derived from their ZIDs.");
  else parts.push("Routers that send no report are named from the mesh peer their session comes from.");
  if (!dir.roster_configured) parts.push("No roster is configured, so organisation and nation are not shown.");
  else parts.push("Organisation, nation and the individual are read from the identity roster, found through the peer name.");
  if (dir.roster_modified_ms && days(dir.roster_modified_ms, now) >= ROSTER_OLD_DAYS) {
    parts.push(`The roster on this host was written ${days(dir.roster_modified_ms, now)} days ago `
      + `(${new Date(dir.roster_modified_ms).toISOString().slice(0, 10)}); anyone enrolled since has no entry, so their routers show a name and no owner.`);
  }
  if (dir.peers_modified_ms && days(dir.peers_modified_ms, now) >= PEERS_OLD_DAYS) {
    parts.push(`The peer directory was last written ${days(dir.peers_modified_ms, now)} days ago, so a peer added since is not named.`);
  }
  for (const e of dir.errors || []) parts.push(e + ".");
  return parts.join(" ");
}
