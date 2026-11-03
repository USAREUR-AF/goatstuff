// SPDX-License-Identifier: Apache-2.0
// What a carve would cost, on the measured model.
//
// Pure arithmetic over the same `terms()` the server uses, so the page and the
// findings engine cannot disagree about the cost of a shape.
import { $, esc, terms } from "../lib.js";

let onPick = () => {};
export function onSelectionChange(fn) { onPick = fn; }

/** The projection itself, separated from rendering so it is readable and
 *  testable on its own. `n` routers total, `k` of them moved into one
 *  subregion below a gateway that stays in both. */
export function projectCarve(n, k, largestNow) {
  if (k === 0 || k === n) {
    return {
      cells: [
        { v: largestNow, k: "largest table now" },
        { v: terms(n, n > 1 ? 1 : 0), k: `flat at ${n} routers` },
        { v: "—", k: "after carve" },
        { v: "—", k: "at the gateway" },
      ],
      caveat: k === 0
        ? "Select routers above, or click them in the graph, to model a subregion."
        : "Selecting every router is the same as selecting none — a region needs something left outside it.",
    };
  }
  const south = terms(k + 1, 1);          // the carved group, plus the gateway
  const north = terms(n - k, n - k > 1 ? 1 : 0); // what stays, gateway included
  const member = Math.max(south, north);
  const gateway = south + north;
  return {
    cells: [
      { v: largestNow, k: "largest table now" },
      { v: terms(n, n > 1 ? 1 : 0), k: `flat at ${n} routers` },
      { v: member, k: "a member after carve", cls: member < largestNow ? "better" : "" },
      { v: gateway, k: "the gateway", cls: gateway > member ? "worse" : "" },
    ],
    caveat: `A member's table falls to its own group's size. The gateway pays for BOTH groups — `
      + `${south} + ${north} = ${gateway} — because it holds one table per group it fronts. `
      + `That concentration is the thing to size for; do not put a gateway on the smallest host at a site.`,
  };
}

export function renderPlanner(d, picks) {
  const names = d.routers.map(r => r.node || r.zid);
  for (const p of [...picks]) if (!names.includes(p)) picks.delete(p);

  $("#picks").innerHTML = names.map(n =>
    `<span class="pick${picks.has(n) ? " on" : ""}" data-n="${esc(n)}">${esc(n)}</span>`).join("");
  for (const el of $("#picks").querySelectorAll(".pick")) {
    el.onclick = () => onPick(el.dataset.n);
  }

  const largestNow = Math.max(0, ...d.routers.map(r => r.route_successor_expected));
  const { cells, caveat } = projectCarve(names.length, picks.size, largestNow);

  $("#planout").innerHTML = cells.map(o =>
    `<div class="po ${o.cls || ""}"><b>${o.v}</b><span>${esc(o.k)}</span></div>`).join("");
  $("#plancaveat").innerHTML = esc(caveat)
    + ` <span style="color:var(--fg-faint)">Arithmetic on a model measured at two groups of `
    + `three routers on one host, and every goatnet today has V ≤ 3 — so this is a projection `
    + `for planning, not a measurement. It counts routing state only: nothing here predicts `
    + `throughput or latency.</span>`;
}
