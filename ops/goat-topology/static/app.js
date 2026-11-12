// SPDX-License-Identifier: Apache-2.0
//
// Entry point: poll, decide whether anything changed, and orchestrate the
// section renderers. It owns no markup of its own.
//
// THE POLL IS CHEAP BY DESIGN. The server builds its payload on the write path
// and tags it with an ETag; a settled fabric returns 304 with no body, and this
// loop then does nothing at all — no parse, no DOM work. That matters because
// the common case for a routing plane is that nothing is happening, and a page
// that rebuilt itself every fifteen seconds anyway would cost an operator's
// scroll position and focus for no information.

import { $ } from "./lib.js";
import { renderVerdict } from "./render/verdict.js";
import { renderGraph, onNodeClick } from "./render/graph.js";
import { renderFindings, renderFindingFilters, onFilterChange } from "./render/findings.js";
import { renderFeed } from "./render/feed.js";
import { renderRouters, onRouterClick } from "./render/routers.js";
import { renderDetail, onDetailSelect, onDetailCarve } from "./render/detail.js";
import { renderPlanner, onSelectionChange } from "./render/planner.js";

const POLL_MS = 15000;
/** Ages tick locally between payloads, so refresh just those cells. */
const AGE_MS = 5000;

const picks = new Set();
/** The one router whose detail is open. A view concern, like `picks`. */
let selected = null;
let etag = null;
let latest = null;

function paint() {
  if (!latest) return;
  renderVerdict(latest);
  renderGraph(latest, picks, selected);
  renderDetail(latest, selected, picks);
  renderFindingFilters(latest);
  renderFindings(latest);
  renderFeed(latest);
  renderRouters(latest);
  renderPlanner(latest, picks);

  const sites = [...new Set(latest.routers.map(r => r.site).filter(Boolean))];
  $("#scope").textContent = sites.length ? sites.join(" · ") : "no site reporting";
}

// Selection is a view concern: it changes what the graph and planner draw, and
// never re-fetches.
function togglePick(node) {
  picks.has(node) ? picks.delete(node) : picks.add(node);
  renderGraph(latest, picks, selected);
  renderDetail(latest, selected, picks);
  renderPlanner(latest, picks);
}
// One click on a router, in the graph or the table, opens everything the page
// holds about it. Clicking the open one closes it. Carve membership moved to a
// button in the detail and stays on the planner's own chips.
function select(node) {
  selected = selected === node ? null : node;
  renderGraph(latest, picks, selected);
  renderDetail(latest, selected, picks);
  if (selected) $("#detail").scrollIntoView({ block: "nearest", behavior: "smooth" });
}
onNodeClick(select);
onRouterClick(select);
onDetailSelect(node => { selected = null; select(node); });
onDetailCarve(togglePick);
onSelectionChange(togglePick);
onFilterChange(() => { renderFindingFilters(latest); renderFindings(latest); });

async function poll() {
  try {
    const res = await fetch("/api/topology", {
      cache: "no-store",
      headers: etag ? { "If-None-Match": etag } : {},
    });

    if (res.status === 304) {
      $("#clock").textContent = "checked " + new Date().toLocaleTimeString() + " · no change";
      return;
    }
    if (res.status === 503) {
      $("#clock").textContent = "waiting for the first report";
      return;
    }
    if (!res.ok) throw new Error("HTTP " + res.status);

    etag = res.headers.get("ETag");
    latest = await res.json();
    paint();
    $("#clock").textContent = "updated " + new Date().toLocaleTimeString();
  } catch (e) {
    $("#clock").textContent = "refresh failed — " + e.message;
  }
}

poll();
setInterval(poll, POLL_MS);
// Ages are computed from absolute timestamps, so they advance without the
// server having to send a new payload.
setInterval(() => { if (latest) { renderRouters(latest); renderDetail(latest, selected, picks); } }, AGE_MS);
