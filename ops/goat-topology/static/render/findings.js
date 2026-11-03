// SPDX-License-Identifier: Apache-2.0
// Findings, in the cold-read shape the engine produces: plain headline, then
// what we saw / why it matters / if nothing is done / numbered steps.
import { $, esc, severityChip } from "../lib.js";

const SEVS = ["all", "critical", "warning", "advisory", "info"];
let sev = "all";
let onChange = () => {};

export function onFilterChange(fn) { onChange = fn; }

export function renderFindingFilters(d) {
  const n = s => (s === "all" ? d.findings.length : d.findings.filter(f => f.severity === s).length);
  $("#filters").innerHTML = SEVS.map(s =>
    `<span class="tab${sev === s ? " on" : ""}" data-sev="${s}">${s} ${n(s)}</span>`).join("");
  for (const t of $("#filters").querySelectorAll(".tab")) {
    t.onclick = () => { sev = t.dataset.sev; onChange(); };
  }
}

export function renderFindings(d) {
  const list = sev === "all" ? d.findings : d.findings.filter(f => f.severity === sev);
  if (!list.length) {
    $("#findings").innerHTML =
      `<div class="finding sev-info">
         <p class="f-head">Nothing at this severity, from the reports currently held.</p>
       </div>`;
    return;
  }
  $("#findings").innerHTML = list.map(f => `
    <div class="finding sev-${esc(f.severity)} enter">
      <div class="f-top">
        <span class="chip chip--${severityChip(f.severity)}">${esc(f.severity)}</span>
        <span class="f-subject">${esc(f.subject)}</span>
        <span class="f-id">${esc(f.id)}</span>
      </div>
      <p class="f-head">${esc(f.headline)}</p>
      <dl class="f-body">
        <dt>What we saw</dt><dd>${esc(f.what)}</dd>
        <dt>Why it matters</dt><dd>${esc(f.why)}</dd>
        <dt>If nothing is done</dt><dd>${esc(f.impact)}</dd>
        <dt>What to do</dt>
        <dd><ol class="f-steps">${(f.steps || []).map(x => `<li>${esc(x)}</li>`).join("")}</ol></dd>
      </dl>
    </div>`).join("");
}
