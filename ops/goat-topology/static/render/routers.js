// SPDX-License-Identifier: Apache-2.0
// The detail table: declared configuration beside what is actually running.
import { $, esc, ageSecs } from "../lib.js";

function modeChip(x) {
  if (x.stale) return `<span class="chip chip--err">stale</span>`;
  if (x.gateway_mode === "auto") return `<span class="chip chip--muted">auto</span>`;
  if (x.gateway_mode === "custom") return `<span class="chip chip--warn">custom</span>`;
  return `<span class="chip chip--err">${esc(x.gateway_mode)}</span>`;
}

export function renderRouters(d, now = Date.now()) {
  const head = `<thead><tr><th>Router</th><th>Mode</th>
    <th>Regions <span class="sub">V/E</span></th>
    <th class="num">Routes</th><th class="num">Age</th></tr></thead>`;

  $("#routers").innerHTML = head + "<tbody>" + d.routers.map(x => {
    const regions = (x.regions || []).map(r =>
      `<div class="regionline"><b>${esc(r.name)}</b> ${r.link_state_v}/${r.link_state_e}`
      + (r.link_state_e ? "" : ` <span class="dead">· no links, carries no routes</span>`)
      + `</div>`).join("") || `<span class="sub">—</span>`;

    const mism = x.route_successor_count !== x.route_successor_expected;
    const errs = [x.observe_error, x.gateway_read_error].filter(Boolean)
      .map(e => `<div class="sub" style="color:var(--status-err-fg)">${esc(e)}</div>`).join("");
    const age = ageSecs(x.received_ms, now);

    return `<tr class="enter">
      <td><div class="name">${esc(x.node || x.zid)}</div>
          <div class="sub">${esc(x.site)}</div>${errs}</td>
      <td>${modeChip(x)}</td>
      <td>${regions}</td>
      <td class="num"${mism ? ' style="color:var(--status-warn-fg)"' : ""}>${x.route_successor_count}${
        mism ? `<div class="sub">pred ${x.route_successor_expected}</div>` : ""}</td>
      <td class="num"${x.stale ? ' style="color:var(--status-err-fg)"' : ""}>${age}s</td>
    </tr>`;
  }).join("") + "</tbody>";
}
