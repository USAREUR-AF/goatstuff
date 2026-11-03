// SPDX-License-Identifier: Apache-2.0
//
// Web-native `goat top` page (ADR 1046). Renders /api/top verbatim —
// every honesty decision (stale-collapse, unmetered, band `—`, scoped
// row filtering) is made server-side, so this file only draws. A 403 or
// scoped response renders an explicit state, never a blank or a fake.

import "./styles.css";
import "./pages.css";
import {
  handleResponse,
  isPageSignedOut,
  pacingFailures,
  pollWithBackoff,
  shouldPoll,
} from "./session.js";

const POLL_MS = 2000;
/** Freshness gate mirrors the COP fleet bar: stale after 4× poll or two
 * consecutive failures. */
const STALE_AFTER_MS = POLL_MS * 4;

interface Chip {
  domain: string;
  state: string;
  sample_count: number;
  stale_age?: string;
}
interface ClassRow {
  class: string;
  label: string;
  sample_rate: number;
  target_sample_rate: number;
  ratio: number;
  color: string;
}
type ClassPane =
  | { state: "absent" }
  | { state: "stale"; age: string }
  | { state: "rows"; rows: ClassRow[] };
interface ProducerRow {
  pos: number;
  anchor: string;
  display: string;
  sample_rate: number;
  byte_rate: number;
  shares: string;
  near_limit: boolean;
  band_label: string;
  band_color: string;
  path: string;
  latency: string;
}
interface TopView {
  scope_tier: string;
  restricted: boolean;
  chips: Chip[];
  classes: ClassPane;
  site?: string;
  fabric_active_producers?: number;
  producers: ProducerRow[];
  totals: {
    visible: number;
    total_msgs: number;
    total_bytes: number;
    direct: number;
    relayed: number;
    alive: number;
    dropped: number;
  };
  generated_at_ms: number;
}

const $ = <T extends HTMLElement>(id: string): T =>
  document.getElementById(id) as T;

let lastSuccessAt = 0;
let consecutiveFailures = 0;
/** Consecutive 403s. Apart from consecutiveFailures so the banner keeps saying
 *  "restricted"; read by the schedule through pacingFailures (session.ts). */
let restrictedStreak = 0;
let filter = "";
let lastView: TopView | null = null;

function stateClass(state: string): string {
  switch (state) {
    case "green": return "state-green";
    case "amber": return "state-amber";
    case "red": return "state-red";
    case "cyan": return "state-cyan";
    default: return "state-dim"; // idle / unmetered / "" — honest dim
  }
}

function esc(s: string): string {
  const d = document.createElement("span");
  d.textContent = s;
  return d.innerHTML;
}

function fmtRate(n: number): string {
  return n >= 100 ? n.toFixed(0) : n >= 10 ? n.toFixed(1) : n.toFixed(2);
}

function fmtBytes(n: number): string {
  if (n >= 1_048_576) return `${(n / 1_048_576).toFixed(1)} MiB/s`;
  if (n >= 1024) return `${(n / 1024).toFixed(1)} KiB/s`;
  return `${n.toFixed(0)} B/s`;
}

function renderChips(chips: Chip[]) {
  $("env-strip").innerHTML = chips
    .map((c) => {
      const stale = c.stale_age ? ` <span class="meta">(stale ${esc(c.stale_age)})</span>` : "";
      return `<div class="env-chip">
        <div class="domain">${esc(c.domain)}</div>
        <div class="state ${stateClass(c.state)}">${esc(c.state)}</div>
        <div class="meta">n=${c.sample_count}${stale}</div>
      </div>`;
    })
    .join("");
}

function renderClasses(pane: ClassPane) {
  const el = $("class-pane");
  if (pane.state === "absent") {
    el.innerHTML = `<p class="class-note state-dim">unmetered — no fabric-health beat</p>`;
    return;
  }
  if (pane.state === "stale") {
    el.innerHTML = `<p class="class-note state-dim">stale (${esc(pane.age)}) — bars hidden, not faked</p>`;
    return;
  }
  el.innerHTML = pane.rows
    .map((c) => {
      const target =
        c.target_sample_rate > 0
          ? `${fmtRate(c.sample_rate)}/${fmtRate(c.target_sample_rate)} msg/s`
          : `${fmtRate(c.sample_rate)} msg/s`;
      const barTone = ["green", "amber", "red"].includes(c.color) ? c.color : "green";
      return `<div class="class-row">
        <span>${esc(c.class)}</span>
        <span class="state-dim">${esc(c.label)}</span>
        <span class="class-bar class-bar-${barTone}"><div style="width:${Math.round(c.ratio * 100)}%"></div></span>
        <span class="${stateClass(c.color)}">${esc(target)} · ${Math.round(c.ratio * 100)}%</span>
      </div>`;
    })
    .join("");
}

function renderProducers(view: TopView) {
  const needle = filter.trim().toLowerCase();
  const rows = view.producers.filter(
    (p) =>
      !needle ||
      p.display.toLowerCase().includes(needle) ||
      p.anchor.toLowerCase().includes(needle),
  );
  $("producers-title").textContent = needle
    ? `PRODUCERS (${rows.length} of ${view.producers.length} shown · filter "${filter}")`
    : `PRODUCERS (${view.producers.length} visible)`;

  $("producer-rows").innerHTML = rows
    .map(
      (p) => `<tr>
        <td>${p.pos}</td>
        <td title="${esc(p.anchor)}">${esc(p.display)}</td>
        <td class="num">${fmtRate(p.sample_rate)}</td>
        <td class="num">${esc(fmtBytes(p.byte_rate))}</td>
        <td class="${stateClass(p.band_color)}">${esc(p.band_label)}</td>
        <td>${esc(p.shares)}</td>
        <td class="state-amber">${p.near_limit ? "!" : ""}</td>
        <td>${esc(p.path)}</td>
        <td class="num">${esc(p.latency)}</td>
      </tr>`,
    )
    .join("");

  const empty = $("producer-empty");
  if (rows.length === 0) {
    empty.hidden = false;
    empty.textContent = needle
      ? "no producers match the filter"
      : view.restricted
        ? "no producers visible at your role's scope"
        : "no producer footprints in this window";
  } else {
    empty.hidden = true;
  }
}

function renderTotals(view: TopView) {
  const t = view.totals;
  const fleet =
    view.fabric_active_producers != null
      ? ` · fleet ${view.fabric_active_producers} producers (meter)`
      : "";
  $("totals").textContent =
    `visible ${t.visible} · ${fmtRate(t.total_msgs)} msg/s · ${fmtBytes(t.total_bytes)}` +
    ` · links ${t.direct} direct / ${t.relayed} relayed · presence ${t.alive} alive / ${t.dropped} dropped${fleet}`;
}

function renderScopeNote(view: TopView) {
  const note = $("scope-note");
  if (view.restricted) {
    note.hidden = false;
    note.textContent =
      `scoped view (${view.scope_tier}): this table shows only producers your role may see — ` +
      `it is not the whole fabric. Fleet aggregates (class load, counts) are unscoped.`;
  } else {
    note.hidden = true;
  }
}

function render(view: TopView) {
  renderScopeNote(view);
  renderChips(view.chips);
  renderClasses(view.classes);
  renderProducers(view);
  renderTotals(view);
}

function setConn(state: "live" | "stale" | "restricted" | "connecting", label: string) {
  const el = $("conn");
  el.dataset.state = state;
  el.textContent = label;
  el.className = `connection-indicator freshness${state === "stale" ? " stale" : ""}`;
}

function showFullError(title: string, body: string) {
  $("main").hidden = true;
  const el = $("full-error");
  el.hidden = false;
  el.innerHTML = `<h2>${esc(title)}</h2><p>${esc(body)}</p><p><a href="/">← back to the COP</a></p>`;
}

async function poll() {
  // A hidden tab has no reader, and a signed-out page has already said so —
  // neither earns a request. The timer keeps ticking and this returns
  // immediately, so the page resumes on its own when the viewer comes back.
  if (!shouldPoll(document.hidden, isPageSignedOut())) return;
  try {
    const res = await fetch("/api/top", { signal: AbortSignal.timeout(8000) });
    // 401 is a dead session and 403 is a live one whose role covers nothing
    // here: the first is fixed by signing in, the second never is.
    if (handleResponse(res)) return;
    if (res.status === 403) {
      // Explicit restricted state (never a blank) — the sidecar
      // authenticated the viewer but their role covers nothing here.
      showFullError(
        "Restricted by your role",
        "Your signed-in role does not include access to this view (ADR 1073). " +
          "Ask an administrator if you believe you need it.",
      );
      setConn("restricted", "restricted");
      restrictedStreak += 1;
      return;
    }
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const view = (await res.json()) as TopView;
    lastView = view;
    lastSuccessAt = Date.now();
    consecutiveFailures = 0;
    restrictedStreak = 0;
    $("main").hidden = false;
    $("full-error").hidden = true;
    render(view);
  } catch {
    consecutiveFailures += 1;
  } finally {
    const age = Date.now() - lastSuccessAt;
    if (lastSuccessAt === 0) {
      if (consecutiveFailures >= 2) setConn("stale", "unreachable");
    } else if (consecutiveFailures >= 2 || age > STALE_AFTER_MS) {
      setConn("stale", `STALE · ${Math.round(age / 1000)}s ago`);
    } else {
      setConn("live", `live · ${Math.round(age / 1000)}s ago`);
    }
  }
}

$("producer-filter").addEventListener("input", (e) => {
  filter = (e.target as HTMLInputElement).value;
  if (lastView) renderProducers(lastView);
});

void poll();
// Not setInterval: a failing portal must not be polled every POLL_MS forever by
// every open tab. pollWithBackoff widens the gap while consecutiveFailures is
// non-zero and snaps back to POLL_MS on the first success. A 403 widens it too:
// see pacingFailures.
pollWithBackoff(POLL_MS, poll, () => pacingFailures(consecutiveFailures, restrictedStreak));
