// SPDX-License-Identifier: Apache-2.0
//
// Legend strip — bottom-of-page metadata + status panel. Refreshed
// every ~5s via GET /api/legend. The resolver-status indicator reports
// the compiled-in family-map floor (`family-map`); per-peer coloring
// rides schema-decl self-registration (see src/schema.rs).

import { handleResponse, isPageSignedOut, pollWithBackoff, shouldPoll } from "./session.js";

const REFRESH_MS = 5000;

export class LegendPanel {
  stopPolling = null;
  /** Consecutive failed refreshes. Drives the back-off, and is reset by the
   *  first refresh that completes. */
  consecutiveFailures = 0;

  start() {
    void this.fetchOnce();
    // Not setInterval: a failing /api/legend must not be re-requested every
    // REFRESH_MS forever by every open tab.
    this.stopPolling = pollWithBackoff(
      REFRESH_MS,
      () => this.fetchOnce(),
      () => this.consecutiveFailures,
    );
  }

  stop() {
    if (this.stopPolling !== null) {
      this.stopPolling();
      this.stopPolling = null;
    }
  }

  async fetchOnce() {
    // Nobody is reading a hidden tab, and a signed-out page has already said
    // so once — neither is worth a request.
    if (!shouldPoll(document.hidden, isPageSignedOut())) return;
    try {
      const resp = await fetch("/api/legend", { cache: "no-store" });
      if (handleResponse(resp)) {
        this.stop();
        return;
      }
      if (!resp.ok) {
        this.consecutiveFailures += 1;
        return;
      }
      const legend = (await resp.json());
      this.render(legend);
      this.consecutiveFailures = 0;
    } catch (e) {
      this.consecutiveFailures += 1;
      console.warn("legend: fetch failed", e);
    }
  }

  render(legend) {
    setText("legend-resolver-status", legend.schema_resolver_status);
    setText(
      "legend-resolver-families",
      String(legend.schema_resolver_known_families),
    );
    renderAliasRegistry(legend.alias_registry);
    setText("legend-drops", totalDrops(legend.sampling_drops).toString());
    setText("legend-ema-tau", legend.ema_tau_secs.toString());
    setText("legend-ws-coalesce", legend.ws_coalesce_ms.toString());
    setText(
      "legend-generated-at",
      new Date(legend.generated_at_ms).toLocaleTimeString(),
    );
  }
}

function setText(id, text) {
  const el = document.getElementById(id);
  if (el) el.textContent = text;
}

/** Render the slot-alias registry cell (ADR 1039 honesty arm).
 *
 *  The point of this cell is that "every peer shows a raw UUID" must never
 *  again be indistinguishable from "this goatnet assigns no aliases". Each
 *  state gets its own words, and the two that need an operator get a
 *  `legend-warn` class plus the reason in the title attribute.
 *
 *  A backend older than the honesty arm sends no `alias_registry` — the cell
 *  stays `—` rather than claiming a state we weren't told. */
function renderAliasRegistry(health) {
  const el = document.getElementById("legend-aliases");
  if (!el) return;

  if (!health) {
    el.textContent = "—";
    el.classList.remove("legend-warn");
    el.removeAttribute("title");
    return;
  }

  let text;
  let title;
  switch (health.state) {
    case "ok":
      text = health.aliases === 0 ? "none assigned" : String(health.aliases);
      title =
        health.aliases === 0
          ? "Portal reachable; this goatnet has no slot aliases assigned."
          : `${health.aliases} slot aliases resolved from the portal.`;
      break;
    case "disabled":
      text = "off — no portal key";
      title =
        "PANOSCOPE_PORTAL_API_KEY is unset, so GET /api/aliases is never " +
        "called and every peer renders its raw UUID. Deploy gap: set " +
        "panoscope_portal_api_key in group_vars and re-apply the role.";
      break;
    case "unreached":
      text = "portal unreachable";
      title = health.last_error
        ? `No successful alias poll yet: ${health.last_error}`
        : "No successful alias poll yet.";
      break;
    case "stale":
      text = `${health.aliases} (stale${ageSuffix(health.last_ok_ms)})`;
      title = health.last_error
        ? `Last poll failed (${health.last_error}); showing the last-good registry.`
        : "Last poll failed; showing the last-good registry.";
      break;
  }

  el.textContent = text;
  el.title = title;
  el.classList.toggle("legend-warn", health.state !== "ok");
}

/** ` 4m` / ` 2h` for a last-good timestamp; empty when we were never told. */
function ageSuffix(lastOkMs) {
  if (lastOkMs === undefined) return "";
  const secs = Math.max(0, Math.round((Date.now() - lastOkMs) / 1000));
  if (secs < 60) return ` ${secs}s`;
  if (secs < 3600) return ` ${Math.round(secs / 60)}m`;
  return ` ${Math.round(secs / 3600)}h`;
}

function totalDrops(drops) {
  return Object.values(drops).reduce((acc, n) => acc + n, 0);
}
