// SPDX-License-Identifier: Apache-2.0
//
// Access COP + data catalog page (docs/design/access-cop-web.md).
// Catalog pane: any authenticated viewer (metadata is advisory, ADR 1028);
// live-rate cells render the server's verdict verbatim (null = out of
// your scope = "—", 0 = genuinely quiet). Access pane: the who/why/topic
// tools ride the who_groups gate server-side — a 403 renders an explicit
// restricted state in the pane, never a blank.

import { foldEmptyOrgs } from "./catalog-filter.js";
import { handleResponse, isPageSignedOut, pollWithBackoff, shouldPoll } from "./session.js";

const POLL_MS = 10_000;
const STALE_AFTER_MS = POLL_MS * 4;

/**
 * @typedef {Object} CatalogOrg
 * @property {string} prefix
 * @property {string} display_name
 * @property {string} [alias]
 * @property {boolean} synthetic
 * @property {number} [live_eps]
 * @property {number | null} [declared_topics]
 */
/**
 * @typedef {Object} CatalogView
 * @property {string} portal_state
 * @property {CatalogOrg[]} orgs
 * @property {boolean} labels_available
 * @property {number} generated_at_ms
 */
/**
 * @typedef {Object} CatalogTopic
 * @property {number} id
 * @property {string} key_pattern
 * @property {string} format
 * @property {string} schema_ref
 * @property {string} description
 * @property {number} publish_count
 * @property {number} [live_eps]
 */

const $ = (id) => document.getElementById(id);

let lastSuccessAt = 0;
let consecutiveFailures = 0;
let filter = "";
// Orgs with nothing declared are folded by default (catalog-filter.ts);
// the toggle under the title unfolds them. A search always looks at every
// org so a folded row can still be found by name.
let showEmpty = false;
let lastView = null;
const expanded = new Set();
const topicsCache = new Map();
let portalBase = "";

function esc(s) {
  const d = document.createElement("span");
  d.textContent = s;
  return d.innerHTML;
}

function rate(eps) {
  if (eps == null) return `<span class="state-dim">—</span>`;
  if (eps === 0) return `<span class="state-dim">quiet</span>`;
  const v = eps >= 10 ? eps.toFixed(0) : eps.toFixed(1);
  return `<span class="state-green">${v}</span>`;
}

function shorten(anchor) {
  return /^[0-9a-f-]{32,36}$/i.test(anchor) ? `${anchor.slice(0, 4)}…` : anchor;
}

function render(view) {
  const note = $("catalog-note");
  if (view.portal_state === "disabled") {
    note.hidden = false;
    note.textContent = "portal not configured at this deployment — the catalog requires the portal";
  } else {
    note.hidden = true;
  }

  const needle = filter.trim().toLowerCase();
  const { shown, folded } = foldEmptyOrgs(view.orgs);
  const pool = needle || showEmpty ? view.orgs : shown;
  const orgs = pool.filter(
    (o) =>
      !needle ||
      o.display_name.toLowerCase().includes(needle) ||
      o.prefix.toLowerCase().includes(needle) ||
      (o.alias ?? "").toLowerCase().includes(needle),
  );
  $("catalog-title").textContent = needle
    ? `DATA CATALOG (${orgs.length} of ${view.orgs.length} orgs · search "${filter}")`
    : showEmpty || folded.length === 0
      ? `DATA CATALOG (${view.orgs.length} orgs)`
      : `DATA CATALOG (${shown.length} of ${view.orgs.length} orgs · ${folded.length} with nothing declared folded)`;
  const toggle = $("catalog-fold");
  toggle.hidden = folded.length === 0;
  $("catalog-fold-count").textContent = String(folded.length);

  $("org-rows").innerHTML = orgs
    .map((o) => {
      const open = expanded.has(o.prefix);
      const orgRow = `<tr data-org="${esc(o.prefix)}" class="org-row">
        <td>${open ? "▾" : "▸"}</td>
        <td>${esc(o.display_name)}</td>
        <td class="anchor-sub" title="${esc(o.prefix)}">${esc(shorten(o.prefix))}</td>
        <td>${o.synthetic ? '<span class="state-dim">NOTIONAL</span>' : ""}</td>
        <td class="num">${rate(o.live_eps)}</td>
      </tr>`;
      if (!open) return orgRow;
      const topics = topicsCache.get(o.prefix);
      const inner =
        topics == null
          ? `<tr class="topic-sub"><td></td><td colspan="4" class="state-dim">loading topics…</td></tr>`
          : topics.length === 0
            ? `<tr class="topic-sub"><td></td><td colspan="4" class="state-dim">no declared topics</td></tr>`
            : topics
                .map(
                  (t) => `<tr class="topic-sub">
                    <td></td>
                    <td colspan="2" title="${esc(t.description)}"><code>${esc(t.key_pattern)}</code></td>
                    <td>${esc(t.format || "·")}${schemaLink(t)}</td>
                    <td class="num">${rate(t.live_eps)}</td>
                  </tr>`,
                )
                .join("");
      return orgRow + inner;
    })
    .join("");
}

function schemaLink(t) {
  if (!portalBase || !t.key_pattern) return "";
  const href = `${portalBase}/schema.html?key=${encodeURIComponent(t.key_pattern)}`;
  return ` <a href="${esc(href)}" target="_blank" rel="noopener">schema →</a>`;
}

async function toggleOrg(prefix) {
  if (expanded.has(prefix)) {
    expanded.delete(prefix);
  } else {
    expanded.add(prefix);
    if (!topicsCache.has(prefix)) {
      if (lastView) render(lastView); // show "loading…" immediately
      try {
        const res = await fetch(
          `/api/access/catalog/topics?org=${encodeURIComponent(prefix)}`,
          { signal: AbortSignal.timeout(15_000) },
        );
        if (res.ok) {
          const v = await res.json();
          topicsCache.set(prefix, v.topics);
        } else {
          topicsCache.set(prefix, []);
        }
      } catch {
        topicsCache.set(prefix, []);
      }
    }
  }
  if (lastView) render(lastView);
}

// --- access pane (who_groups-gated server-side) ------------------------------

function resultBox(html) {
  $("access-result").innerHTML = html;
}

function restrictedNote() {
  return `<p class="state-amber">restricted by your role — the access picture requires an identity-administration role (ADR 1073)</p>`;
}

async function gatedFetch(url) {
  const res = await fetch(url, { signal: AbortSignal.timeout(15_000) });
  if (res.status === 403) {
    resultBox(restrictedNote());
    return null;
  }
  if (!res.ok) {
    resultBox(`<p class="state-amber">query failed (HTTP ${res.status})</p>`);
    return null;
  }
  return res;
}

async function lookupWho(id) {
  if (!id) return;
  resultBox(`<p class="state-dim">looking up ${esc(id)}…</p>`);
  const res = await gatedFetch(`/api/who/detail?id=${encodeURIComponent(id)}`);
  if (!res) return;
  const w = await res.json();
  const grants = (list) =>
    list.length
      ? `<ul class="grant-list">${list
          .map(
            (g) =>
              `<li>${esc(g.topic)} <span class="grant-layer">· ${esc(g.layer)}${g.reason ? ` · ${esc(g.reason)}` : ""}</span></li>`,
          )
          .join("")}</ul>`
      : `<p class="state-dim">none</p>`;
  resultBox(`<dl>
    <dt>identity</dt><dd>${esc(w.resolved || id)} · ${esc(w.kind || "·")} · ${esc(w.state || "·")}</dd>
    <dt>affiliation</dt><dd>${esc(w.affiliation || "·")} ${esc(w.nation || "")}</dd>
    <dt>writes (${w.writes.length})</dt><dd>${grants(w.writes)}</dd>
    <dt>reads (${w.reads.length})</dt><dd>${grants(w.reads)}</dd>
    ${w.notes?.length ? `<dt>notes</dt><dd>${w.notes.map((n) => esc(n)).join("<br>")}</dd>` : ""}
  </dl>`);
}

async function traceWhy(id, key) {
  if (!id || !key) return;
  resultBox(`<p class="state-dim">tracing ${esc(id)} × ${esc(key)}…</p>`);
  const res = await gatedFetch(
    `/api/access/query/why?id=${encodeURIComponent(id)}&key=${encodeURIComponent(key)}`,
  );
  if (!res) return;
  const w = await res.json();
  const verdictClass = (v) => v.toLowerCase().includes("allow") ? "state-green" : "state-amber";
  resultBox(`<dl>
    <dt>write verdict</dt><dd class="${verdictClass(w.write_verdict)}">${esc(w.write_verdict || "·")}</dd>
    <dt>read verdict</dt><dd class="${verdictClass(w.read_verdict)}">${esc(w.read_verdict || "·")}</dd>
    <dt>trace (${w.steps.length} layers)</dt>
    <dd><ul class="grant-list">${w.steps
      .map(
        (s) =>
          `<li><strong>${esc(s.layer)}</strong> · <span class="${verdictClass(s.verdict)}">${esc(s.verdict)}</span> <span class="grant-layer">${esc(s.reason)}</span></li>`,
      )
      .join("")}</ul></dd>
    ${w.notes?.length ? `<dt>notes</dt><dd>${w.notes.map((n) => esc(n)).join("<br>")}</dd>` : ""}
  </dl>`);
}

async function queryTopic(key) {
  if (!key) return;
  resultBox(`<p class="state-dim">resolving ${esc(key)}…</p>`);
  const res = await gatedFetch(`/api/access/query/topic?key=${encodeURIComponent(key)}`);
  if (!res) return;
  const t = await res.json();
  const list = (
    rows,
  ) =>
    rows.length
      ? `<ul class="grant-list">${rows
          .map(
            (r) =>
              `<li>${esc(r.identity)} <span class="grant-layer">· ${esc(r.layer)}${r.reason ? ` · ${esc(r.reason)}` : ""}</span></li>`,
          )
          .join("")}</ul>`
      : `<p class="state-dim">none</p>`;
  resultBox(`<dl>
    <dt>topic</dt><dd>${esc(t.topic || key)}${t.found ? "" : ' <span class="state-dim">(no declared row — namespace query)</span>'}</dd>
    <dt>writers (${t.writers.length})</dt><dd>${list(t.writers)}</dd>
    <dt>readers (${t.readers.length})</dt><dd>${list(t.readers)}</dd>
    ${t.receivers?.length ? `<dt>release receivers</dt><dd>${t.receivers.map((r) => esc(r)).join(", ")}</dd>` : ""}
    ${t.notes?.length ? `<dt>notes</dt><dd>${t.notes.map((n) => esc(n)).join("<br>")}</dd>` : ""}
  </dl>`);
}

// --- freshness + polling ------------------------------------------------------

function setConn(state, label) {
  const el = $("conn");
  el.dataset.state = state;
  el.textContent = label;
  el.className = `connection-indicator freshness${state === "stale" ? " stale" : ""}`;
}

async function poll() {
  // See session.ts: a hidden tab has no reader, and a signed-out page has
  // already said so once.
  if (!shouldPoll(document.hidden, isPageSignedOut())) return;
  try {
    const res = await fetch("/api/access/catalog", { signal: AbortSignal.timeout(15_000) });
    if (handleResponse(res)) return;
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const view = (await res.json());
    lastView = view;
    lastSuccessAt = Date.now();
    consecutiveFailures = 0;
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

async function loadSiblings() {
  try {
    const res = await fetch("/api/legend", { signal: AbortSignal.timeout(8000) });
    if (res.ok) {
      const l = await res.json();
      portalBase = l.siblings?.portal ?? l.portal_base_url ?? "";
    }
  } catch {
    /* schema links simply hidden */
  }
}

$("catalog-filter").addEventListener("input", (e) => {
  filter = e.target.value;
  if (lastView) render(lastView);
});
$("catalog-show-empty").addEventListener("change", (e) => {
  showEmpty = e.target.checked;
  if (lastView) render(lastView);
});
$("org-table").addEventListener("click", (e) => {
  const tr = e.target.closest("tr.org-row");
  if (tr) void toggleOrg(tr.dataset.org);
});
$("who-go").addEventListener("click", () => void lookupWho($("who-id").value.trim()));
$("why-go").addEventListener("click", () =>
  void traceWhy($("why-id").value.trim(), $("why-key").value.trim()),
);
$("topic-go").addEventListener("click", () => void queryTopic($("topic-key").value.trim()));

// R.3 deep-link route: /access/identity?id=<identity_id> pre-fills and
// runs the identity lookup (ADR 1026 — the relay baseball card target).
const deepId = new URLSearchParams(location.search).get("id");
if (location.pathname.endsWith("/identity") && deepId) {
  $("who-id").value = deepId;
  void lookupWho(deepId);
}

void loadSiblings();
void poll();
// Not setInterval: a failing portal must not be polled every POLL_MS forever by
// every open tab. pollWithBackoff widens the gap while consecutiveFailures is
// non-zero and snaps back to POLL_MS on the first success.
pollWithBackoff(POLL_MS, poll, () => consecutiveFailures);
