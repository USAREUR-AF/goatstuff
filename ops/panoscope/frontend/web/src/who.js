// SPDX-License-Identifier: Apache-2.0
//
// Web-native `goat who` page (ADR 1047) — the identity COP roster.
// Group-gated server-side (/api/who/* returns 403 outside the
// identity-administration groups); this file renders the server's
// derived rows verbatim and shows explicit restricted / portal-down
// states, never a blank or a silently-empty roster.

import {
  handleResponse,
  isPageSignedOut,
  pacingFailures,
  pollWithBackoff,
  shouldPoll,
} from "./session.js";

const POLL_MS = 10_000;
const STALE_AFTER_MS = POLL_MS * 4;

/**
 * @typedef {Object} WhoRow
 * @property {string} anchor
 * @property {string} name
 * @property {string} kind
 * @property {string} affiliation
 * @property {string} nation
 * @property {string} wire_state
 * @property {string} recon_reason
 * @property {string} status
 * @property {string} status_color
 * @property {boolean} on_mesh
 * @property {boolean} connected
 * @property {number} last_seen_unix
 * @property {number} writes
 * @property {number} reads
 * @property {string} onboarding_stage Furthest onboarding step the portal reports positive evidence for. Empty
 *   = the portal reported nothing (no progress row, or a portal predating the
 *   field) — render `unknown`, never "did not start".
 * @property {string} live
 * @property {string} [live_age]
 * @property {number} [drop_count]
 * @property {number} [sample_rate]
 * @property {number} [byte_rate]
 * @property {string} shares
 * @property {boolean} near_limit
 * @property {string} band_label
 * @property {string} band_color
 */
/**
 * @typedef {Object} WhoView
 * @property {string} portal_state
 * @property {string} [roster_age]
 * @property {WhoRow[]} rows
 * @property {{ rows: number; online: number; strays: number; cut_off: number; no_grants: number; unfinished_onboarding: number; }} totals
 * @property {string[]} notes
 * @property {{ generated_at: string; member_count: number; }} [freshness]
 * @property {number} generated_at_ms
 */

const $ = (id) => document.getElementById(id);

let lastSuccessAt = 0;
let consecutiveFailures = 0;
/** Consecutive 403s. Apart from consecutiveFailures so the banner keeps saying
 *  "restricted"; read by the schedule through pacingFailures (session.ts). */
let restrictedStreak = 0;
let filter = "";
let selected = null;
let lastView = null;

function esc(s) {
  const d = document.createElement("span");
  d.textContent = s;
  return d.innerHTML;
}

function stateClass(color) {
  switch (color) {
    case "green": return "state-green";
    case "amber": return "state-amber";
    case "red": return "state-red";
    case "cyan": return "state-cyan";
    default: return "state-dim";
  }
}

/** The verify-first stray marker mirrors the TUI: an unregistered-key
 * stray renders `stray?` — flag before any cut. */
// Onboarding stage cell. The empty string is the important case: it means the
// portal reported no stage — either no progress row, or a portal older than the
// field — and that is UNKNOWN, not "did not start". Rendering it as a dimmed
// "unknown" keeps the difference visible instead of laundering a gap in
// instrumentation into a claim about a person.
function onboardingCell(r) {
  if (!r.onboarding_stage) {
    return '<span class="state-dim" title="The portal reported no onboarding stage for this anchor — no progress row, or a portal predating the field. This is unknown, not a claim that they never started.">unknown</span>';
  }
  if (r.onboarding_stage === "publishing") {
    return `<span class="state-dim">${esc(r.onboarding_stage)}</span>`;
  }
  // Short of publishing: worth the eye, not an alarm — an onboarding can
  // legitimately sit mid-ladder. Amber says "look", not "broken".
  return `<span class="state-amber" title="Furthest step with positive evidence. Short of publishing.">${esc(r.onboarding_stage)}</span>`;
}

function statusLabel(r) {
  if (r.status === "stray" && r.recon_reason === "unregistered-key") return "stray?";
  return r.status;
}

function fmtLastSeen(unix) {
  if (unix <= 0) return "—";
  const s = Math.floor(Date.now() / 1000) - unix;
  if (s < 0) return "—";
  if (s < 90) return `${s}s ago`;
  if (s < 5400) return `${Math.floor(s / 60)}m ago`;
  if (s < 129_600) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86_400)}d ago`;
}

function liveCell(r) {
  if (r.live === "alive") return `<span class="state-green">alive</span>`;
  if (r.live === "dropped")
    return `<span class="state-amber">dropped ×${r.drop_count ?? 0}</span>`;
  return `<span class="state-dim">unmetered</span>`;
}

function rowMatches(r, needle) {
  return (
    r.name.toLowerCase().includes(needle) ||
    r.anchor.toLowerCase().includes(needle) ||
    r.status.toLowerCase().includes(needle) ||
    r.kind.toLowerCase().includes(needle) ||
    r.affiliation.toLowerCase().includes(needle) ||
    r.nation.toLowerCase().includes(needle)
  );
}

function render(view) {
  const note = $("portal-note");
  if (view.portal_state === "disabled") {
    note.hidden = false;
    note.textContent =
      "portal not configured at this deployment — the roster requires the portal; live overlay only";
  } else if (view.portal_state === "stale") {
    note.hidden = false;
    note.textContent = `portal unreachable — showing the last good roster (${view.roster_age ?? "age unknown"})`;
  } else {
    note.hidden = true;
  }

  const needle = filter.trim().toLowerCase();
  const rows = view.rows.filter((r) => !needle || rowMatches(r, needle));
  $("roster-title").textContent = needle
    ? `ROSTER (${rows.length} of ${view.rows.length} shown · search "${filter}")`
    : `ROSTER (${view.rows.length})`;

  $("who-rows").innerHTML = rows
    .map((r) => {
      const aff = r.affiliation || r.nation ? `${esc(r.affiliation)} ${esc(r.nation)}`.trim() : "·";
      const msgs = r.sample_rate != null ? r.sample_rate.toFixed(1) : "—";
      const sel = selected === r.anchor ? " class=\"selected\"" : "";
      return `<tr data-anchor="${esc(r.anchor)}"${sel}>
        <td title="${esc(r.anchor)}">${esc(r.name)}<div class="anchor-sub">${esc(shorten(r.anchor))}</div></td>
        <td>${esc(r.kind || "·")}</td>
        <td>${aff}</td>
        <td class="${stateClass(r.status_color)}">${esc(statusLabel(r))}</td>
        <td>${liveCell(r)}</td>
        <td class="num">${r.writes}</td>
        <td class="num">${r.reads}</td>
        <td>${onboardingCell(r)}</td>
        <td class="num">${msgs}${r.near_limit ? ' <span class="state-amber">!</span>' : ""}</td>
        <td class="${stateClass(r.band_color)}">${esc(r.band_label)}</td>
        <td class="state-dim">${esc(fmtLastSeen(r.last_seen_unix))}</td>
      </tr>`;
    })
    .join("");

  const empty = $("who-empty");
  if (rows.length === 0) {
    empty.hidden = false;
    empty.textContent = needle
      ? "no identities match the search"
      : view.portal_state === "disabled"
        ? ""
        : "the roster is empty";
  } else {
    empty.hidden = true;
  }

  const t = view.totals;
  const fresh = view.freshness
    ? ` · registry ${view.freshness.member_count} declared (${esc(view.freshness.generated_at)})`
    : "";
  $("who-totals").innerHTML =
    `${t.rows} identities · ${t.online} online · ${t.strays} strays · ` +
    `${t.cut_off} cut-off · ${t.no_grants} no-grants` +
    (t.unfinished_onboarding ? ` · ${t.unfinished_onboarding} never finished onboarding` : "") +
    `${fresh}`;
}

function shorten(anchor) {
  return /^[0-9a-f-]{32,36}$/i.test(anchor) ? `${anchor.slice(0, 4)}…` : anchor;
}

async function showDetail(anchor) {
  selected = anchor;
  if (lastView) render(lastView);
  const pane = $("who-detail");
  pane.hidden = false;
  const row = lastView?.rows.find((r) => r.anchor === anchor);
  $("detail-title").textContent = `DETAIL · ${row?.name ?? anchor}`;
  const body = $("detail-body");
  body.innerHTML = `<p class="state-dim">loading grants…</p>`;
  try {
    const res = await fetch(`/api/who/detail?id=${encodeURIComponent(anchor)}`, {
      signal: AbortSignal.timeout(15_000),
    });
    if (res.status === 403) {
      body.innerHTML = `<p class="state-amber">restricted by your role</p>`;
      return;
    }
    if (!res.ok) {
      body.innerHTML = `<p class="state-amber">portal query failed (HTTP ${res.status}) — retry from the row</p>`;
      return;
    }
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
    // COP-to-lens deep link (ADR 1090): jump to microscope focused on this
    // entity. Only rendered when the backend carries a microscope_link (i.e.
    // a microscope sibling is configured at this deployment).
    const microscopeBtn = w.microscope_link
      ? `<p class="who-microscope"><a class="microscope-link" href="${esc(w.microscope_link)}" target="_blank" rel="noopener">examine in microscope ↗</a></p>`
      : "";
    body.innerHTML = `<dl>
      <dt>anchor</dt><dd>${esc(anchor)}</dd>
      <dt>kind / state</dt><dd>${esc(w.kind || "·")} · ${esc(w.state || "·")}</dd>
      <dt>affiliation</dt><dd>${esc(w.affiliation || "·")} ${esc(w.nation || "")}</dd>
      ${w.recon_reason ? `<dt>recon</dt><dd class="state-amber">${esc(w.recon_reason)}</dd>` : ""}
      <dt>writes (${w.writes.length})</dt><dd>${grants(w.writes)}</dd>
      <dt>reads (${w.reads.length})</dt><dd>${grants(w.reads)}</dd>
      ${w.notes?.length ? `<dt>notes</dt><dd>${w.notes.map((n) => esc(n)).join("<br>")}</dd>` : ""}
    </dl>${microscopeBtn}`;
  } catch {
    body.innerHTML = `<p class="state-amber">portal query timed out — retry from the row</p>`;
  }
}

function setConn(state, label) {
  const el = $("conn");
  el.dataset.state = state;
  el.textContent = label;
  el.className = `connection-indicator freshness${state === "stale" ? " stale" : ""}`;
}

function showFullError(title, bodyText) {
  $("main").hidden = true;
  const el = $("full-error");
  el.hidden = false;
  el.innerHTML = `<h2>${esc(title)}</h2><p>${esc(bodyText)}</p><p><a href="/">← back to the COP</a></p>`;
}

async function poll() {
  // See top.ts: no request from a hidden tab, none once the session is gone.
  if (!shouldPoll(document.hidden, isPageSignedOut())) return;
  try {
    const res = await fetch("/api/who/roster", { signal: AbortSignal.timeout(15_000) });
    if (handleResponse(res)) return;
    if (res.status === 403) {
      showFullError(
        "Restricted by your role",
        "The identity roster requires an identity-administration role (ADR 1073). " +
          "Ask an administrator if you believe you need it.",
      );
      setConn("restricted", "restricted");
      restrictedStreak += 1;
      return;
    }
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const view = (await res.json());
    lastView = view;
    lastSuccessAt = Date.now();
    consecutiveFailures = 0;
    restrictedStreak = 0;
    $("main").hidden = false;
    $("full-error").hidden = true;
    render(view);
    applyInbound(view);
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

// ── inbound deep link ────────────────────────────────────────────────────
// `?identity=<handle>` (alias `?handle=`), per the cross-surface deep-link
// contract in docs/design/zenoh-surface-deep-links.md. Sent by any surface that
// shows a peer and wants to hand the operator the identity answerable for it —
// today the peerview demand + peers pages.
//
// WHY IT FILTERS RATHER THAN ONLY SELECTING. The caller has a HANDLE; a roster
// row is keyed by its portal anchor, which on a slot-pool goatnet is a UUID and
// not the handle at all. Filtering by the term always shows something true,
// selecting is an optimisation when the term picks out exactly one row, and a
// term that matches nothing SAYS SO instead of leaving the operator looking at
// a full roster wondering whether the link worked.
const INBOUND = new URLSearchParams(window.location.search);
const wantIdentity = INBOUND.get("identity") ?? INBOUND.get("handle") ?? "";
let inboundApplied = false;

function applyInbound(view) {
  if (!wantIdentity || inboundApplied) return;
  inboundApplied = true;
  const needle = wantIdentity.toLowerCase();
  const box = $("who-filter");
  box.value = wantIdentity;
  filter = wantIdentity;
  const matches = view.rows.filter((r) => rowMatches(r, needle));
  const exact = view.rows.find(
    (r) => r.anchor.toLowerCase() === needle || r.name.toLowerCase() === needle,
  );
  const note = $("inbound-note");
  if (matches.length === 0 && !exact) {
    note.hidden = false;
    note.textContent =
      `no identity in this roster matches "${wantIdentity}" — it may be declared ` +
      `in the registry without an entry here, or belong to another goatnet. ` +
      `Clear the search to see the whole roster.`;
  } else {
    note.hidden = true;
  }
  render(view);
  const pick = exact ?? (matches.length === 1 ? matches[0] : undefined);
  if (pick) void showDetail(pick.anchor);
}

$("who-filter").addEventListener("input", (e) => {
  filter = e.target.value;
  if (lastView) render(lastView);
});

$("who-table").addEventListener("click", (e) => {
  const tr = e.target.closest("tr[data-anchor]");
  if (tr) void showDetail(tr.dataset.anchor);
});

void poll();
// Not setInterval: a failing portal must not be polled every POLL_MS forever by
// every open tab. pollWithBackoff widens the gap while consecutiveFailures is
// non-zero and snaps back to POLL_MS on the first success. A 403 widens it too:
// see pacingFailures.
pollWithBackoff(POLL_MS, poll, () => pacingFailures(consecutiveFailures, restrictedStreak));
