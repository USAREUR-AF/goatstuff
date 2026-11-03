// SPDX-License-Identifier: Apache-2.0
// The one line that says whether anything is wrong, so nobody has to read the
// graph to find out.
import { $, esc } from "../lib.js";

export function renderVerdict(d) {
  const c = d.counts;
  const cls = c.critical ? "is-crit" : c.warning ? "is-warn" : "is-ok";
  const head = c.critical
    ? `${c.critical} thing${c.critical === 1 ? "" : "s"} need attention`
    : c.warning
    ? `${c.warning} thing${c.warning === 1 ? "" : "s"} worth a look`
    : "Nothing is wrong that this surface can see";
  const detail = c.critical
    ? "Critical findings are configuration that is unsafe, or that silently does the opposite of its intent. Start at the top."
    : c.warning
    ? "No critical findings. Warnings are inconsistencies and unknowns — often a rollout mid-flight, sometimes not."
    : `${c.routers} router${c.routers === 1 ? "" : "s"} reporting, ${c.stale} stale. `
      + `This states what was observed; a router that stopped reporting is listed as stale, never counted healthy.`;

  $("#verdict").className = "verdict " + cls;
  $("#verdict").innerHTML =
    `<div><div class="headline">${esc(head)}</div>
       <div class="detail">${esc(detail)}</div></div>
     <div class="counts">
       <div class="tally crit"><b>${c.critical}</b><span>critical</span></div>
       <div class="tally warn"><b>${c.warning}</b><span>warning</span></div>
       <div class="tally"><b>${c.routers}</b><span>routers</span></div>
       <div class="tally"><b>${c.clients}</b><span>clients</span></div>
     </div>`;
}
