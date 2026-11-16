// SPDX-License-Identifier: Apache-2.0
// The detail table: declared configuration beside what is actually running.
import { $, esc, ageSecs, displayName, roleOf, labelOf, orgLine, ownerTone, carriesLine, NOT_RECORDED } from "../lib.js";

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
  // Organization and the ORGANIZATION's nation; the individual is only in the
  // detail a click opens.
  const org = x => orgLine(x)
    ? `<span style="color:${x.owner_state === "recorded" || x.owner_state === "lead" ? "inherit" : ownerTone(x)}">${esc(orgLine(x))}</span>`
      + (x.owner_reason ? `<div class="sub">${esc(x.owner_reason)}</div>` : "")
    : `<span class="sub">${esc(x.owner_reason || (x.reporting === false ? "not known" : NOT_RECORDED))}</span>`;
  const carries = x => { const c = carriesLine(x);
    return `<td><div${c.err ? ' style="color:var(--status-err-fg)"' : ""}>${esc(c.text)}</div><div class="sub">${esc(c.sub)}</div></td>`; };
  const head = `<thead><tr><th>Router</th><th>Organization <span class="sub">· nation</span></th><th>Mode</th>
    <th>Carries</th><th>Regions <span class="sub">V/E</span></th>
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
        ${carries(x)}
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
      ${carries(x)}
      <td>${regions}</td>
      <td class="num"${mism ? ' style="color:var(--status-warn-fg)"' : ""}>${x.route_successor_count}${
        mism ? `<div class="sub">pred ${x.route_successor_expected}</div>` : ""}</td>
      <td class="num"${x.stale ? ' style="color:var(--status-err-fg)"' : ""}>${age}s</td>
    </tr>`;
  }).join("") + "</tbody>";
  $("#names-note").textContent = namesNote(d, now);
  $("#load-note").textContent = LOAD_NOTE;
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
  if (!dir.roster_configured) parts.push("No roster is configured, so organization and nation are not shown.");
  else parts.push("Organization, nation and the individual are read from the identity roster, found through the peer name.");
  if (dir.roster_modified_ms && days(dir.roster_modified_ms, now) >= ROSTER_OLD_DAYS) {
    parts.push(`The roster on this host was written ${days(dir.roster_modified_ms, now)} days ago `
      + `(${new Date(dir.roster_modified_ms).toISOString().slice(0, 10)}); anyone enrolled since has no entry, so their routers show a name and no owner.`);
  }
  if (dir.peers_modified_ms && days(dir.peers_modified_ms, now) >= PEERS_OLD_DAYS) {
    parts.push(`The peer directory was last written ${days(dir.peers_modified_ms, now)} days ago, so a peer added since is not named.`);
  }
  const or = dir.owner_read;
  if (or && or.state === "unread") parts.push(`Owners could not be read on this host: ${or.reason}.`);
  else if (or && or.state === "read") parts.push("Owners are read from the identity record through the portal; a router whose owner the record does not hold is a critical finding.");
  for (const e of dir.errors || []) parts.push(e + ".");
  return parts.join(" ");
}

/** Said once, because it is true of every router on the page. */
export const LOAD_NOTE = "Counts are declarations, read from each reporting router's own tables every few minutes. "
  + "Messages and bytes are not shown: the router build exposes no traffic counters. "
  + "Traffic between two routers that both send no report is not knowable from here.";
