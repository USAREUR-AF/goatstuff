// SPDX-License-Identifier: Apache-2.0
//
// Welcome / empty-state overlay.
//
// Renders a centered card explaining what panoscope is, what the user
// will see when data flows, and the current connection-source status.
// The card stays visible until the first node lands in the graph, then
// fades to a minimized status pill in the top-right corner. Click the
// pill to re-open the card.
//
// Why this exists (per the design doc's empty-state rationale):
// the fabric will frequently be quiet — vendors haven't started
// publishing yet, the router is bumping versions, the schema-resolver
// is in fallback, etc. A blank D3 canvas is technically correct but
// gives no signal about which of those quiet-conditions applies. The
// overlay enumerates the layers so the operator can tell apart "no
// vendors publishing yet" from "router unreachable" from "schema
// resolver in fallback."

import type { ConnectionState } from "./ws.js";
import type { Legend } from "./types.js";

export type StateBadge = "ok" | "wait" | "warn" | "fail";

export interface WelcomeStatus {
  /** False until a view has actually been received. Before that the page
   *  knows nothing about the fabric, and must not say it is empty. */
  answered: boolean;
  ws: ConnectionState;
  resolver: Legend["schema_resolver_status"] | null;
  resolverFamilies: number;
  nodeCount: number;
  edgeCount: number;
  lastEventMs: number | null;
}

const SOURCE_DOCS_URL =
  "https://github.com/usareur-af/goatcore/blob/main/docs/design/panoscope.md";

export class WelcomePanel {
  private card: HTMLDivElement;
  private pill: HTMLButtonElement;
  private rows: Record<string, HTMLElement> = {};
  private dismissed = false;
  // Auto-dismiss is a ONE-SHOT — fires once on first data, with a small
  // readable delay, and never again. Prior behaviour re-fired on every
  // ws delta, so clicking the pill flashed the card open for <1 s
  // before the next update auto-dismissed it. This latch decouples
  // "card should currently be visible" (dismissed) from "the one-shot
  // auto-dismiss has already happened" (autoDismissed). Once
  // autoDismissed is true, only operator clicks control visibility.
  private autoDismissed = false;
  // How long the card stays up on first load before the one-shot
  // auto-dismiss fires. Picked to be long enough to read the four
  // connection-state rows without feeling like a wall.
  private readonly AUTO_DISMISS_DELAY_MS = 4000;

  constructor(parent: HTMLElement) {
    this.card = document.createElement("div");
    this.card.className = "welcome-card";
    this.card.innerHTML = this.cardHtml();
    parent.appendChild(this.card);

    this.pill = document.createElement("button");
    this.pill.type = "button";
    this.pill.className = "welcome-pill hidden";
    this.pill.textContent = "STATUS";
    this.pill.title =
      "Click to re-open the connection status panel " +
      "(WebSocket, Zenoh subscriber, schema resolver, peer counts)";
    this.pill.addEventListener("click", () => this.reopen());
    parent.appendChild(this.pill);

    this.card
      .querySelector(".welcome-dismiss")
      ?.addEventListener("click", () => this.dismiss(false));

    this.cacheRows();
  }

  private cardHtml(): string {
    return `
      <header class="welcome-header">
        <h1>panoscope</h1>
        <p class="welcome-subtitle">
          Live visualization of the Zenoh fabric.
        </p>
        <button class="welcome-dismiss" type="button" aria-label="Hide welcome panel">×</button>
      </header>
      <section class="welcome-section">
        <h2>What you're looking at</h2>
        <p>
          Every peer publishing or subscribing on the fabric appears as a
          <strong>node</strong>. Every topic flow appears as an
          <strong>edge</strong>. Edge color = registered schema family
          (gray = no schema declared). Edge thickness = recent publish
          rate. Small <strong>packet dots</strong> animate along edges
          on each sampled publish — you'll see the fabric breathing.
        </p>
      </section>
      <section class="welcome-section">
        <h2 id="welcome-canvas-h">The canvas, right now</h2>
        <p id="welcome-canvas-p">
          Waiting for the first answer from panoscope. Until it lands this
          page knows nothing about the fabric — the counts read
          <code>—</code> rather than zero, because zero would be a claim.
        </p>
      </section>
      <section class="welcome-section">
        <h2>Connection state</h2>
        <dl class="welcome-status">
          <dt>WebSocket to backend</dt>
          <dd class="welcome-status-row" id="row-ws">
            <span class="welcome-badge" data-state="wait"></span>
            <span class="welcome-status-label">checking…</span>
          </dd>
          <dt>Zenoh subscriber</dt>
          <dd class="welcome-status-row" id="row-zenoh">
            <span class="welcome-badge" data-state="wait"></span>
            <span class="welcome-status-label">No samples observed yet.</span>
          </dd>
          <dt>Schema resolver</dt>
          <dd class="welcome-status-row" id="row-resolver">
            <span class="welcome-badge" data-state="wait"></span>
            <span class="welcome-status-label">checking…</span>
          </dd>
          <dt>Peers / edges in view</dt>
          <dd class="welcome-status-row" id="row-counts">
            <span class="welcome-badge" data-state="wait"></span>
            <span class="welcome-status-label">0 / 0</span>
          </dd>
          <dt>Render</dt>
          <dd class="welcome-status-row" id="row-render">
            <span class="welcome-badge" data-state="ok"></span>
            <span class="welcome-status-label">No errors.</span>
          </dd>
        </dl>
      </section>
      <section class="welcome-section">
        <h2>Where this fits</h2>
        <p>
          panoscope is the <em>read</em> surface of the registry stack
          — the <em>write</em> surfaces (cert-sign for per-vendor X.509
          identity, the portal for topic + schema registration,
          schema-enforcer for audit-mode counting) live in sibling
          services. Schemas registered there appear here as colored
          edges; un-registered topics show up gray. That visibility is
          the point: the visualization makes the quality of the
          registry work feelable, not just the fact of data flow.
        </p>
        <p class="welcome-footer">
          Design: <a href="${SOURCE_DOCS_URL}" target="_blank" rel="noopener">docs/design/panoscope.md</a>.
        </p>
      </section>
    `;
  }

  private cacheRows(): void {
    for (const id of ["row-ws", "row-zenoh", "row-resolver", "row-counts", "row-render"]) {
      const el = this.card.querySelector(`#${id}`) as HTMLElement | null;
      if (el) this.rows[id] = el;
    }
  }

  /** Surface a WS-handler render error in the STATUS panel. Paired
   *  with the on-canvas banner in main.ts: the banner is loud + can
   *  be dismissed; this row is quiet + persistent, so an operator who
   *  reopens the STATUS panel after dismissing the banner can still
   *  see that the last snapshot failed to render. */
  noteRenderError(reason: string, err: unknown): void {
    const message = err instanceof Error ? err.message : String(err);
    this.setRow("row-render", "fail", `${reason}: ${message}`);
  }

  update(status: WelcomeStatus): void {
    // WS row
    this.setRow("row-ws", this.wsBadge(status.ws), this.wsLabel(status.ws));

    // Zenoh subscriber row — inferred from event activity + node count.
    // No way to distinguish "router unreachable" from "router reachable
    // but quiet" purely from the frontend; the backend's /api/stats
    // would let us. v1 conflates them — both surface as "No samples
    // observed yet" with a wait badge.
    if (!status.answered) {
      this.setRow("row-zenoh", "wait", "Not asked yet — the first view is still in flight.");
    } else if (status.lastEventMs !== null && Date.now() - status.lastEventMs < 30000) {
      this.setRow("row-zenoh", "ok", "Samples flowing. Watch the graph.");
    } else if (status.nodeCount > 0) {
      this.setRow(
        "row-zenoh",
        "ok",
        "Subscribed; last sample > 30 s ago — fabric is quiet right now.",
      );
    } else {
      this.setRow(
        "row-zenoh",
        "wait",
        "No samples observed yet. Vendors haven't published, or the router-side bump is still in flight.",
      );
    }

    // Resolver row — the schema resolver is the compiled-in family-map
    // floor; per-peer edge colors come from peer schema-declarations on
    // top. A single honest state, never a portal-reachability verdict.
    if (status.resolver === "family-map") {
      this.setRow(
        "row-resolver",
        "ok",
        `Built-in schema-family map (${status.resolverFamilies} families). Edge colors come from peer schema-declarations (attest/schema/**) plus this floor map — the normal, healthy state.`,
      );
    } else {
      this.setRow("row-resolver", "wait", "checking…");
    }

    // What the canvas section says depends on whether anything answered:
    // before that, an empty canvas is ignorance, not a fact about the fabric.
    const h = document.getElementById("welcome-canvas-h");
    const para = document.getElementById("welcome-canvas-p");
    if (h && para) {
      if (!status.answered) {
        h.textContent = "The canvas, right now";
        para.innerHTML =
          "Waiting for the first answer from panoscope. Until it lands this " +
          "page knows nothing about the fabric — the counts read <code>—</code> " +
          "rather than zero, because zero would be a claim.";
      } else if (status.nodeCount === 0) {
        h.textContent = "The empty canvas, right now";
        para.innerHTML =
          "panoscope answered and there is nothing to draw. That's a fact about " +
          "the fabric, not a bug — until a publisher sends a sample under its " +
          "own anchor there are no nodes. The four layers below tell you which " +
          "condition you're in.";
      } else {
        h.textContent = "The canvas, right now";
        para.textContent =
          "Drawing what panoscope served for the current filters. The four " +
          "layers below tell you which condition you're in.";
      }
    }

    // Counts row
    const countsLabel = status.answered ? `${status.nodeCount} / ${status.edgeCount}` : "—";
    if (status.nodeCount > 0) {
      this.setRow("row-counts", "ok", countsLabel);
    } else {
      this.setRow(
        "row-counts",
        "wait",
        status.answered ? `${countsLabel} — waiting for first peer` : "— waiting for the first answer",
      );
    }

    // First-data auto-dismiss — one-shot, latched on `autoDismissed`.
    // Schedules a delayed dismiss so the operator has time to read the
    // four-row status table on cold load, then never auto-fires again
    // (so a manual pill-click to reopen stays open until the operator
    // dismisses with the × button). Without the latch, every ws delta
    // re-dismissed within ~100 ms of any reopen.
    if (status.nodeCount > 0 && !this.autoDismissed) {
      this.autoDismissed = true;
      window.setTimeout(() => {
        if (!this.dismissed) this.dismiss(true);
      }, this.AUTO_DISMISS_DELAY_MS);
    }
  }

  private setRow(id: string, state: StateBadge, label: string): void {
    const row = this.rows[id];
    if (!row) return;
    const badge = row.querySelector(".welcome-badge") as HTMLElement | null;
    const lbl = row.querySelector(".welcome-status-label") as HTMLElement | null;
    if (badge) badge.setAttribute("data-state", state);
    if (lbl) lbl.textContent = label;
  }

  private wsBadge(s: ConnectionState): StateBadge {
    switch (s) {
      case "live":
        return "ok";
      case "lagged":
        return "warn";
      case "connecting":
        return "wait";
      case "closed":
        return "fail";
    }
  }

  private wsLabel(s: ConnectionState): string {
    switch (s) {
      case "live":
        return "Connected. Streaming flow deltas.";
      case "lagged":
        return "Lagged — too many events buffered; will resync via /api/snapshot.";
      case "connecting":
        return "Connecting…";
      case "closed":
        return "Disconnected. Reconnecting…";
    }
  }

  private dismiss(animate: boolean): void {
    this.dismissed = true;
    this.card.classList.toggle("welcome-card-fade", animate);
    this.card.classList.add("welcome-card-hidden");
    this.pill.classList.remove("hidden");
  }

  private reopen(): void {
    this.dismissed = false;
    this.card.classList.remove("welcome-card-hidden", "welcome-card-fade");
    this.pill.classList.add("hidden");
  }
}
