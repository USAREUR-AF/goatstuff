// SPDX-License-Identifier: Apache-2.0
//
// Frontend entry point.
//
// Bootstrap:
//   1. Build the GraphRenderer against the #graph svg element.
//   2. Start the LegendPanel (polls /api/legend on a 5s timer).
//   3. Build the WelcomePanel overlay (explains panoscope + per-source
//      connection status until first data arrives).
//   4. Open WebSocket to /ws — first message is the initial snapshot,
//      then coalesced deltas every ~100ms.
//   5. On publish_sampled events, ping the matching edge with a packet
//      dot. On peer_seen events, the next snapshot resync via /api/
//      snapshot will rebuild the graph (cheaper than diffing the WS
//      deltas in v1).
//
// Connection state surfaces in the header indicator + welcome panel.

import { GraphRenderer, isFutureDesignKeyExpr, selectorMatches, shortenUuids } from "./render.js";
import { ViewClient } from "./view-client.js";
import { shortId } from "./ids.js";
import { medianIntervalMs, streamIntervalMs } from "./freshness.js";
import { makeIcon, type IconName } from "./icons.js";
import { LegendPanel } from "./legend.js";
import { PerformanceMonitor, POLICY, type PerfTier } from "./perf.js";
import { WsClient, type ConnectionState } from "./ws.js";
import { WelcomePanel, type WelcomeStatus } from "./welcome.js";
import { compileFilter, type JsonValue } from "./jq-lite.js";
import { isStale, foldFetchEma } from "./freshness.js";
import { handleResponse, isPageSignedOut, shouldPoll } from "./session.js";
import { LEX, applyLexicon } from "./lex.js";
import type {
  DecodedSampleRow,
  GroupMemberRow,
  GroupOut,
  Legend,
  SiblingUrls,
  TopicDecodeOutcome,
  ViewLimits,
  ViewParams,
  ViewResponse,
  WsEvent,
} from "./types.js";

const STATUS_REFRESH_MS = 2_000;
/** Per-fetch hard timeout for the fleet-bar + legend polls. Without
 *  this, a hung backend response keeps the connection slot occupied
 *  forever; with browsers capped at ~6 connections/origin, a couple
 *  of stuck fetches saturate the pool and every subsequent /api/* call
 *  sits 'pending'. Capping the wait makes a slow backend show up as a
 *  visible failure (consecutiveFailures bumps, STALE trips honestly)
 *  rather than silently hanging the HUD. 8 s is well past the normal
 *  /api/view response time even on a busy fabric — anything
 *  beyond that is genuinely broken, not just slow. */
const FLEET_FETCH_TIMEOUT_MS = 8_000;

/** The operator's view parameters and the `/api/view` client — one per page.
 *  Every toolbar control, canvas gesture, the inspector and search go through
 *  it (docs/design/panoscope-view-model.md), so they all describe one view. */
const viewClient = new ViewClient();

/** View bus — one HTTP fetcher (the fleet-bar poll), many consumers. One
 *  publisher keeps the browser's per-origin connection pool free: the welcome
 *  panel, canvas and HUD used to fetch on their own and saturated it whenever
 *  a request lagged. */
type ViewSubscriber = (view: ViewResponse, params: ViewParams) => void;
const viewBus = new Set<ViewSubscriber>();
function publishView(view: ViewResponse, params: ViewParams): void {
  for (const cb of viewBus) {
    try {
      cb(view, params);
    } catch (e) {
      console.warn("view subscriber threw", e);
    }
  }
}
function subscribeView(cb: ViewSubscriber): () => void {
  viewBus.add(cb);
  return () => viewBus.delete(cb);
}

/** Fetch the view now rather than at the next poll — a parameter changed.
 *  Assigned by installFleetBar, which owns the one fetcher. */
let requestViewNow: () => void = () => {};

/** History-lens window selections, hours, widest→narrowest (matches the
 *  toolbar select). docs/design/panoscope-history-lens.md. */
const HISTORY_WINDOW_OPTIONS: { hours: number; label: string }[] = [
  { hours: 168, label: "7d" },
  { hours: 72, label: "3d" },
  { hours: 48, label: "48h" },
  { hours: 24, label: "24h" },
  { hours: 12, label: "12h" },
  { hours: 6, label: "6h" },
  { hours: 1, label: "1h" },
];

function main(): void {
  const svg = document.getElementById("graph") as SVGSVGElement | null;
  if (!svg) {
    throw new Error("missing #graph element");
  }
  const indicator = document.getElementById("connection-indicator");
  const mainEl = document.querySelector("main") as HTMLElement | null;
  if (!mainEl) {
    throw new Error("missing <main> element");
  }

  const renderer = new GraphRenderer(svg, {
    onNodeClick: (n) => renderNodeInfo(n),
    onSelectionChange: (selectedIds) => {
      // When selection cleared via canvas click, hide the side panel
      // and reset the inspection history — a fresh selection starts a
      // new drill-down trail rather than appending to the prior one.
      if (selectedIds.length === 0) {
        const panel = document.getElementById("info-panel");
        if (panel) panel.classList.add("hidden");
        inspectionHistory = [];
        renderInspectionBreadcrumb();
        stopStream();
      }
      // Re-evaluate the watchful badge — selection change can flip the
      // condition (selection appears → badge hides; selection clears
      // under sustained fleet-rate → badge re-appears).
      refreshWatchfulBadge();
    },
  });
  // Expose renderer to search overlay + keyboard handlers.
  (window as unknown as { __panoscopeRenderer: GraphRenderer }).__panoscopeRenderer = renderer;
  // Canvas gestures become view-parameter edits; every edit refetches now.
  renderer.setViewIntents({
    openGroup: (id) => viewClient.openGroup(id),
    togglePeerFold: (peer) => viewClient.togglePeerFold(peer),
    toggleOrg: (org) => viewClient.toggleOrg(org),
    collapseAllOrgs: () => viewClient.collapseAllOrgs(),
    expandAll: () => viewClient.expandAll(),
    reveal: (key, publisher) => viewClient.reveal(key, publisher),
  });
  viewClient.onChange(() => requestViewNow());
  // Adaptive performance monitor — drives the tier badge + the
  // fleet-bar polling cadence + the renderer's animation gates.
  installPerformanceMonitor(renderer);
  installFleetBar();
  installSearchOverlay(renderer);
  installPanelClose(renderer);
  installStreamPause();
  installUuidToggle();
  installStreamLive();
  installStreamModeToggle();
  installStreamToolkit();
  installSourceToggle();
  installInfraToggle();
  installTestToggle();
  installHistoryWindow();
  installDetailToggle();
  installVendorCollapseControls(renderer);
  installLegendOverlay();
  installKeysOverlay();

  const legend = new LegendPanel();
  legend.start();

  const welcome = new WelcomePanel(mainEl);

  // WelcomeStatus inputs — accumulated from the WS lifecycle + the
  // last snapshot + a periodic /api/legend poll (LegendPanel does its
  // own poll for the footer strip; we read the legend cache for the
  // welcome status as well via fetch on a slower interval).
  const status: WelcomeStatus = {
    answered: false,
    ws: "connecting",
    resolver: null,
    resolverFamilies: 0,
    nodeCount: 0,
    edgeCount: 0,
    lastEventMs: null,
  };

  const renderStatus = (): void => welcome.update({ ...status });
  renderStatus();

  // Diagnostics overlay — clickable left sidebar opens a detailed
  // explanation panel (metric defs + cadences + tier semantics + WS
  // state). ALSO drives the connection-indicator's "quiet" sub-state
  // via a periodic check of status.lastEventMs (a fabric-alive signal
  // separate from the raw WS state — see task #13).
  installFleetDiagnostics(status);

  // Block 94 cross-surface deep-link contract: pre-select from URL
  // query params on the first snapshot that contains the requested
  // target. Single-shot — applied once and cleared, so subsequent
  // operator selections aren't overridden when the next snapshot
  // arrives. URL → selection only at v1; selection → URL (history
  // push) is documented as v2.
  let inboundSelectionPending = parseInboundSelection(window.location.search);

  const handleView = (view: ViewResponse, params: ViewParams): void => {
    try {
      renderer.setView(view, params);
    } catch (e) {
      // setView has already restored the last good frame; say what failed
      // rather than leaving a stale canvas that looks current.
      showRenderErrorBanner("view render failed", e);
      welcome.noteRenderError("view render failed", e);
      return;
    }
    renderTruncation(view);
    renderFocusBanner(renderer);
    status.answered = true;
    status.nodeCount = view.peers.length + view.topics.length + view.groups.length + view.orgs.length;
    status.edgeCount = view.edges.length;
    renderStatus();
    if (inboundSelectionPending) {
      const ids = resolveInboundSelection(inboundSelectionPending, view);
      if (ids.length > 0) {
        renderer.setSelection(ids);
        // Render the side panel for the first selected node so the
        // inbound link produces the same surface as a manual click.
        const node = renderer.allNodeIds().find((nn) => nn.id === ids[0]);
        if (node) renderNodeInfo(node);
        // Clear so a later view doesn't re-fire over operator selections.
        inboundSelectionPending = null;
      }
      // When the target isn't in this view yet (the peer hasn't published,
      // or the registry replay is still filling in) the request stays
      // pending and is retried on the next view.
    }
  };

  const handleDelta = (events: WsEvent[]): void => {
    // Cheap path: animate publish_sampled events along existing edges.
    // peer_seen and publish_dropped events bump a counter but don't
    // mutate the renderer until the next resync. sample_arrived events
    // trigger LIVE-follow refresh (notify-only; payload still via HTTP).
    let liveRefreshPending = false;
    for (const ev of events) {
      if (ev.kind === "publish_sampled") {
        renderer.pingEdge(ev.peer_id, ev.topic);
        status.lastEventMs = Date.now();
      } else if (ev.kind === "peer_seen") {
        status.lastEventMs = Date.now();
      } else if (ev.kind === "sample_arrived") {
        // Backend has already filtered to watched topics — if any
        // sample_arrived shows up here, the operator wants to see
        // the refreshed stream pane. Coalesce to a single trailing
        // debounce so high-rate topics don't churn the DOM.
        if (streamLive && streamCurrentTopics.includes(ev.topic)) {
          liveRefreshPending = true;
        }
      }
    }
    if (liveRefreshPending) {
      if (liveRefreshDebounce !== null) {
        window.clearTimeout(liveRefreshDebounce);
      }
      liveRefreshDebounce = window.setTimeout(() => {
        liveRefreshDebounce = null;
        void refreshStream();
      }, LIVE_REFRESH_DEBOUNCE_MS) as unknown as number;
    }
    renderStatus();
    // The view is refreshed by the fleet-bar poll → viewBus, NOT by a
    // delta-triggered fetch from this handler: a parallel fetch per delta
    // saturated the browser's per-origin connection cap when any fetch
    // lagged. One publisher, many subscribers.
  };

  // The fleet-bar's single /api/view fetcher reaches the renderer and the
  // welcome panel through the view bus.
  subscribeView(handleView);

  const refreshLegendStatus = async (): Promise<void> => {
    if (!shouldPoll(document.hidden, isPageSignedOut())) return;
    try {
      const resp = await fetch("/api/legend", {
        cache: "no-store",
        signal: AbortSignal.timeout(FLEET_FETCH_TIMEOUT_MS),
      });
      if (handleResponse(resp)) return;
      if (!resp.ok) return;
      const l = (await resp.json()) as Legend;
      // Display lexicon (ADR — portal-lexicon §"Panoscope"): relex
      // JS-generated labels + sync the title/subtitle to this goatnet.
      applyLexicon(l);
      status.resolver = l.schema_resolver_status;
      status.resolverFamilies = l.schema_resolver_known_families;
      renderStatus();
      // Block 94 cross-surface deep-link contract: prefer the structured
      // `siblings` set; fall back to the legacy flat `portal_base_url`
      // for backwards compat. The renderer's mirror keeps the existing
      // schema-page link working either way.
      if (l.siblings) renderer.setSiblings(l.siblings);
      if (l.portal_base_url) renderer.setPortalBaseUrl(l.portal_base_url);
      decoderEnabled = !!l.decoder_enabled;
      applyDecoderEnabledToStreamToggle();
    } catch (e) {
      // Network hiccup; LegendPanel itself handles its own retry.
    }
  };
  void refreshLegendStatus();
  window.setInterval(() => void refreshLegendStatus(), STATUS_REFRESH_MS);

  const ws = new WsClient(buildWsUrl(), {
    // The socket is opened with `snapshot=0`; a server that predates it still
    // sends one, which this page ignores — it draws only from /api/view.
    onSnapshot: () => {},
    onDelta: handleDelta,
    onLagged: (missed) => {
      // The next fleet-bar view poll (~2 s at LIVE tier) catches
      // us up via the snapshotBus. No out-of-band fetch from here —
      // that pattern used to race the fleet-bar poll and contributed
      // to the per-origin connection-pool saturation that tripped the
      // STALE gate on healthy internet.
      console.warn(`ws: lagged, missed ${missed}`);
      status.ws = "lagged";
      renderStatus();
    },
    onStateChange: (s: ConnectionState) => {
      status.ws = s;
      renderStatus();
      if (indicator) {
        indicator.setAttribute("data-state", s);
        indicator.textContent =
          s === "live" ? "live" : s === "connecting" ? "connecting…" : s;
      }
      // WS closed → drop the local watch mirror. The backend's
      // per-connection state died with the socket; the next onReady
      // re-emits if streamLive is still on. Lifecycle invariant per
      // docs/design/panoscope.md §"LIVE-follow on the stream view".
      if (s === "closed") {
        streamWatchedTopics.clear();
      }
    },
    onReady: () => {
      // Reconnect re-emits the current watch so the backend's
      // (newly-fresh) per-connection HashSet matches what the
      // operator opted into pre-disconnect.
      if (streamLive && streamCurrentTopics.length > 0) {
        for (const t of streamCurrentTopics) {
          ws.send({ op: "watch", topic: t });
          streamWatchedTopics.add(t);
        }
      }
    },
    onError: (reason, err) => {
      showRenderErrorBanner(reason, err);
      welcome.noteRenderError(reason, err);
    },
  });
  __wsClient = ws;
  ws.connect();
}

function buildWsUrl(): string {
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  // `snapshot=0`: this page draws from /api/view and has no use for the
  // opening full snapshot the server would otherwise build and send.
  return `${proto}//${window.location.host}/ws?snapshot=0`;
}

/** Surface a sticky banner over the canvas when a WS handler throws.
 *  Without this, a TDZ / type-confusion inside renderer.setView
 *  blanks the viz while the HUD keeps ticking from the parallel
 *  WS-events path — operator reads it as "fabric is quiet" and the
 *  real failure stays hidden in devtools. */
function showRenderErrorBanner(reason: string, err: unknown): void {
  let banner = document.getElementById("render-error-banner");
  if (!banner) {
    banner = document.createElement("div");
    banner.id = "render-error-banner";
    banner.className = "render-error-banner";
    const msg = document.createElement("span");
    msg.className = "render-error-banner-msg";
    banner.appendChild(msg);
    const dismiss = document.createElement("button");
    dismiss.type = "button";
    dismiss.className = "render-error-banner-dismiss";
    dismiss.setAttribute("aria-label", "Dismiss render-error banner");
    dismiss.textContent = "×";
    dismiss.addEventListener("click", () => banner!.classList.add("hidden"));
    banner.appendChild(dismiss);
    document.body.appendChild(banner);
  }
  banner.classList.remove("hidden");
  const message = err instanceof Error ? err.message : String(err);
  const msgEl = banner.querySelector(".render-error-banner-msg");
  if (msgEl) msgEl.textContent = `${reason}: ${message}`;
}

let streamRefreshHandle: number | null = null;
let streamCurrentTopics: string[] = [];
let streamPaused = false;
type StreamMode = "raw" | "decoded";
let streamMode: StreamMode = "raw";

/** LIVE-follow state (operator opt-in per docs/design/panoscope.md
 *  §"LIVE-follow on the stream view"). When true, the backend is
 *  notified-watching streamWatchedTopics and inbound sample_arrived
 *  events trigger an early refreshStream() instead of waiting for the
 *  tier-adaptive poll. The watch lifecycle is tied to selection +
 *  pause + stream-section visibility; see installStreamLive(). */
let streamLive = false;
/** Topics we've sent `{op:"watch",topic}` for without a matching
 *  unwatch — the source of truth for the per-connection watch set
 *  the backend is currently honoring for us. Kept in sync with
 *  streamCurrentTopics on every selection change while streamLive,
 *  and emptied on PAUSE + stopStream + LIVE-off + WS close. */
let streamWatchedTopics: Set<string> = new Set();
/** Trailing-edge debounce timer for sample_arrived notifies. High-rate
 *  topics naturally batch into one refresh per LIVE_REFRESH_DEBOUNCE_MS
 *  window; low-rate topics still see at most ~50ms of latency. */
let liveRefreshDebounce: number | null = null;
const LIVE_REFRESH_DEBOUNCE_MS = 50;
/** Stream-pane staleness floor when the publish interval is unknown (a
 *  single sample). Generous so one recent storage-recall doesn't read as
 *  STALE; a genuinely old value (minutes) still trips it. */
const STREAM_STALE_UNKNOWN_FLOOR_MS = 30_000;

/** WsClient handle so installStreamLive can send {op:"watch"|"unwatch"}.
 *  Set in main() during WS construction; null until then. */
let __wsClient: WsClient | null = null;

// Inspection-flow history — stack of selections the operator drilled
// through. Lets the side panel render a clickable breadcrumb so the
// operator can step back peer → topic → decoded without losing the
// canvas selection or having to re-find the original peer by eye.
// ESC pops one level.
//
// Scope is also per-entry so the hears/says/all toggle on a peer
// inspection survives a topic drill-down + back-navigation (operator
// who set a deriver to "HEARS only" doesn't lose that choice when
// they pop back from a topic inspection).
type StreamScope = "all" | "hears" | "says";
interface InspectionEntry {
  kind: "peer" | "topic" | "namespace";
  nodeId: string;
  label: string;
  scope: StreamScope;  // peer-only; topic entries always 'all'
}
let inspectionHistory: InspectionEntry[] = [];
// True while a back-nav / breadcrumb-jump is mid-flight so renderNodeInfo
// doesn't double-push onto the history.
let suppressHistoryPush = false;
// Assigned by installSearchOverlay so the namespace inspector's
// "open in fleet search" escape-hatch can drive the existing `/` search
// overlay (open + pre-fill a query) without reaching into its closure.
// Null until the overlay is installed at bootstrap.
let openFleetSearch: ((prefill: string) => void) | null = null;
type JsonFormat = "pretty" | "compact";
let streamJsonFormat: JsonFormat = "pretty";
// Compiled jq-lite filter (or null when the input is empty / errored).
// Recompiled whenever the input box changes. The next refreshStream pass
// applies it to each decoded sample's JSON output.
let streamFilter: import("./jq-lite.js").CompiledFilter | null = null;
// Cached from /api/legend on bootstrap. When false, the Decoded toggle
// stays disabled and shows a one-line tooltip pointing at the deploy gap.
let decoderEnabled = false;

/** Re-render the breadcrumb row above #info-title. Empty when only
 *  one entry is in history (no path to step back through). Each
 *  segment except the last is clickable + pops to that level. */
function renderInspectionBreadcrumb(): void {
  const crumb = document.getElementById("info-breadcrumb");
  if (!crumb) return;
  while (crumb.firstChild) crumb.removeChild(crumb.firstChild);
  if (inspectionHistory.length < 2) {
    crumb.classList.add("hidden");
    return;
  }
  crumb.classList.remove("hidden");

  const back = document.createElement("button");
  back.type = "button";
  back.className = "info-breadcrumb-back";
  back.title = "back one level (ESC)";
  back.textContent = "‹";
  back.addEventListener("click", () => popInspection());
  crumb.appendChild(back);

  inspectionHistory.forEach((entry, idx) => {
    if (idx > 0) {
      const sep = document.createElement("span");
      sep.className = "info-breadcrumb-sep";
      sep.textContent = "›";
      crumb.appendChild(sep);
    }
    const seg = document.createElement(idx === inspectionHistory.length - 1 ? "span" : "button");
    seg.className = "info-breadcrumb-seg";
    if (idx === inspectionHistory.length - 1) {
      seg.classList.add("info-breadcrumb-seg--current");
    } else {
      (seg as HTMLButtonElement).type = "button";
      seg.addEventListener("click", () => jumpInspection(idx));
    }
    seg.textContent = entry.label;
    crumb.appendChild(seg);
  });
}

/** Step back one level. Used by the breadcrumb back-arrow + ESC. */
function popInspection(): void {
  if (inspectionHistory.length < 2) return;
  inspectionHistory.pop();
  reEnterCurrent();
}

/** Jump to a specific breadcrumb segment by index. */
function jumpInspection(idx: number): void {
  if (idx < 0 || idx >= inspectionHistory.length) return;
  inspectionHistory = inspectionHistory.slice(0, idx + 1);
  reEnterCurrent();
}

/** Re-render the side panel for the topmost history entry without
 *  pushing onto history (used by back / jump navigation). */
function reEnterCurrent(): void {
  const top = inspectionHistory[inspectionHistory.length - 1];
  if (!top) return;
  const rndr = (window as unknown as { __panoscopeRenderer: GraphRenderer })
    .__panoscopeRenderer;
  if (!rndr) return;
  const node = rndr.allNodeIds().find((nn) => nn.id === top.nodeId);
  if (!node) {
    // Node disappeared (e.g. peer aged out while operator was deep-
    // diving). Drop the entry and try again.
    inspectionHistory.pop();
    reEnterCurrent();
    return;
  }
  rndr.setSelection([top.nodeId]);
  suppressHistoryPush = true;
  try {
    renderNodeInfo(node);
  } finally {
    suppressHistoryPush = false;
  }
}

/** Render the publisher pub/sub-hygiene block: the snapshot verdict badge,
 *  then a lazy fetch of the full report (findings + fixes + the live storage
 *  retrievability probe) from `/api/publisher/hygiene`. Honesty discipline:
 *  the retrievability line distinguishes "confirmed" from "unconfirmed"
 *  (never claims "not retrievable"). See docs/design/pubsub-hygiene-diagnostic.md. */
function appendHygiene(
  body: HTMLElement,
  peerId: string,
  h: import("./types.js").NodeHygiene,
): void {
  const wrap = document.createElement("div");
  wrap.className = "hygiene-block";
  const badge = document.createElement("div");
  badge.className = `hygiene-badge hygiene-${h.verdict}`;
  badge.textContent = `pub/sub hygiene: ${h.verdict.toUpperCase()}`;
  wrap.appendChild(badge);
  const detail = document.createElement("div");
  detail.className = "hygiene-detail";
  detail.textContent = "checking retrievability…";
  wrap.appendChild(detail);
  body.appendChild(wrap);

  fetch(`/api/publisher/hygiene?source=${encodeURIComponent(peerId)}`)
    .then((r) => (r.ok ? r.json() : null))
    .then((rep: import("./types.js").PublisherHygiene | null) => {
      detail.textContent = "";
      if (!rep) {
        detail.textContent = "hygiene check unavailable";
        return;
      }
      const ret = document.createElement("div");
      ret.className = `hygiene-retrieval hygiene-ret-${rep.retrievability}`;
      const retText: Record<string, string> = {
        confirmed: "retrievable later: CONFIRMED — a storage answered a get",
        unconfirmed:
          "retrievable later: UNCONFIRMED — no storage answered the probe (may be no storage, an empty one, or none configured)",
        notprobed: "retrievable later: not probed",
      };
      ret.textContent = retText[rep.retrievability] ?? rep.retrievability;
      detail.appendChild(ret);
      for (const f of rep.findings) {
        if (f.severity === "info") continue;
        const fd = document.createElement("div");
        fd.className = `hygiene-finding hygiene-sev-${f.severity}`;
        const msg = document.createElement("div");
        msg.className = "hygiene-msg";
        msg.textContent = f.message;
        fd.appendChild(msg);
        if (f.fix) {
          const fix = document.createElement("div");
          fix.className = "hygiene-fix";
          fix.textContent = `fix: ${f.fix}`;
          fd.appendChild(fix);
        }
        detail.appendChild(fd);
      }
    })
    .catch(() => {
      detail.textContent = "hygiene check failed";
    });
}

function renderNodeInfo(n: {
  id: string;
  slot_alias?: string | null;
  display_name?: string | null;
  vendor: string | null;
  role: string;
  publishes: string[];
  subscribes: string[];
  first_seen_ms: number;
  last_seen_ms: number;
  // Group fields: present only when the clicked node is a group, which
  // routes to the group inspector below.
  is_singleton_super?: boolean;
  singleton_source_peer?: string;
  singleton_namespace?: string;
  singleton_count?: number;
}): void {
  const panel = document.getElementById("info-panel");
  const title = document.getElementById("info-title");
  const body = document.getElementById("info-body");
  if (!panel || !title || !body) return;

  panel.classList.remove("hidden");

  // Group inspector — fourth side-panel mode. A group node stands for many
  // topics; render a list browser instead of the peer/topic/router modes
  // below. Handles its own history push (kind "namespace").
  if (n.is_singleton_super) {
    renderGroupInspector(n, panel, title, body);
    return;
  }

  // Topic-virtual node: title is the topic, the only "publish" target
  // is the topic itself. Strip the synthetic `topic:` prefix when
  // displaying.
  const isTopicNode = n.role === "topic" && n.id.startsWith("topic:");
  const topicKeyExpr = isTopicNode ? n.id.slice("topic:".length) : null;

  // History push (skipped during back / jump navigation). Topic and
  // peer entries are distinguished so the breadcrumb can label the
  // segment correctly.
  if (!suppressHistoryPush) {
    const label = isTopicNode
      ? topicKeyExpr ?? n.id
      : (n.slot_alias
          ?? (n as { display_name?: string | null }).display_name
          ?? n.vendor
          ?? shortId(n.id));
    const top = inspectionHistory[inspectionHistory.length - 1];
    // Repeated select on the same node id: refresh in place, don't
    // grow the stack.
    if (top && top.nodeId === n.id) {
      top.label = label;
    } else {
      inspectionHistory.push({
        kind: isTopicNode ? "topic" : "peer",
        nodeId: n.id,
        label,
        scope: "all",
      });
    }
  }
  renderInspectionBreadcrumb();
  // Infrastructure (Zenoh router) — admin-namespace discovered, no
  // schema-decl, no display_name/vendor/role_class. Without an
  // explicit branch the title would render as a truncated ZID.
  const meta = n as typeof n & {
    site?: string | null;
    role_class?: string | null;
    format_hint?: string | null;
    node_kind?: string | null;
    node_class?: string | null;
    node_subkind?: string | null;
    locator?: string | null;
    universal_observer?: boolean | null;
    hygiene?: import("./types.js").NodeHygiene | null;
  };
  const isRouterNode = meta.node_kind === "router";
  // Universal `**` observer — subscribes to everything; the backend
  // suppresses its per-topic fan-out edges, so it renders edgeless. Badge +
  // explain it so an edgeless node reads as intentional, not orphaned.
  const isUniversalObserver = meta.universal_observer === true && !isTopicNode;
  // Underlying infrastructure (the generalization of isRouterNode) — any
  // node the backend classified as substrate: router, observer subscriber,
  // schema-registry / operator-system publisher, chaos generator, or
  // self-declared. Drives the INFRASTRUCTURE badge + the substrate side-
  // panel treatment (no sibling actions, a "what this is, not a data
  // participant" explainer) uniformly across the whole class.
  const isInfraNode = meta.node_class === "infra";

  title.innerHTML = "";
  // Label preference for peer nodes: display_name > vendor > short(peer_id).
  // After the notional namespace collapse, every cert-bound
  // vendor is uniform ("notional"), so vendor alone produces 22 nodes
  // all labeled "notional" — useless. The schema-decl `display_name`
  // field (e.g. "tarsier-alpha") is what gives the side panel + node
  // labels their per-peer variety back.
  const titleText = isTopicNode
    ? topicKeyExpr ?? n.id
    : isRouterNode
      ? "zenoh-router"
      : (n.slot_alias ?? n.display_name ?? n.vendor ?? shortId(n.id));
  // Shorten the UUID in a topic-node title; keep the full key on hover.
  // (Honors the console expand toggle, but the title isn't a `.uuid-key`
  // element — it's re-rendered per selection, so it picks up the flag then.)
  if (isTopicNode) {
    (title as HTMLElement).title = titleText;
    title.appendChild(document.createTextNode(consoleKeyDisplay(titleText)));
  } else {
    title.appendChild(document.createTextNode(titleText));
  }
  // FUTURE-DESIGN badge — attest/** topics carry schema-decl heartbeats
  // from the synth-vendor (every 30 s) but the schema-enforcer
  // interceptor + data-dictionary renderer that would consume them are
  // not deployed; the portal serves that role as a
  // facilitation expedient. Tag the selection visibly so a new operator
  // doesn't read the cluster as live application traffic.
  if (isTopicNode && topicKeyExpr && isFutureDesignKeyExpr(topicKeyExpr)) {
    const badge = document.createElement("span");
    badge.className = "future-design-badge";
    badge.textContent = "FUTURE DESIGN";
    title.appendChild(badge);
  }
  // SYNTHETIC badge — visible cue that this peer's traffic is notional.
  if ((n as typeof n & { synthetic?: boolean | null }).synthetic === true) {
    const badge = document.createElement("span");
    badge.className = "synthetic-badge";
    badge.textContent = "SYNTHETIC";
    title.appendChild(badge);
  }
  // INFRASTRUCTURE badge — visible cue that this node is underlying
  // substrate, not an application data participant. Generalized from the
  // router-only check to the whole infra class (router / observer
  // subscriber / schema-registry / operator-system / chaos / self-
  // declared); the subkind is appended so the operator sees both "this is
  // substrate" and "what kind." Paired with the desaturated fill on the
  // canvas to keep infra nodes out of the peer-attention path.
  if (isInfraNode) {
    const badge = document.createElement("span");
    badge.className = "infra-badge";
    const sub = infraSubkindLabel(meta.node_subkind);
    badge.textContent = sub ? `INFRASTRUCTURE · ${sub}` : "INFRASTRUCTURE";
    title.appendChild(badge);
  }
  if (isUniversalObserver) {
    const badge = document.createElement("span");
    badge.className = "universal-observer-badge";
    badge.textContent = "WATCHES ∗∗"; // "WATCHES **"
    badge.title = "Subscribes to ** (every topic). Per-topic edges suppressed.";
    title.appendChild(badge);
  }

  body.innerHTML = "";
  // Block 94 sibling-surface buttons — diagnose / tail / schema-page
  // for the currently-selected target. Per-topic for topic-broker
  // pins; per-vendor + per-peer for application peers. Infrastructure
  // nodes get nothing (no actionable mission-data target on the
  // substrate) — generalized from the router-only suppression.
  if (!isInfraNode) {
    const rndr = (window as unknown as { __panoscopeRenderer: GraphRenderer }).__panoscopeRenderer;
    const siblings = rndr?.getSiblings?.() ?? {};
    appendSiblingActionRow(body, siblings, {
      key: topicKeyExpr ?? undefined,
      vendor: n.vendor ?? undefined,
      peer: isTopicNode ? undefined : n.id,
    });
  }
  // Fold/unfold control — the discoverable, labeled equivalent of the
  // canvas double-click gesture (Option A + side-panel button). Guards
  // internally: topic pins + the infra router render nothing.
  appendFoldActionRow(body, n as Parameters<GraphRenderer["toggleFold"]>[0]);
  if (isUniversalObserver) {
    appendRow(
      body,
      "consumes",
      "** (every topic) — a universal observer. Its per-topic edges are deliberately " +
        "suppressed on the canvas: a node that watches everything would draw one edge to " +
        "every topic (N×M noise) and bury the real data flows. It still consumes all topics.",
    );
  }
  if (isTopicNode) {
    appendRow(body, "key_expr", topicKeyExpr ?? "");
    appendRow(body, "kind", "topic (broker pin)");
    if (topicKeyExpr && isFutureDesignKeyExpr(topicKeyExpr)) {
      appendRow(
        body,
        "status",
        "Future-design namespace. Here it carries 30s schema-decl heartbeats " +
          "from the synthetic publisher (panoscope's restart-resilience source) but the " +
          "schema-enforcer interceptor + data-dictionary consumers are not active on " +
          "this fabric; the goat-zenoh-portal serves the schema-registry role as a " +
          "facilitation expedient. Reserved on-wire namespace for the long-term shape.",
      );
    }
  } else if (isInfraNode) {
    // Infrastructure side panel: describe what this node IS and why it is
    // NOT an application data participant, for an operator who's never
    // seen the COP before. The kind + explainer are keyed off the backend
    // subkind so every infra type (router, observer subscriber, schema-
    // registry, operator-system, chaos generator, self-declared) gets a
    // specific "what this is" line rather than the generic router copy.
    const info = infraPanelInfo(meta.node_subkind);
    appendRow(body, "kind", info.kind);
    appendRow(body, "description", info.description);
    appendRow(body, "peer_id", n.id);
    if (n.vendor) appendRow(body, LEX.org, n.vendor);
    if (meta.locator) appendRow(body, "locator", meta.locator);
    if (n.publishes.length) appendRow(body, "publishes", String(n.publishes.length));
    if (n.subscribes.length) appendRow(body, "subscribes", String(n.subscribes.length));
    appendRow(body, "first seen", new Date(n.first_seen_ms).toLocaleTimeString());
    appendRow(body, "last seen", new Date(n.last_seen_ms).toLocaleTimeString());
  } else {
    // Slot alias (ADR 1039) first — the human identity — with the canonical
    // UUID kept on its own `peer_id` row directly beneath (the render_both
    // shape: alias label + full UUID for copy/deep-link). Absent for random
    // ZIDs / unknown anchors, where peer_id alone carries the identity.
    if (n.slot_alias) appendRow(body, "slot alias", n.slot_alias);
    if (n.display_name) appendRow(body, "name", n.display_name);
    appendRow(body, "peer_id", n.id);
    if (n.vendor) appendRow(body, LEX.org, n.vendor);
    if (meta.site) appendRow(body, "site", meta.site);
    if (meta.role_class) appendRow(body, "class", meta.role_class);
    if (meta.format_hint) appendRow(body, "format", meta.format_hint.toUpperCase());
    appendRow(body, "role", n.role);
    appendRow(body, "first seen", new Date(n.first_seen_ms).toLocaleTimeString());
    appendRow(body, "last seen", new Date(n.last_seen_ms).toLocaleTimeString());
    // Pub/sub hygiene verdict + lazy retrievability probe (publishers only).
    if (meta.hygiene) appendHygiene(body, n.id, meta.hygiene);
    // Drill-down lists: click a publish → select that topic-virtual
    // node (loads its stream). Click a subscribe → if it matches a
    // concrete topic show that, else show the pattern as text.
    const rndr = (window as unknown as { __panoscopeRenderer: GraphRenderer }).__panoscopeRenderer;
    if (n.publishes.length > 0) {
      appendTopicList(body, "publishes", n.publishes, rndr, (topic) => {
        const topicNodeId = `topic:${topic}`;
        rndr.setSelection([topicNodeId]);
        // Re-render the panel for the topic node.
        const tn = rndr.allNodeIds().find((nn) => nn.id === topicNodeId);
        if (tn) renderNodeInfo(tn);
      });
    }
    if (n.subscribes.length > 0) {
      appendList(body, "subscribes (patterns)", n.subscribes);
    }
    // For any peer that consumes (sink or deriver), surface the
    // concrete topics matching its subscribe patterns. Each is
    // clickable to drill into.
    const consumed = rndr?.consumedTopicsFor?.(n.id) ?? [];
    if (consumed.length > 0) {
      // Cap display at first 20 to avoid runaway with `**` subscribers.
      const truncated = consumed.length > 20 ? consumed.slice(0, 20) : consumed;
      appendTopicList(
        body,
        consumed.length > 20 ? `consumes (${consumed.length}, first 20)` : "consumes",
        truncated,
        rndr,
        (topic) => {
          const topicNodeId = `topic:${topic}`;
          rndr.setSelection([topicNodeId]);
          const tn = rndr.allNodeIds().find((nn) => nn.id === topicNodeId);
          if (tn) renderNodeInfo(tn);
        },
      );
    }
  }

  // Wire the stream panel. Topics flowing into the stream depend on
  // the operator's scope choice (HEARS / SAYS / ALL), which is per-
  // history-entry so it survives drill-downs + back-navigation.
  //   topic node          → that one topic (scope ignored)
  //   peer with pub + sub → scope picks publishes-only, subscribes-
  //                         only, or both
  //   pure source         → only publishes[]; toggle hidden
  //   pure sink           → only consumed topics; toggle hidden
  const renderer = (window as unknown as { __panoscopeRenderer: GraphRenderer })
    .__panoscopeRenderer;
  const consumed = renderer?.consumedTopicsFor?.(n.id) ?? [];
  const hasPub = n.publishes.length > 0;
  const hasSub = consumed.length > 0;
  const topEntry = inspectionHistory[inspectionHistory.length - 1];
  const scope: StreamScope = topEntry?.scope ?? "all";
  const streamTopics: string[] = isTopicNode
    ? topicKeyExpr ? [topicKeyExpr] : []
    : scopeTopics(n.publishes, consumed, scope);

  // Render the HEARS / SAYS / ALL toggle when the peer has both
  // directions; hide it otherwise (a pure source / pure sink has
  // only one meaningful choice).
  renderStreamScopeToggle({
    show: !isTopicNode && hasPub && hasSub,
    scope,
    onChange: (next) => {
      const top = inspectionHistory[inspectionHistory.length - 1];
      if (top) top.scope = next;
      const refreshed = scopeTopics(n.publishes, consumed, next);
      startStream(refreshed);
    },
  });

  startStream(streamTopics);
}

// ─── Group inspector (4th side-panel mode) ──────────────────────────
// A group is a set of one publisher's topics the server drew as one element
// (a family of same-shape siblings, a collapsed subtree, or the whole
// publisher). Graph viz is for relationships, a list is for inventory: this
// mode pages the members from /api/view/group — filtered server-side, in the
// viewer's scope, from exactly the grouping the canvas drew — with the
// group's aggregate stats. See docs/design/panoscope.md §"Side panel".

interface NsTopicRow {
  topic: string;
  format: string | null;
  total_events: number;
  last_event_ms: number;
  projected_eps: number;
  schema_family: string | null;
}

/** Above this many members "show on canvas" carries a "may slow browser"
 *  notice. The ceiling on what can be opened at all is the server's
 *  `limits.max_open_units`. */
const CANVAS_OPEN_WARN = 150;
const NS_SEARCH_DEBOUNCE_MS = 150;
/** Members fetched per page (the server caps a page at 1,000). */
const NS_PAGE = 500;
/** Row height in px — MUST match `.ns-inspector-row` height in styles.css
 *  (the windowed renderer does its visible-window math against it). */
const NS_ROW_HEIGHT = 30;
const NS_LIST_OVERSCAN = 6;

/** Whether opening a group on the canvas will draw it. A family opens into
 *  its members one level down, so its size is what the canvas receives; a
 *  subtree or whole-publisher group opens into regrouped children. The server
 *  keeps a group closed when the opening would exceed the limit either way. */
function groupOpensOnCanvas(g: GroupOut, limits: ViewLimits): boolean {
  return g.kind !== "family" || g.topic_count <= limits.max_open_units;
}

/** The part of a group label to pre-fill fleet search with: the literal
 *  prefix before its first wildcard. */
function groupSearchPrefix(label: string): string {
  const star = label.indexOf("*");
  return star < 0 ? label : label.slice(0, star);
}

function renderGroupInspector(
  n: {
    id: string;
    vendor: string | null;
    display_name?: string | null;
    singleton_source_peer?: string;
    singleton_namespace?: string;
    singleton_count?: number;
  },
  _panel: HTMLElement,
  title: HTMLElement,
  body: HTMLElement,
  initialFilter = "",
): void {
  const rndr = (window as unknown as { __panoscopeRenderer?: GraphRenderer })
    .__panoscopeRenderer;
  const group = rndr?.groupById(n.id);
  const view = rndr?.currentView();
  const peer = group?.publisher ?? n.singleton_source_peer ?? "";
  const label = group?.label ?? n.singleton_namespace ?? "";
  const count = group?.topic_count ?? n.singleton_count ?? 0;
  const peerNode = rndr?.allNodeIds().find((nn) => nn.id === peer);
  const peerName = peerNode?.slot_alias ?? peerNode?.display_name ?? shortId(peer);
  const wholePublisher = group?.kind === "publisher";
  const heading = wholePublisher ? `${peerName} · all topics` : label;

  // History push (skipped during back / jump navigation). One entry per
  // group id; a repeat select on the same id refreshes in place.
  if (!suppressHistoryPush) {
    const top = inspectionHistory[inspectionHistory.length - 1];
    if (top && top.nodeId === n.id) {
      top.label = heading;
    } else {
      inspectionHistory.push({ kind: "namespace", nodeId: n.id, label: heading, scope: "all" });
    }
  }
  renderInspectionBreadcrumb();

  title.innerHTML = "";
  title.appendChild(document.createTextNode(`${heading} `));
  const cnt = document.createElement("span");
  cnt.className = "ns-inspector-count";
  cnt.textContent = `(${count} topics)`;
  title.appendChild(cnt);

  body.innerHTML = "";

  // Sibling-action row scoped to the publisher's vendor (diagnose / tail).
  // No `key`: a pattern has no concrete schema, so the schema-viewer button
  // would dead-end — per-topic schema links live on the drilled-in panel.
  const siblings = rndr?.getSiblings?.() ?? {};
  appendSiblingActionRow(body, siblings, { vendor: n.vendor ?? undefined });

  // No live stream at the group level — the stream pane is per-topic and
  // opens on drill-in. Clear any stream left from a prior selection.
  startStream([]);

  const stats = document.createElement("div");
  stats.className = "ns-inspector-stats";
  body.appendChild(stats);

  const searchWrap = document.createElement("div");
  searchWrap.className = "ns-inspector-search";
  const search = document.createElement("input");
  search.type = "search";
  search.id = "ns-inspector-search-input";
  search.className = "ns-inspector-search-input";
  search.placeholder = `filter ${count} topics…`;
  search.setAttribute("aria-label", "filter topics in this group");
  search.value = initialFilter;
  searchWrap.appendChild(search);
  body.appendChild(searchWrap);

  const listHost = document.createElement("div");
  listHost.className = "ns-inspector-list";
  body.appendChild(listHost);

  // Canvas actions. A folded peer's single group unfolds the peer; any other
  // group opens on the canvas when the server will draw it.
  const actions = document.createElement("div");
  actions.className = "ns-inspector-actions";
  const folded = peer !== "" && viewClient.get().fold_peers.includes(peer);
  if (folded) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "ns-inspector-btn ns-inspector-show-canvas";
    btn.textContent = "unfold peer";
    btn.title = "show this peer's topics grouped normally again";
    btn.addEventListener("click", () => viewClient.togglePeerFold(peer));
    actions.appendChild(btn);
  } else if (group && view && groupOpensOnCanvas(group, view.limits)) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "ns-inspector-btn ns-inspector-show-canvas";
    const heavy = group.kind === "family" && count > CANVAS_OPEN_WARN;
    btn.textContent = heavy ? `show ${count} on canvas · ⚠ may slow browser` : "show on canvas";
    btn.title = heavy
      ? `draw all ${count} topics on the graph — this is a lot of nodes and may slow the browser`
      : "open this group on the graph, one level down";
    btn.addEventListener("click", () => viewClient.openGroup(n.id));
    actions.appendChild(btn);
  } else if (group) {
    const note = document.createElement("div");
    note.className = "ns-inspector-explode-note";
    note.textContent = `${count} topics — too many to draw on the graph; browse them in the list above`;
    actions.appendChild(note);
  }
  const fleetBtn = document.createElement("button");
  fleetBtn.type = "button";
  fleetBtn.className = "ns-inspector-btn ns-inspector-fleet-search";
  fleetBtn.textContent = "open in fleet search";
  fleetBtn.title = "search the whole fabric for this prefix";
  fleetBtn.addEventListener("click", () => openFleetSearch?.(groupSearchPrefix(label)));
  actions.appendChild(fleetBtn);
  body.appendChild(actions);

  // Drill into one topic → the topic-virtual mode. The pin usually isn't on
  // the canvas (it is inside this group), so synthesize the minimal node
  // renderNodeInfo's topic branch needs; setSelection is a no-op highlight
  // when the pin isn't painted.
  const drillIntoTopic = (topic: string) => {
    const topicNodeId = `topic:${topic}`;
    rndr?.setSelection([topicNodeId]);
    const onCanvas = rndr?.allNodeIds().find((nn) => nn.id === topicNodeId);
    renderNodeInfo(
      onCanvas ?? {
        id: topicNodeId,
        display_name: null,
        vendor: n.vendor,
        role: "topic",
        publishes: [],
        subscribes: [],
        first_seen_ms: 0,
        last_seen_ms: 0,
      },
    );
  };

  // Members, paged. `request` increments on every new filter so a slow page
  // for an old filter never paints over a newer one.
  let rows: NsTopicRow[] = [];
  let matched = 0;
  let total = count;
  let request = 0;
  let loading = false;
  const stillHere = () => {
    const top = inspectionHistory[inspectionHistory.length - 1];
    return !!top && top.nodeId === n.id;
  };
  const paint = () => {
    renderGroupStats(stats, group, total, matched, search.value.trim());
    renderVirtualizedTopicList(listHost, rows, matched, drillIntoTopic, () => void loadMore());
  };
  const load = async (q: string) => {
    const mine = ++request;
    loading = true;
    const page = await viewClient.groupMembers(n.id, q, 0, NS_PAGE).catch(() => null);
    loading = false;
    if (mine !== request || !stillHere()) return;
    if (!page) {
      // The group is no longer in this view (the model moved on, or a
      // parameter changed); say so rather than showing an empty list.
      rows = [];
      matched = 0;
      paint();
      listHost.innerHTML = "";
      const gone = document.createElement("div");
      gone.className = "ns-inspector-empty";
      gone.textContent = "this group is no longer drawn — select the publisher again";
      listHost.appendChild(gone);
      return;
    }
    total = page.total;
    matched = page.matched;
    rows = page.rows.map(memberRow);
    paint();
  };
  const loadMore = async () => {
    if (loading || rows.length >= matched) return;
    const mine = request;
    loading = true;
    const page = await viewClient.groupMembers(n.id, search.value.trim(), rows.length, NS_PAGE).catch(() => null);
    loading = false;
    if (mine !== request || !stillHere() || !page) return;
    rows = rows.concat(page.rows.map(memberRow));
    matched = page.matched;
    paint();
  };
  paint();
  void load(initialFilter.trim());

  let debounce: number | null = null;
  search.addEventListener("input", () => {
    if (debounce !== null) window.clearTimeout(debounce);
    debounce = window.setTimeout(() => void load(search.value.trim()), NS_SEARCH_DEBOUNCE_MS) as unknown as number;
  });
}

function memberRow(r: GroupMemberRow): NsTopicRow {
  return {
    topic: r.key,
    format: r.format,
    total_events: r.total_events,
    last_event_ms: r.last_event_ms,
    projected_eps: r.projected_eps,
    schema_family: r.schema_family,
  };
}

/** Aggregate stats above the member list, from the group as drawn: count
 *  (matched/total while filtering), format mix, rate, last event, subscribers. */
function renderGroupStats(
  host: HTMLElement,
  group: GroupOut | undefined,
  total: number,
  matched: number,
  filter: string,
): void {
  host.innerHTML = "";
  const line = (label: string, value: string) => {
    const row = document.createElement("div");
    row.className = "ns-inspector-stat";
    const k = document.createElement("span");
    k.className = "ns-inspector-stat-k";
    k.textContent = label;
    const v = document.createElement("span");
    v.className = "ns-inspector-stat-v";
    v.textContent = value;
    row.appendChild(k);
    row.appendChild(v);
    host.appendChild(row);
  };
  line("topics", filter ? `${matched} / ${total}` : String(total));
  if (!group) return;
  const formats = Object.entries(group.formats);
  if (formats.length > 0) {
    line(
      "formats",
      formats
        .sort((a, b) => b[1] - a[1])
        .map(([f, c]) => `${formatCompactNumber(c)} ${f}`)
        .join(" · "),
    );
  }
  if (group.projected_eps > 0) line("aggregate", `${group.projected_eps.toFixed(1)} msg/s`);
  if (group.last_event_ms > 0) line("last event", new Date(group.last_event_ms).toLocaleTimeString());
  if (group.subscriber_count > 0) line("subscribers", String(group.subscriber_count));
}

/** Windowed topic-list renderer. Only ~one viewport of rows is in the DOM at a
 *  time; an empty spacer reserves the scroll height of every MATCHED member,
 *  and scrolling near the end of the loaded rows asks for the next page. */
function renderVirtualizedTopicList(
  host: HTMLElement,
  rows: NsTopicRow[],
  matched: number,
  onRowClick: (topic: string) => void,
  onNeedMore: () => void,
): void {
  const keepScroll = host.scrollTop;
  host.innerHTML = "";
  if (rows.length === 0) {
    host.scrollTop = 0;
    host.onscroll = null;
    const empty = document.createElement("div");
    empty.className = "ns-inspector-empty";
    empty.textContent = matched === 0 ? "no matching topics" : "loading…";
    host.appendChild(empty);
    return;
  }

  const spacer = document.createElement("div");
  spacer.className = "ns-inspector-list-spacer";
  spacer.style.height = `${Math.max(rows.length, matched) * NS_ROW_HEIGHT}px`;
  const viewport = document.createElement("div");
  viewport.className = "ns-inspector-list-viewport";
  host.appendChild(spacer);
  host.appendChild(viewport);
  host.scrollTop = keepScroll;

  let lastFirst = -1;
  const renderWindow = () => {
    const viewH = host.clientHeight || 320;
    const first = Math.max(0, Math.floor(host.scrollTop / NS_ROW_HEIGHT) - NS_LIST_OVERSCAN);
    const visible = Math.ceil(viewH / NS_ROW_HEIGHT) + NS_LIST_OVERSCAN * 2;
    if (first + visible >= rows.length && rows.length < matched) onNeedMore();
    if (first === lastFirst) return;
    lastFirst = first;
    const last = Math.min(rows.length, first + visible);
    viewport.style.transform = `translateY(${first * NS_ROW_HEIGHT}px)`;
    viewport.innerHTML = "";
    for (let i = first; i < last; i++) {
      viewport.appendChild(buildNsRow(rows[i], onRowClick));
    }
  };
  host.onscroll = renderWindow;
  renderWindow();
  // clientHeight needs layout to settle (the element's CSS max-height
  // applies after it's in the DOM); recompute once on the next frame.
  requestAnimationFrame(() => {
    lastFirst = -1;
    renderWindow();
  });
}

/** One topic row: key + format pill + event-count. Whole-row click
 *  drills into the topic (opens the per-topic stream pane). */
function buildNsRow(
  row: NsTopicRow,
  onRowClick: (topic: string) => void,
): HTMLElement {
  const el = document.createElement("div");
  el.className = "ns-inspector-row";
  el.title = `click to inspect ${row.topic}`;
  el.addEventListener("click", () => onRowClick(row.topic));

  const name = document.createElement("span");
  name.className = "ns-inspector-row-name";
  name.title = row.topic;
  tagConsoleKey(name, row.topic);
  el.appendChild(name);

  const meta = document.createElement("span");
  meta.className = "ns-inspector-row-meta";
  const pill = document.createElement("span");
  pill.className = `topic-format topic-format-${row.format ?? "unknown"}`;
  pill.textContent = row.format ?? "—";
  pill.title = row.format ? `latest wire format: ${row.format}` : "no sample classified yet";
  meta.appendChild(pill);
  if (row.total_events > 0) {
    const ev = document.createElement("span");
    ev.className = "ns-inspector-row-events";
    ev.textContent = formatCompactNumber(row.total_events);
    ev.title = `${row.total_events} events`;
    meta.appendChild(ev);
  }
  el.appendChild(meta);
  return el;
}

function scopeTopics(
  publishes: string[],
  consumed: string[],
  scope: StreamScope,
): string[] {
  if (scope === "hears") return Array.from(new Set(consumed));
  if (scope === "says") return Array.from(new Set(publishes));
  return Array.from(new Set([...publishes, ...consumed]));
}

/** Render the HEARS / SAYS / ALL toggle. Mirrors the existing RAW /
 *  DECODED toggle's visual treatment (info-stream-mode buttons) so the
 *  two controls read as a pair. Hidden when the peer has only one
 *  direction (pure source / pure sink). */
function renderStreamScopeToggle(opts: {
  show: boolean;
  scope: StreamScope;
  onChange: (next: StreamScope) => void;
}): void {
  const host = document.getElementById("info-stream-scope-host");
  if (!host) return;
  while (host.firstChild) host.removeChild(host.firstChild);
  if (!opts.show) {
    host.classList.add("hidden");
    return;
  }
  host.classList.remove("hidden");
  const group = document.createElement("div");
  group.className = "info-stream-mode";
  group.setAttribute("role", "group");
  group.setAttribute("aria-label", "Stream scope (hears / says / all)");
  // Each button's aria-pressed gates the active-state CSS. Clicking a
  // sibling has to flip its own aria-pressed AND clear every other
  // button's — otherwise the visual stays stuck on the prior
  // selection even though the underlying state updated.
  const buttons: Array<{ btn: HTMLButtonElement; value: StreamScope }> = [];
  const mk = (label: string, value: StreamScope) => {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "info-stream-mode-btn";
    btn.setAttribute("aria-pressed", String(opts.scope === value));
    btn.textContent = label;
    btn.title =
      value === "hears" ? "show samples from topics this peer subscribes to"
        : value === "says" ? "show samples from topics this peer publishes"
          : "show samples from both directions";
    btn.addEventListener("click", () => {
      for (const { btn: b, value: v } of buttons) {
        b.setAttribute("aria-pressed", String(v === value));
      }
      opts.onChange(value);
    });
    buttons.push({ btn, value });
    return btn;
  };
  group.appendChild(mk("HEARS", "hears"));
  group.appendChild(mk("SAYS", "says"));
  group.appendChild(mk("ALL", "all"));
  host.appendChild(group);
}

function startStream(topics: string[]): void {
  const section = document.getElementById("info-stream");
  const topicLabel = document.getElementById("info-stream-topic-label");
  const streamBody = document.getElementById("info-stream-body");
  if (!section || !topicLabel || !streamBody) return;
  if (streamRefreshHandle !== null) {
    window.clearInterval(streamRefreshHandle);
    streamRefreshHandle = null;
  }
  // Reconcile the LIVE-follow watch set against the new selection.
  // unwatch(old ∖ new), watch(new ∖ old). No-op when streamLive=false.
  reconcileLiveWatch(topics);
  streamCurrentTopics = topics;
  if (topics.length === 0) {
    section.classList.add("hidden");
    streamBody.textContent = "";
    return;
  }
  section.classList.remove("hidden");
  topicLabel.textContent = topics.length === 1 ? topics[0] : `${topics.length} topics`;
  refreshStream();
  // Same tier-adaptive cadence as the fleet bar. AUSTERE tier has
  // streamPollMs=0 → manual refresh only (no auto re-fetch).
  const reschedule = () => {
    const ms = currentStreamPollMs();
    if (ms <= 0) return;
    streamRefreshHandle = window.setTimeout(async () => {
      if (streamCurrentTopics.length === 0) return;
      await refreshStream();
      reschedule();
    }, ms) as unknown as number;
  };
  reschedule();
}

function clearStreamBody(streamBody: HTMLElement): void {
  while (streamBody.firstChild) streamBody.removeChild(streamBody.firstChild);
}

function setStreamPlainText(streamBody: HTMLElement, text: string): void {
  clearStreamBody(streamBody);
  const pre = document.createElement("pre");
  pre.className = "stream-sample-body stream-sample-body--note";
  pre.textContent = text;
  streamBody.appendChild(pre);
}

/** Layer A of panoscope-stale-recall. Mark the stream pane stale when the
 *  newest rendered sample is older than a rate-aware threshold (~2.5× the
 *  topic's observed publish interval, with a `4× snapshot-poll` fast-topic
 *  floor). Earlier this was `recalled || age > 4× poll` — a flat ~6s that
 *  falsely flagged every healthy slow topic (a 0.1 Hz publisher's newest
 *  sample is routinely >6s old) and treated any storage recall as stale by
 *  fiat (a recent recall between beats of a slow-but-live topic is fine).
 *  Toggles the shared `.stale` class on the pane (clearing it when fresh)
 *  and, when stale, prepends a "last seen Ns ago" banner — noting "recalled
 *  from storage" when the value came from storage. Call only after at least
 *  one sample has rendered; `newestTsMs` is the newest sample's timestamp
 *  (for recalled samples that is the original publish time); `intervalMs`
 *  is the observed median inter-sample gap (0 when <2 samples). */
function applyStreamStaleness(
  streamBody: HTMLElement,
  newestTsMs: number,
  recalled: boolean,
  intervalMs: number,
): void {
  const ageMs = Date.now() - newestTsMs;
  // Rate-aware threshold. The old rule (`recalled || age > 4× poll`, i.e.
  // a flat 6s at LIVE) falsely flagged every healthy SLOW topic as STALE:
  // a 0.1 Hz synth-vendor publishes every 10s, so its newest sample is
  // routinely >6s old and the pane read STALE non-stop. And `recalled`
  // forced STALE by fiat — but a recent storage recall (the live ring is
  // momentarily empty between beats of a slow-but-live topic) is healthy,
  // not stale. So staleness is now purely age-vs-threshold, where the
  // threshold tracks the topic's own observed publish interval: stale only
  // once it's been ~2.5× the typical gap since the last sample (a genuinely
  // missed beat). `intervalMs` is the observed median inter-sample gap; 0
  // when we have <2 samples, in which case a generous floor avoids a
  // single recent recall reading STALE.
  const floor = 4 * currentSnapshotPollMs();
  const threshold =
    intervalMs > 0
      ? Math.max(floor, 2.5 * intervalMs)
      : Math.max(floor, STREAM_STALE_UNKNOWN_FLOOR_MS);
  const stale = ageMs > threshold;
  streamBody.classList.toggle("stale", stale);
  if (!stale) return;
  const ageS = Math.max(0, Math.round(ageMs / 1000));
  const banner = document.createElement("div");
  banner.className = "stream-stale-banner";
  banner.textContent = recalled
    ? `STALE · recalled from storage · last seen ${ageS} sec ago`
    : `STALE · last seen ${ageS} sec ago`;
  streamBody.insertBefore(banner, streamBody.firstChild);
}

async function refreshStream(): Promise<void> {
  const streamBody = document.getElementById("info-stream-body");
  if (!streamBody || streamCurrentTopics.length === 0) return;
  // Pause = stop pushing new content to the DOM (next refresh re-fetches
  // the latest, but only renders on resume). The refresh fetch still
  // happens so the LIVE view picks up where it left off on unpause —
  // intentional: pause is for inspection, not for archiving.
  if (streamPaused) return;
  // Unlike pause, these two mean the fetch itself is pointless.
  if (!shouldPoll(document.hidden, isPageSignedOut())) return;
  // Capture topics at call start; if streamCurrentTopics changes mid-fetch
  // (operator re-clicks) the stale result will be discarded at write time.
  const captured = streamCurrentTopics.slice();
  // Per-topic fetch cap: a multi-topic selection (e.g. a sink with many
  // consumes) was previously dominated by high-rate topics, slow ones
  // never appearing in the merged 30-sample window. Cap per-topic so
  // every consumed topic stays visible — small N when many topics, 15
  // when there's only one.
  const PER_TOPIC = captured.length > 1
    ? Math.max(2, Math.floor(30 / captured.length))
    : 15;
  if (streamMode === "decoded") {
    await refreshDecodedStream(streamBody, captured, PER_TOPIC);
  } else {
    await refreshRawStream(streamBody, captured, PER_TOPIC);
  }
}

async function refreshRawStream(
  streamBody: HTMLElement,
  captured: string[],
  perTopic: number,
): Promise<void> {
  try {
    const results = await Promise.all(
      captured.map(async (t) => {
        const resp = await fetch(`/api/samples?topic=${encodeURIComponent(t)}&limit=${perTopic}`);
        if (handleResponse(resp)) return [];
        if (!resp.ok) return [];
        const json = await resp.json();
        return (json.entries ?? []) as Array<{
          ts_ms: number;
          topic: string;
          source_peer: string | null;
          format: string;
          size_bytes: number;
          truncated: boolean;
          preview: string;
          stale?: boolean;
        }>;
      }),
    );
    if (selectionChanged(captured)) return;
    const merged = results.flat().sort((a, b) => b.ts_ms - a.ts_ms);
    setStateSummary(null);
    if (merged.length === 0) {
      setStreamPlainText(streamBody, "[no samples yet — waiting…]");
      return;
    }
    const FMT_ICON: Record<string, string> = {
      json:  "{ }",
      cbor:  "[*]",
      text:  "txt",
      hex:   "0x ",
      empty: "0  ",
      protobuf: "<p>",
    };
    clearStreamBody(streamBody);
    for (const e of merged) {
      const sample = document.createElement("div");
      sample.className = "stream-sample stream-sample--unevaluated";
      if (e.stale) sample.classList.add("stream-sample--stale");
      const header = document.createElement("div");
      header.className = "stream-sample-header";
      const headerText = document.createElement("span");
      headerText.className = "stream-sample-header-text";
      const ts = formatTs(e.ts_ms);
      const icon = FMT_ICON[e.format] ?? "??";
      const size = `${e.size_bytes}B${e.truncated ? "+" : ""}`;
      const peer = e.source_peer ? ` ← ${shortId(e.source_peer)}` : "";
      headerText.textContent = `${ts}  ${icon}  ${size.padEnd(7)} ${consoleKeyDisplay(e.topic)}${peer}`;
      headerText.title = e.topic;
      header.appendChild(headerText);
      sample.appendChild(header);
      const body = document.createElement("pre");
      body.className = "stream-sample-body";
      const preview = e.preview.trim();
      if (preview.length > 0) {
        const lines = preview.split("\n");
        const shown = lines.slice(0, 6).join("\n");
        body.textContent = lines.length > 6 ? `${shown}\n…` : shown;
      }
      sample.appendChild(body);
      streamBody.appendChild(sample);
    }
    // Layer A — pane-level staleness (newest first, so merged[0] is newest).
    applyStreamStaleness(
      streamBody,
      merged[0].ts_ms,
      merged.some((e) => e.stale === true),
      streamIntervalMs(merged.map((e) => ({ topic: e.topic, ts_ms: e.ts_ms }))),
    );
  } catch (e) {
    setStreamPlainText(streamBody, `[stream error: ${e}]`);
  }
}

async function refreshDecodedStream(
  streamBody: HTMLElement,
  captured: string[],
  perTopic: number,
): Promise<void> {
  if (captured.length === 1) {
    await refreshDecodedSingle(streamBody, captured, perTopic);
    return;
  }
  await refreshDecodedMulti(streamBody, captured, perTopic);
}

async function refreshDecodedSingle(
  streamBody: HTMLElement,
  captured: string[],
  perTopic: number,
): Promise<void> {
  const topic = captured[0];
  try {
    const resp = await fetch(
      `/api/samples/decoded?topic=${encodeURIComponent(topic)}&limit=${perTopic}`,
    );
    if (selectionChanged(captured)) return;
    if (!resp.ok) {
      streamBody.textContent = `[decode error: HTTP ${resp.status}]`;
      setSchemaBadge(null);
      return;
    }
    const outcome = (await resp.json()) as TopicDecodeOutcome;
    if (selectionChanged(captured)) return;
    renderDecodedOutcome(streamBody, outcome);
  } catch (e) {
    streamBody.textContent = `[decode error: ${e}]`;
    setSchemaBadge(null);
  }
}

/** Multi-topic decoded — fan out N parallel /api/samples/decoded
 *  fetches, one per topic in the current scope, then interleave their
 *  decoded samples by timestamp (newest first) into a single stream.
 *  Each sample carries its topic in the header so a sink reading from
 *  4 different schemas reads as one log with topic-tagged rows. The
 *  per-sample renderer (`renderDecodedSampleRow`) already takes the
 *  topic as an argument and routes the correct decoder per topic on
 *  the server side, so we just merge the resulting samples client-
 *  side; no decoder confusion across schemas. Topics whose schema is
 *  missing / disabled / errored contribute zero samples but their
 *  counts surface in the schema badge so the operator knows. */
async function refreshDecodedMulti(
  streamBody: HTMLElement,
  captured: string[],
  perTopic: number,
): Promise<void> {
  try {
    const outcomes = await Promise.all(
      captured.map(async (t): Promise<TopicDecodeOutcome | null> => {
        try {
          const resp = await fetch(
            `/api/samples/decoded?topic=${encodeURIComponent(t)}&limit=${perTopic}`,
          );
          if (!resp.ok) return null;
          return (await resp.json()) as TopicDecodeOutcome;
        } catch {
          return null;
        }
      }),
    );
    if (selectionChanged(captured)) return;
    // Topic-status tally for the badge — operator should see at a
    // glance whether the empty-looking stream is "no samples yet" or
    // "no schemas uploaded for these topics."
    let decodedTopics = 0;
    let passthroughTopics = 0;
    let noSchema = 0;
    let disabled = 0;
    let badSchema = 0;
    let transient = 0;
    let unreachable = 0;
    type StampedSample = {
      topic: string;
      row: DecodedSampleRow;
    };
    const merged: StampedSample[] = [];
    for (let i = 0; i < captured.length; i++) {
      const o = outcomes[i];
      if (o === null) {
        unreachable++;
        continue;
      }
      const s = o.status;
      if (s === "decoded") {
        decodedTopics++;
        for (const row of o.samples) merged.push({ topic: o.topic, row });
      } else if (s === "passthrough") {
        // No protobuf schema, but JSON/text shown as-is — its rows carry
        // per-sample PASSTHROUGH tags, so merge them in alongside decoded.
        passthroughTopics++;
        for (const row of o.samples) merged.push({ topic: o.topic, row });
      } else if (s === "no_schema") {
        noSchema++;
      } else if (s === "portal_disabled") {
        disabled++;
      } else if (s === "empty_descriptor") {
        badSchema++;
      } else if (typeof s === "object" && "transient" in s) {
        transient++;
      }
    }

    // Only short-circuit to "decoder off" when NOTHING rendered — neither a
    // protobuf decode nor a JSON/text passthrough (passthrough needs no
    // portal, so a disabled portal doesn't blank a JSON topic).
    if (disabled > 0 && decodedTopics === 0 && passthroughTopics === 0) {
      setStreamPlainText(
        streamBody,
        "[decoder not configured at this site]\n\n" +
          "Set PANOSCOPE_PORTAL_API_KEY in the panoscope deployment\n" +
          "and re-apply the role for the Decoded view to fetch schemas\n" +
          "from the portal.",
      );
      setSchemaBadge(
        "DECODER OFF",
        "PANOSCOPE_PORTAL_API_KEY not set on this panoscope instance",
      );
      setStateSummary(null);
      return;
    }

    const badgeText =
      passthroughTopics > 0
        ? `${decodedTopics} DECODED · ${passthroughTopics} PASSTHROUGH /${captured.length}`
        : `${decodedTopics}/${captured.length} TOPICS DECODED`;
    setSchemaBadge(
      badgeText,
      `${decodedTopics} decoded · ${passthroughTopics} passthrough (no protobuf schema, raw JSON/text) · ${noSchema} no-schema · ${badSchema} bad-schema · ${transient} transient · ${unreachable} unreachable`,
    );

    if (merged.length === 0) {
      const counts = noSchema + badSchema + transient + unreachable;
      setStreamPlainText(
        streamBody,
        counts > 0
          ? `[no decodable samples — ${noSchema} topic(s) without schema, ${badSchema} bad, ${transient} transient, ${unreachable} unreachable]`
          : "[no samples yet — waiting…]",
      );
      setStateSummary({ working: 0, failing: 0, noraw: 0, total: 0 });
      return;
    }

    // Newest-first, matches the raw-stream merge order.
    merged.sort((a, b) => b.row.ts_ms - a.row.ts_ms);

    let working = 0;
    let failing = 0;
    let noraw = 0;
    let passthrough = 0;
    let filteredOut = 0;
    clearStreamBody(streamBody);
    for (const m of merged) {
      const row = renderDecodedSampleRow(m.topic, m.row);
      if (row === null) {
        filteredOut++;
        continue;
      }
      if (row.state === "working") working++;
      else if (row.state === "failing") failing++;
      else if (row.state === "noraw") noraw++;
      else if (row.state === "passthrough") passthrough++;
      streamBody.appendChild(row.node);
    }
    if (streamBody.childElementCount === 0) {
      setStreamPlainText(
        streamBody,
        `[filter eliminated all ${merged.length} samples — clear the jq input to see them]`,
      );
    } else {
      // Layer A — merged is sorted newest-first above; merged[0] is newest.
      applyStreamStaleness(
        streamBody,
        merged[0].row.ts_ms,
        merged.some((m) => m.row.stale === true),
        streamIntervalMs(merged.map((m) => ({ topic: m.topic, ts_ms: m.row.ts_ms }))),
      );
    }
    setStateSummary({
      working,
      failing,
      noraw,
      passthrough,
      total: merged.length,
      filtered: filteredOut,
    });
  } catch (e) {
    streamBody.textContent = `[decode error: ${e}]`;
    setSchemaBadge(null);
  }
}

function renderDecodedOutcome(streamBody: HTMLElement, outcome: TopicDecodeOutcome): void {
  const status = outcome.status;
  if (status === "no_schema") {
    setStreamPlainText(
      streamBody,
      `[no schema uploaded for ${outcome.topic}]\n\n` +
        `The portal does not have a .fds (protobuf descriptor) for this\n` +
        `topic. To enable decoding, upload one via the portal's topics page\n` +
        `or the bootstrap-notional-schemas.sh script.`,
    );
    setSchemaBadge("NO SCHEMA", "Portal returned 404 for this topic's .fds — upload one via portal /vendor.html");
    setStateSummary(null);
    return;
  }
  if (status === "portal_disabled") {
    setStreamPlainText(
      streamBody,
      `[decoder not configured at this site]\n\n` +
        `Set PANOSCOPE_PORTAL_API_KEY in the panoscope deployment and\n` +
        `re-apply the role for the Decoded view to fetch schemas from the\n` +
        `portal.`,
    );
    setSchemaBadge("DECODER OFF", "PANOSCOPE_PORTAL_API_KEY not set on this panoscope instance");
    setStateSummary(null);
    return;
  }
  if (status === "empty_descriptor") {
    setStreamPlainText(
      streamBody,
      `[schema descriptor is empty]\n\n` +
        `The portal returned a .fds for this topic, but it contains no\n` +
        `message types. Re-upload a valid FileDescriptorSet.`,
    );
    setSchemaBadge("BAD SCHEMA", "Uploaded .fds declares no message types");
    setStateSummary(null);
    return;
  }
  if (typeof status === "object" && "transient" in status) {
    setStreamPlainText(streamBody, `[transient decode failure: ${status.transient}]`);
    setSchemaBadge("RETRY", "Transient error reaching the portal; will retry");
    setStateSummary(null);
    return;
  }
  // status === "decoded" or "passthrough" (no protobuf schema, JSON/text
  // shown as-is). The badge distinguishes them so the operator knows whether
  // bytes were schema-validated.
  const isPassthrough = status === "passthrough";
  setSchemaBadge(
    isPassthrough ? "PASSTHROUGH" : null,
    isPassthrough
      ? "No protobuf schema for this topic — raw JSON/text shown exactly as published, not protobuf-decoded"
      : undefined,
  );
  if (outcome.samples.length === 0) {
    setStreamPlainText(streamBody, "[no samples yet — waiting…]");
    setStateSummary({ working: 0, failing: 0, noraw: 0, total: 0 });
    return;
  }
  // Roll counters as we render so the header summary reflects only what's
  // actually on screen (not what came back from the API — they're the same
  // here, but the invariant matters once filtering hides rows).
  let working = 0;
  let failing = 0;
  let noraw = 0;
  let passthrough = 0;
  let filteredOut = 0;
  clearStreamBody(streamBody);
  if (isPassthrough) appendPassthroughNote(streamBody);
  for (const s of outcome.samples) {
    const row = renderDecodedSampleRow(outcome.topic, s);
    if (row === null) {
      filteredOut++;
      continue;
    }
    if (row.state === "working") working++;
    else if (row.state === "failing") failing++;
    else if (row.state === "noraw") noraw++;
    else if (row.state === "passthrough") passthrough++;
    streamBody.appendChild(row.node);
  }
  if (streamBody.childElementCount === 0) {
    setStreamPlainText(
      streamBody,
      `[filter eliminated all ${outcome.samples.length} samples — clear the jq input to see them]`,
    );
  } else {
    // Layer A — samples are newest-first; samples[0] is newest.
    applyStreamStaleness(
      streamBody,
      outcome.samples[0].ts_ms,
      outcome.samples.some((s) => s.stale === true),
      medianIntervalMs(outcome.samples.map((s) => s.ts_ms)),
    );
  }
  setStateSummary({
    working,
    failing,
    noraw,
    passthrough,
    total: outcome.samples.length,
    filtered: filteredOut,
  });
}

/** Pane-level note prepended above passthrough rows, so the "not decoded"
 *  framing is stated once in prose even before the operator reads the
 *  per-row PASSTHROUGH tags. */
function appendPassthroughNote(streamBody: HTMLElement): void {
  const note = document.createElement("div");
  note.className = "stream-passthrough-note";
  note.textContent =
    "PASSTHROUGH — no protobuf schema for this topic; showing raw JSON/text as published (not protobuf-decoded).";
  streamBody.appendChild(note);
}

interface RenderedSampleRow {
  node: HTMLElement;
  state: "working" | "failing" | "noraw" | "passthrough";
}

function renderDecodedSampleRow(topic: string, s: DecodedSampleRow): RenderedSampleRow | null {
  const sample = document.createElement("div");
  sample.className = "stream-sample";
  if (s.stale) sample.classList.add("stream-sample--stale");
  const header = document.createElement("div");
  header.className = "stream-sample-header";
  const headerText = document.createElement("span");
  headerText.className = "stream-sample-header-text";
  const ts = formatTs(s.ts_ms);
  const size = `${s.size_bytes}B`;
  const peer = s.source_peer ? ` ← ${shortId(s.source_peer)}` : "";
  headerText.textContent = `${ts}  ${size.padEnd(7)} ${consoleKeyDisplay(topic)}${peer}`;
  headerText.title = topic;
  header.appendChild(headerText);

  const body = document.createElement("pre");
  body.className = "stream-sample-body";

  let state: RenderedSampleRow["state"];
  let copyValue: string | null = null;

  if (s.result === "no_raw_bytes") {
    sample.classList.add("stream-sample--noraw");
    body.classList.add("stream-sample-body--note");
    body.textContent = "[raw bytes not retained — sample exceeded 4 KiB cap]";
    state = "noraw";
  } else if ("error" in s.result) {
    sample.classList.add("stream-sample--failing");
    body.classList.add("stream-sample-body--error");
    body.textContent = `decode error: ${s.result.error}`;
    state = "failing";
  } else if ("passthrough" in s.result) {
    // NOT a protobuf decode — the topic has no .fds, so the raw JSON/text
    // is shown as published. Tag it loudly so the operator never reads it
    // as schema-validated. (Backend only emits this for json/text formats.)
    const pt = s.result.passthrough;
    sample.classList.add("stream-sample--passthrough");
    const tag = document.createElement("span");
    tag.className = "stream-sample-passthrough-tag";
    tag.textContent = `PASSTHROUGH · ${pt.format.toUpperCase()}`;
    tag.title = `No protobuf schema for this topic — raw ${pt.format} shown exactly as published on the wire, not protobuf-decoded.`;
    header.appendChild(tag);
    if (pt.format === "json") {
      // JSON passthrough honors the same jq-lite filter + formatting as a
      // real decode (the body is already a structured value).
      const value = pt.body as JsonValue;
      const values = streamFilter ? streamFilter(value) : [value];
      if (values.length === 0) return null;
      const text = values
        .map((v) =>
          streamJsonFormat === "compact" ? JSON.stringify(v) : JSON.stringify(v, null, 2),
        )
        .join("\n");
      body.textContent = text;
      copyValue = text;
    } else {
      // Text passthrough — show the string verbatim (no jq).
      const text = String(pt.body);
      body.textContent = text;
      copyValue = text;
    }
    state = "passthrough";
  } else {
    // Decoded → apply the optional jq-lite filter, then format. The filter
    // can fan a single sample into N output values (e.g. `.arr[]`) or zero
    // (e.g. `select(.kind == "air")` against a non-matching sample).
    const decoded = s.result.decoded as JsonValue;
    const values = streamFilter ? streamFilter(decoded) : [decoded];
    if (values.length === 0) {
      // Sample passed the descriptor cleanly but was filtered out.
      return null;
    }
    sample.classList.add("stream-sample--working");
    const text = values
      .map((v) =>
        streamJsonFormat === "compact"
          ? JSON.stringify(v)
          : JSON.stringify(v, null, 2),
      )
      .join("\n");
    body.textContent = text;
    copyValue = text;
    state = "working";
  }

  if (copyValue !== null) {
    const copyBtn = document.createElement("button");
    copyBtn.type = "button";
    copyBtn.className = "stream-sample-copy";
    copyBtn.textContent = "COPY";
    copyBtn.title = "Copy decoded JSON to clipboard";
    copyBtn.addEventListener("click", () => {
      void copyToClipboard(copyValue).then((ok) => {
        if (!ok) return;
        copyBtn.setAttribute("data-state", "copied");
        copyBtn.textContent = "COPIED";
        window.setTimeout(() => {
          copyBtn.removeAttribute("data-state");
          copyBtn.textContent = "COPY";
        }, 1500);
      });
    });
    header.appendChild(copyBtn);
  }

  // Append the header (timestamp / topic / peer / passthrough tag / COPY)
  // ahead of the body — mirrors the raw-stream row order. This line was
  // missing, so decoded rows silently dropped their header + COPY button;
  // surfaced while adding the passthrough tag (which lives in the header).
  sample.appendChild(header);
  sample.appendChild(body);
  return { node: sample, state };
}

async function copyToClipboard(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
      return true;
    }
    // Fallback for non-secure contexts (rare; mesh is HTTPS-only).
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(ta);
    return ok;
  } catch {
    return false;
  }
}

interface StateSummary {
  working: number;
  failing: number;
  noraw: number;
  total: number;
  filtered?: number;
  passthrough?: number;
}

function setStateSummary(s: StateSummary | null): void {
  const el = document.getElementById("info-stream-state-summary");
  if (!el) return;
  if (s === null) {
    el.classList.add("hidden");
    el.textContent = "";
    el.removeAttribute("data-state");
    return;
  }
  el.classList.remove("hidden");
  const parts: string[] = [];
  if (s.working > 0 || (s.passthrough ?? 0) === 0) parts.push(`${s.working} ok`);
  if (s.passthrough && s.passthrough > 0) parts.push(`${s.passthrough} passthrough`);
  if (s.failing > 0) parts.push(`${s.failing} err`);
  if (s.noraw > 0) parts.push(`${s.noraw} no-raw`);
  if (s.filtered && s.filtered > 0) parts.push(`${s.filtered} filtered`);
  el.textContent = parts.join(" · ");
  // Aggregate tri-color hint: green when all working, red when none, amber
  // for any mix that isn't all-green / all-red. Passthrough is a legitimate
  // render (not an error) but isn't a schema-validated decode → it tips a
  // working-only pane to "mixed" so the summary colour matches the amber
  // PASSTHROUGH row tags rather than reading as all-green decoded.
  const pass = s.passthrough ?? 0;
  let tone: "working" | "mixed" | "failing";
  if (s.total === 0 || (s.failing === 0 && s.noraw === 0 && pass === 0 && s.working > 0)) {
    tone = "working";
  } else if (s.working === 0 && pass === 0) {
    tone = "failing";
  } else {
    tone = "mixed";
  }
  el.setAttribute("data-state", tone);
}

function selectionChanged(captured: string[]): boolean {
  return (
    streamCurrentTopics.length !== captured.length ||
    streamCurrentTopics.some((t, i) => t !== captured[i])
  );
}

function formatTs(tsMs: number): string {
  return (
    new Date(tsMs).toLocaleTimeString("en-US", { hour12: false }) +
    "." +
    String(tsMs % 1000).padStart(3, "0")
  );
}

/** Set or clear the NO SCHEMA / DECODER OFF / etc. badge next to the
 *  Decoded toggle. Passing null hides the badge. */
function setSchemaBadge(text: string | null, tooltip?: string): void {
  const badge = document.getElementById("info-stream-schema-badge");
  if (!badge) return;
  if (text === null) {
    badge.classList.add("hidden");
    badge.textContent = "";
    badge.removeAttribute("title");
    return;
  }
  badge.classList.remove("hidden");
  badge.textContent = text;
  if (tooltip) badge.setAttribute("title", tooltip);
  else badge.removeAttribute("title");
}

/** Apply the cached decoder-enabled flag to the Decoded toggle button.
 *  Called whenever the legend response refreshes — until /api/legend
 *  comes back, the toggle defaults to enabled-with-fallback (clicking
 *  surfaces PORTAL_DISABLED if it turns out to not be configured). */
function applyDecoderEnabledToStreamToggle(): void {
  const btn = document.getElementById("info-stream-mode-decoded") as
    | HTMLButtonElement
    | null;
  if (!btn) return;
  if (decoderEnabled) {
    btn.disabled = false;
    btn.removeAttribute("title");
  } else {
    btn.disabled = false; // still clickable — see PORTAL_DISABLED status
    btn.setAttribute(
      "title",
      "PANOSCOPE_PORTAL_API_KEY not set; click to see deploy-gap hint",
    );
  }
}

function installStreamModeToggle(): void {
  const raw = document.getElementById("info-stream-mode-raw");
  const dec = document.getElementById("info-stream-mode-decoded");
  if (!raw || !dec) return;
  const apply = () => {
    raw.setAttribute("aria-pressed", String(streamMode === "raw"));
    dec.setAttribute("aria-pressed", String(streamMode === "decoded"));
    if (streamMode === "raw") {
      setSchemaBadge(null);
      setStateSummary(null);
    }
    applyToolkitVisibility();
  };
  raw.addEventListener("click", () => {
    if (streamMode === "raw") return;
    streamMode = "raw";
    apply();
    void refreshStream();
  });
  dec.addEventListener("click", () => {
    if (streamMode === "decoded") return;
    streamMode = "decoded";
    apply();
    void refreshStream();
  });
  apply();
}

/** Hide / show the toolkit container based on stream mode. Toolkit only
 *  appears in DECODED mode — its controls (jq filter, pretty/compact)
 *  are no-ops against the RAW hex view. */
function applyToolkitVisibility(): void {
  const toolkit = document.getElementById("info-stream-toolkit");
  if (!toolkit) return;
  if (streamMode === "decoded") toolkit.classList.remove("hidden");
  else toolkit.classList.add("hidden");
}

/** Wire the jq filter input + pretty/compact toggle. Recompiles the filter
 *  on every input change; surfaces structured parse errors in the error
 *  banner; triggers a stream refresh so the change is immediately visible. */
function installStreamToolkit(): void {
  const filterInput = document.getElementById("info-stream-filter") as HTMLInputElement | null;
  const filterError = document.getElementById("info-stream-filter-error");
  const pretty = document.getElementById("info-stream-fmt-pretty");
  const compact = document.getElementById("info-stream-fmt-compact");
  if (!filterInput || !filterError || !pretty || !compact) return;

  const applyFormat = () => {
    pretty.setAttribute("aria-pressed", String(streamJsonFormat === "pretty"));
    compact.setAttribute("aria-pressed", String(streamJsonFormat === "compact"));
  };
  pretty.addEventListener("click", () => {
    if (streamJsonFormat === "pretty") return;
    streamJsonFormat = "pretty";
    applyFormat();
    void refreshStream();
  });
  compact.addEventListener("click", () => {
    if (streamJsonFormat === "compact") return;
    streamJsonFormat = "compact";
    applyFormat();
    void refreshStream();
  });
  applyFormat();

  const onFilterChange = () => {
    const src = filterInput.value;
    const result = compileFilter(src);
    if ("error" in result) {
      streamFilter = null;
      filterError.classList.remove("hidden");
      filterError.textContent = `jq parse error at col ${result.error.col}: ${result.error.message}`;
      filterInput.setAttribute("aria-invalid", "true");
    } else {
      streamFilter = result.filter;
      filterError.classList.add("hidden");
      filterError.textContent = "";
      filterInput.removeAttribute("aria-invalid");
    }
    void refreshStream();
  };
  // Debounce keystrokes a touch so very large streams don't relayout on
  // every character. 150ms is below the perception threshold; feels live.
  let debounceHandle: number | null = null;
  filterInput.addEventListener("input", () => {
    if (debounceHandle !== null) window.clearTimeout(debounceHandle);
    debounceHandle = window.setTimeout(() => {
      debounceHandle = null;
      onFilterChange();
    }, 150) as unknown as number;
  });

  applyToolkitVisibility();
}

function appendRow(parent: HTMLElement, label: string, value: string): void {
  const dt = document.createElement("dt");
  dt.textContent = label;
  const dd = document.createElement("dd");
  dd.textContent = value;
  parent.appendChild(dt);
  parent.appendChild(dd);
}

/** Inbound URL params per Block 94 cross-surface deep-link contract
 *  (see docs/design/zenoh-surface-deep-links.md §"URL parameter canon").
 *  More specific wins on conflict: `key` > `vendor` > `peer`. */
interface InboundSelection {
  key?: string;
  vendor?: string;
  peer?: string;
}

function parseInboundSelection(search: string): InboundSelection | null {
  const qs = new URLSearchParams(search);
  const sel: InboundSelection = {};
  const key = qs.get("key");
  const vendor = qs.get("vendor");
  const peer = qs.get("peer");
  if (key) sel.key = key;
  if (vendor) sel.vendor = vendor;
  if (peer) sel.peer = peer;
  return sel.key || sel.vendor || sel.peer ? sel : null;
}

/** Resolve an inbound selection to renderer node IDs against a fresh
 *  snapshot. Empty array = target not yet present in the snapshot;
 *  caller can re-try on the next snapshot.
 *
 *  Resolution order matches the Block 94 contract's specificity rule
 *  (`key` > `vendor` > `peer`). Vendor selects every peer carrying
 *  that vendor tag (multi-select); peer is a single ID match; key
 *  resolves to the topic's PUBLISHER peer(s) — the real graph nodes
 *  that render a proper side panel. */
function resolveInboundSelection(
  sel: InboundSelection,
  view: ViewResponse,
): string[] {
  if (sel.key) {
    // Resolve a topic key to its publisher peer(s): real graph nodes whose
    // side panel lists the topic, which is "show me X in the fabric". A
    // shared pin names its publishers; a topic inside a group is attributed
    // by the group's selector. Returning [] when no publisher is drawn yet
    // keeps the request pending for a later view.
    const publishers = new Set<string>();
    for (const t of view.topics) {
      if (t.key === sel.key) t.publishers.forEach((p) => publishers.add(p));
    }
    const segs = sel.key.split("/");
    for (const g of view.groups) {
      if (selectorMatches(g.selector.split("/"), segs)) publishers.add(g.publisher);
    }
    return view.peers.filter((p) => publishers.has(p.peer_id)).map((p) => p.peer_id);
  }
  if (sel.vendor) {
    const org = view.orgs.find((o) => o.org === sel.vendor);
    if (org) return [org.id];
    return view.peers.filter((p) => p.vendor === sel.vendor).map((p) => p.peer_id);
  }
  if (sel.peer) {
    return view.peers.some((p) => p.peer_id === sel.peer) ? [sel.peer] : [];
  }
  return [];
}

/** Build a sibling-surface deep-link URL per the Block 94 cross-
 *  surface contract (see docs/design/zenoh-surface-deep-links.md).
 *  Returns null when the sibling base URL is unset — callers hide
 *  the corresponding outbound button rather than emitting a dead href.
 *
 *  Param canon: `key=<zenoh-key-expr>` for topics, `vendor=<tag>` for
 *  vendors, `peer=<id>` for panoscope-only peer selection. The helper
 *  encodes values via `encodeURIComponent` (key expressions contain
 *  `/` which must be percent-encoded as a query value). */
function siblingUrl(
  siblings: SiblingUrls,
  target: "stethoscope" | "flashlight" | "portal-schema",
  params: { key?: string; vendor?: string; peer?: string },
): string | null {
  const base =
    target === "stethoscope"
      ? siblings.stethoscope
      : target === "flashlight"
        ? siblings.flashlight
        : siblings.portal;
  if (!base) return null;
  const qs = new URLSearchParams();
  if (params.key) qs.set("key", params.key);
  if (params.vendor) qs.set("vendor", params.vendor);
  if (params.peer) qs.set("peer", params.peer);
  if (target === "portal-schema") {
    // Portal schema viewer lives at /schema.html and consumes `key=`
    // exclusively. Vendor/peer params are silently dropped — the
    // schema page is topic-scoped.
    return `${base}/schema.html?${qs.toString()}`;
  }
  return `${base}/?${qs.toString()}`;
}

/** Render a per-selection sibling-action row in the side panel:
 *  `diagnose ↗` (stethoscope) + `tail ↗` (flashlight) + `schema ↗`
 *  (portal). Each button is conditional on the corresponding sibling
 *  base URL being set — a deployment with no stethoscope just doesn't
 *  render the diagnose button (Block 94 contract). Topic-broker pins
 *  pass `key`, application peers pass `vendor` and/or `peer`. */
function appendSiblingActionRow(
  parent: HTMLElement,
  siblings: SiblingUrls,
  target: { key?: string; vendor?: string; peer?: string },
): void {
  // Stethoscope: prefer `key` (topic-level) → `vendor` (peer-level)
  // since stethoscope's target picker auto-detects kind from key shape.
  const stethoParams = target.key
    ? { key: target.key }
    : target.vendor
      ? { vendor: target.vendor }
      : {};
  const flashParams = target.key
    ? { key: target.key }
    : target.vendor
      ? { vendor: target.vendor }
      : {};
  const stethoUrl =
    Object.keys(stethoParams).length > 0
      ? siblingUrl(siblings, "stethoscope", stethoParams)
      : null;
  const flashUrl =
    Object.keys(flashParams).length > 0
      ? siblingUrl(siblings, "flashlight", flashParams)
      : null;
  const schemaUrl = target.key
    ? siblingUrl(siblings, "portal-schema", { key: target.key })
    : null;
  // Hide the entire row when no buttons would render — avoids an
  // empty visual gutter on selections that don't compose with any
  // sibling.
  if (!stethoUrl && !flashUrl && !schemaUrl) return;
  const row = document.createElement("div");
  row.className = "sibling-action-row";
  // Icon + label links — consistent icon set across the surface, no
  // mixed glyphs/arrows. The action row has room for the larger
  // stethoscope mark; the per-topic list uses the compact `health` mark.
  const add = (
    url: string | null,
    cls: string,
    icon: IconName,
    label: string,
    title: string,
  ): void => {
    if (!url) return;
    const a = document.createElement("a");
    a.className = `sibling-action ${cls}`;
    a.href = url;
    a.target = "_blank";
    a.rel = "noopener noreferrer";
    a.title = title;
    a.appendChild(makeIcon(icon, { size: 15 }));
    const span = document.createElement("span");
    span.className = "sibling-action-label";
    span.textContent = label;
    a.appendChild(span);
    row.appendChild(a);
  };
  add(stethoUrl, "sibling-action-diagnose", "stethoscope", "diagnose", "open this target in stethoscope");
  add(flashUrl, "sibling-action-tail", "flashlight", "tail", "tail this target in flashlight");
  add(schemaUrl, "sibling-action-schema", "registry", "schema", "open schema viewer in portal");
  parent.appendChild(row);
}

/** Short uppercase label for an infrastructure subkind, appended to the
 *  INFRASTRUCTURE badge (e.g. "INFRASTRUCTURE · ROUTER"). Returns "" for
 *  the generic self-declared case so the badge stays just
 *  "INFRASTRUCTURE". Presentation copy lives here; the classification
 *  (which subkind) is decided backend-side in PeerInfo::node_subkind. */
function infraSubkindLabel(subkind: string | null | undefined): string {
  switch (subkind) {
    case "router":
      return "ROUTER";
    case "subscriber":
      return "OBSERVER";
    case "chaos":
      return "CHAOS";
    case "schema-registry":
      return "SCHEMA REGISTRY";
    case "operator-system":
      return "OPERATOR/SYSTEM";
    default:
      return "";
  }
}

/** Side-panel `kind` + `description` for an infrastructure node, keyed
 *  off the backend subkind. The shared message across every case: this is
 *  underlying substrate, NOT an application data participant in the Zenoh
 *  ecosystem — so a new operator can tell at a glance that attest /
 *  trashgoat / system / router nodes are different in kind from the
 *  vendors whose mission data the fabric carries. */
function infraPanelInfo(subkind: string | null | undefined): {
  kind: string;
  description: string;
} {
  switch (subkind) {
    case "router":
      return {
        kind: "router (infrastructure)",
        description:
          "Zenoh broker — the substrate every peer connects through. " +
          "Routes pub/sub traffic between application peers; does not " +
          "publish or subscribe mission data on its own.",
      };
    case "subscriber":
      return {
        kind: "observer subscriber (infrastructure)",
        description:
          "Substrate observer — subscribes across the fabric to mirror " +
          "traffic (e.g. the audit-subscriber writing the tamper-evident " +
          "log). Consumes for integrity / observability; never originates " +
          "mission data, so it is not a data participant.",
      };
    case "chaos":
      return {
        kind: "chaos generator (infrastructure)",
        description:
          "trashgoat — platform-internal chaos actor publishing deliberate " +
          "garbage under trashgoat/** to exercise the fabric. Synthetic by " +
          "design; not real mission data and not a data participant.",
      };
    case "schema-registry":
      return {
        kind: "schema registry (infrastructure)",
        description:
          "Publishes only under the reserved attest/ namespace — schema " +
          "declarations describing other peers' topics. Metadata about the " +
          "fabric, not mission data.",
      };
    case "operator-system":
      return {
        kind: "operator/control (infrastructure)",
        description:
          "Publishes only under the reserved operator namespace (ops/ — " +
          "announcements, fabric health, per-service control planes; or the " +
          "deprecated system/). Operator control-plane traffic — substrate, " +
          "not an application data participant.",
      };
    default:
      return {
        kind: "infrastructure",
        description:
          "Self-declared underlying infrastructure (infra: true). Part of " +
          "the substrate that runs the fabric, not an application data " +
          "participant.",
      };
  }
}

/** Append the fold/unfold control to the side panel for the selected
 *  node — the discoverable, labeled twin of the canvas double-click
 *  gesture (see docs/design/panoscope.md §"Fold/unfold interaction").
 *  One labeled button per applicable action; the label states the scope
 *  ("this peer" vs "vendor «X»") so the outcome is never ambiguous:
 *    - collapsed organisation → "▾ expand vendor"
 *    - application peer → "▭ collapse vendor «X»" (when vendored) +
 *                         "▸ fold peer" / "▾ unfold peer" +
 *                         "▴ close N opened groups" (when any are open)
 *    - group, topic pin, infrastructure → no fold control (groups have their
 *      own inspector; pins and infrastructure are not foldable)
 *  Every selectable element also gets "◎ isolate what this connects to". */
function appendFoldActionRow(
  parent: HTMLElement,
  n: Parameters<GraphRenderer["toggleFold"]>[0],
): void {
  const rndr = (window as unknown as { __panoscopeRenderer?: GraphRenderer })
    .__panoscopeRenderer;
  if (!rndr) return;
  const m = n as {
    is_vendor_super?: boolean;
    is_singleton_super?: boolean;
    singleton_count?: number;
    vendor?: string | null;
    role?: string;
    node_kind?: string | null;
    node_class?: string | null;
  };
  const row = document.createElement("div");
  row.className = "fold-action-row";
  const mkBtn = (label: string, title: string, onClick: () => void) => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "fold-action-btn";
    b.textContent = label;
    b.title = title;
    b.addEventListener("click", onClick);
    row.appendChild(b);
  };

  const rndrFocus = rndr;
  mkBtn(
    rndrFocus.focusInfo() ? "◎ re-isolate on this" : "◎ isolate what this connects to",
    "draw only this element's connected component and hide the rest (f; esc to leave)",
    () => {
      rndrFocus.focusComponent([(n as { id: string }).id]);
      renderFocusBanner(rndrFocus);
    },
  );

  if (m.is_vendor_super && m.vendor) {
    mkBtn(
      `▾ expand ${LEX.org}`,
      `show this ${LEX.org}'s peers + topics (or double-click the node)`,
      () => rndr.toggleFold(n),
    );
  } else if (m.is_singleton_super) {
    // Groups have their own inspector with the canvas actions; nothing here.
    return;
  } else if (m.role !== "topic" && m.node_class !== "infra") {
    // Application peer. The common group operation (collapse the whole
    // vendor) comes first when the peer is vendored; the peer-level fold
    // is the secondary control. Both are explicitly scoped in the label.
    // Infrastructure nodes are not foldable (substrate, not a vendor's
    // fan-out) — generalized from the router-only skip.
    if (m.vendor) {
      const v = m.vendor;
      mkBtn(
        `▭ collapse ${LEX.org} «${v}»`,
        `fold this whole ${LEX.org} into one box (or double-click its cluster)`,
        () => rndr.collapseVendor(v),
      );
    }
    const peerId = (n as { id: string }).id;
    const folded = viewClient.get().fold_peers.includes(peerId);
    mkBtn(
      folded ? "▾ unfold peer" : "▸ fold peer",
      folded
        ? "show this peer's topics grouped normally again (or double-click the peer)"
        : "fold this peer's outgoing topics into one box (or double-click the peer)",
      () => rndr.toggleFold(n),
    );
    const opened = viewClient.get().expand.filter((g) => g.startsWith(`grp:${peerId}:`)).length;
    if (opened > 0) {
      mkBtn(
        `▴ close ${opened} opened group${opened === 1 ? "" : "s"}`,
        "return this peer's opened groups to the default grouping",
        () => viewClient.closeGroupsOf(peerId),
      );
    }
  }
  // Topic pins and infrastructure are not foldable; they still isolate.
  parent.appendChild(row);
}

/** Append a sibling-surface outbound link icon to the given parent
 *  span. No-op when the target sibling has no configured base URL —
 *  this is how we keep the contract URL-only with no runtime coupling
 *  (a deployment without stethoscope just doesn't render the button). */
function appendSiblingIconLink(
  parent: HTMLElement,
  url: string | null,
  className: string,
  icon: IconName,
  title: string,
): void {
  if (!url) return;
  const a = document.createElement("a");
  a.className = className;
  a.href = url;
  a.target = "_blank";
  a.rel = "noopener noreferrer";
  a.title = title;
  a.appendChild(makeIcon(icon, { size: 13, title }));
  a.addEventListener("click", (e) => e.stopPropagation());
  parent.appendChild(a);
}

/** Render one topic <li> with name + format pill + per-topic sibling
 *  links (schema-page · diagnose · tail). Shared by appendTopicList's
 *  inline + overflow branches so the two paths stay visually identical.
 *
 *  Sibling buttons are individually conditional on the corresponding
 *  base URL being configured — a deployment without a stethoscope
 *  sibling just doesn't render the diagnose button, per the Block 94
 *  contract. */
// ---------------------------------------------------------------------------
// Console UUID display — shortened by default to reclaim the scarce side-panel
// width, expandable to full on operator demand via the info-panel `UUID`
// toggle. Element-based keys (topic lists, namespace-inspector rows, the
// panel title) are tagged `.uuid-key` carrying their full key in
// `data-fullkey`; the toggle walks those + re-applies. Stream lines compose
// their text fresh each refresh, so they read the flag at compose time and
// self-update on the next sample. Graph labels are always shortened (no
// expand) — see `shortenUuids` use in render.ts.
// ---------------------------------------------------------------------------
let consoleExpandUuids = false;

/** Visible text for a key in the console, honoring the expand toggle. */
function consoleKeyDisplay(fullKey: string): string {
  return consoleExpandUuids ? fullKey : shortenUuids(fullKey);
}

/** Tag an element as a toggleable console key: stash the full key, set the
 *  (possibly shortened) visible text, and keep the full key on hover. */
function tagConsoleKey(el: HTMLElement, fullKey: string): void {
  el.dataset.fullkey = fullKey;
  el.classList.add("uuid-key");
  el.textContent = consoleKeyDisplay(fullKey);
  if (!el.title) el.title = fullKey;
}

/** Re-apply the current expand state to every tagged console key. Called by
 *  the info-panel UUID toggle; no full re-render needed. */
function refreshConsoleKeys(): void {
  document
    .querySelectorAll<HTMLElement>("#info-panel .uuid-key")
    .forEach((el) => {
      const f = el.dataset.fullkey;
      if (f) el.textContent = consoleKeyDisplay(f);
    });
}

/** Wire the info-panel `UUID short/full` toggle once at startup. */
function installUuidToggle(): void {
  const btn = document.getElementById("info-uuid-toggle");
  if (!btn) return;
  const sync = () => {
    btn.dataset.expand = consoleExpandUuids ? "full" : "short";
    btn.textContent = consoleExpandUuids ? "UUID·full" : "UUID·short";
    btn.title = consoleExpandUuids
      ? "UUIDs shown in full — click to shorten"
      : "UUIDs shortened — click to show full";
  };
  sync();
  btn.addEventListener("click", () => {
    consoleExpandUuids = !consoleExpandUuids;
    sync();
    refreshConsoleKeys();
  });
}

function renderTopicLi(
  topic: string,
  renderer: GraphRenderer,
  portalBase: string | null,
  onClick: (item: string) => void,
): HTMLLIElement {
  const li = document.createElement("li");
  li.className = "drilldown-list-item topic-list-item";

  const name = document.createElement("span");
  name.className = "topic-name";
  name.title = `click to drill into ${topic}`;
  tagConsoleKey(name, topic);
  name.addEventListener("click", () => onClick(topic));

  const meta = document.createElement("span");
  meta.className = "topic-meta";

  const fmt = renderer.formatForTopic(topic);
  const pill = document.createElement("span");
  pill.className = `topic-format topic-format-${fmt ?? "unknown"}`;
  pill.textContent = fmt ?? "—";
  pill.title = fmt
    ? `latest wire format: ${fmt}`
    : "no sample classified yet";
  meta.appendChild(pill);

  // Portal schema viewer (already-existing link; kept as the seed
  // pattern even when `siblings.portal` is unset). Falls back to the
  // legacy `portalBase` arg for the small window where the meta
  // endpoint hasn't surfaced siblings yet.
  const siblings = renderer.getSiblings();
  const schemaUrl =
    siblingUrl(siblings, "portal-schema", { key: topic }) ??
    (portalBase
      ? `${portalBase}/schema.html?key=${encodeURIComponent(topic)}`
      : null);
  appendSiblingIconLink(
    meta,
    schemaUrl,
    "topic-schema-link",
    "registry",
    "open schema viewer in portal",
  );
  // Stethoscope per-topic diagnose (Block 94). Compact `health` mark —
  // same icon family as the action row's larger stethoscope mark.
  appendSiblingIconLink(
    meta,
    siblingUrl(siblings, "stethoscope", { key: topic }),
    "topic-diagnose-link",
    "health",
    `diagnose this topic in stethoscope`,
  );
  // Flashlight per-topic live tail (Block 94).
  appendSiblingIconLink(
    meta,
    siblingUrl(siblings, "flashlight", { key: topic }),
    "topic-tail-link",
    "flashlight",
    `tail this topic in flashlight`,
  );

  li.appendChild(name);
  li.appendChild(meta);
  return li;
}

/** Topic list with per-row format pill + portal schema-page link.
 *  Used for publishes/consumes lists in the side panel. The pill shows
 *  the latest classified wire format (json / protobuf / cbor / text /
 *  hex / empty / —); the link icon jumps to the portal's schema viewer
 *  for that topic if a portal base URL has been configured.
 *
 *  Progressive disclosure: when a node has more than INLINE_TOPIC_CAP
 *  topics in this direction, the first 4 render inline and the rest
 *  collapse into a `<details>` overflow expander so the panel stays
 *  scannable. Mirrors the selection-driven progressive-disclosure
 *  principle: show a tractable default; let the operator expand on
 *  demand. */
const INLINE_TOPIC_CAP = 4;

function appendTopicList(
  parent: HTMLElement,
  label: string,
  items: string[],
  renderer: GraphRenderer,
  onClick: (item: string) => void,
): void {
  const dt = document.createElement("dt");
  dt.textContent = label;
  const dd = document.createElement("dd");
  const ul = document.createElement("ul");
  ul.className = "drilldown-list topic-list";
  const portalBase = renderer.getPortalBaseUrl();

  const inline = items.slice(0, INLINE_TOPIC_CAP);
  const overflow = items.slice(INLINE_TOPIC_CAP);

  for (const topic of inline) {
    ul.appendChild(renderTopicLi(topic, renderer, portalBase, onClick));
  }
  dd.appendChild(ul);

  if (overflow.length > 0) {
    const details = document.createElement("details");
    details.className = "topic-list-overflow";
    const summary = document.createElement("summary");
    summary.textContent = `show ${overflow.length} more`;
    summary.title = `expand to see all ${items.length} topics`;
    details.appendChild(summary);
    const moreUl = document.createElement("ul");
    moreUl.className = "drilldown-list topic-list";
    for (const topic of overflow) {
      moreUl.appendChild(renderTopicLi(topic, renderer, portalBase, onClick));
    }
    details.appendChild(moreUl);
    dd.appendChild(details);
  }

  parent.appendChild(dt);
  parent.appendChild(dd);
}

function appendList(parent: HTMLElement, label: string, items: string[]): void {
  const dt = document.createElement("dt");
  dt.textContent = label;
  const dd = document.createElement("dd");
  const ul = document.createElement("ul");
  for (const item of items) {
    const li = document.createElement("li");
    li.textContent = item;
    ul.appendChild(li);
  }
  dd.appendChild(ul);
  parent.appendChild(dt);
  parent.appendChild(dd);
}

/** Compact-number formatter for HUD counters. Raw integer up to 100K;
 *  100K-100M shown as `N.NM`; ≥100M as `N.NB`. The K tier is skipped
 *  on purpose — 99,999 → 100K crossover is the operator's first cue
 *  that the fabric has left small-fleet territory; once that lands we
 *  prefer reading "12.3M" over "12,345,678" in a 60-char-wide HUD. */
function formatCompactNumber(n: number): string {
  if (n < 100_000) return String(n);
  if (n < 100_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  return `${(n / 1_000_000_000).toFixed(1)}B`;
}

/** Wire the stream pause/resume button. Pause stops DOM updates so
 *  the operator can read a frozen frame; resume continues from the
 *  latest fetch. Space toggles when stream is focused. */
function installStreamPause(): void {
  const btn = document.getElementById("info-stream-pause");
  if (!btn) return;
  const apply = () => {
    btn.setAttribute("aria-pressed", String(streamPaused));
    btn.textContent = streamPaused ? "▶ RESUME" : "⏸ PAUSE";
  };
  btn.addEventListener("click", () => {
    streamPaused = !streamPaused;
    apply();
    // PAUSE/RESUME composes with LIVE-follow per lifecycle invariant.
    // Pause → unwatch (no point streaming notifies into a frozen view).
    // Resume → re-watch the current selection if LIVE is still armed,
    // then immediately refresh.
    if (streamPaused) {
      if (streamLive) unwatchAllAndClear();
    } else {
      if (streamLive) reconcileLiveWatch(streamCurrentTopics);
      void refreshStream();
    }
  });
  apply();
}

/** Wire the LEGEND popup — visual reading guide. Toggled by the
 *  fleet-bar LEGEND button + dismissable via × close or Escape. */
/** Dismiss an open overlay when the operator clicks the main COP canvas.
 *  Clicking the graph is the universal "back to the COP" gesture, so any
 *  open reference overlay (LEGEND / KEYS / diagnostics) steps aside.
 *  Capture-phase so the overlay closes before the graph's node-pick
 *  handler runs; the click still selects a node (or empty space), which
 *  is what the operator intended. Shared by every badge-opened popup so
 *  they dismiss consistently (the diagnostics overlay wires its own copy
 *  inline as it predates this helper). */
function dismissOverlayOnGraphClick(overlay: HTMLElement): void {
  document.getElementById("graph")?.addEventListener(
    "click",
    () => {
      if (!overlay.classList.contains("hidden")) overlay.classList.add("hidden");
    },
    { capture: true },
  );
}

function installLegendOverlay(): void {
  const overlay = document.getElementById("legend-overlay");
  const openBtn = document.getElementById("fleet-legend-toggle");
  const closeBtn = document.getElementById("legend-close");
  if (!overlay || !openBtn) return;
  // Opener TOGGLES (open ↔ close) so a second click on the LEGEND badge
  // dismisses it — matching the KEYS + diagnostics badges.
  openBtn.addEventListener("click", () => overlay.classList.toggle("hidden"));
  closeBtn?.addEventListener("click", () => overlay.classList.add("hidden"));
  dismissOverlayOnGraphClick(overlay);
  document.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape" && !overlay.classList.contains("hidden")) {
      overlay.classList.add("hidden");
      ev.stopPropagation();
    }
  });
}

/** Wire the KEYS popup — keyboard-shortcut reference. Replaces the
 *  cramped inline-hint strip that used to overflow the right half of
 *  the fleet bar. Toggled by:
 *    - the fleet-bar KEYS [ ? ] button (click toggles open ↔ close)
 *    - `?` anywhere (Shift+/ on US layouts — distinct from `/` which
 *      opens search; the Shift makes them unambiguous)
 *    - × close button, Escape, or a click on the COP canvas
 *
 *  `?` toggles (open ↔ close) — the operator's mental model is "one
 *  key, on and off," matching how Slack/GitHub/Gmail bind it. Skipped
 *  when an input/textarea is focused so it doesn't intercept literal
 *  `?` typed into the jq filter input or the search box.
 */
function installKeysOverlay(): void {
  const overlay = document.getElementById("keys-overlay");
  const openBtn = document.getElementById("fleet-keys-toggle");
  const closeBtn = document.getElementById("keys-close");
  if (!overlay || !openBtn) return;
  const toggle = () => overlay.classList.toggle("hidden");
  openBtn.addEventListener("click", toggle);
  closeBtn?.addEventListener("click", () => overlay.classList.add("hidden"));
  dismissOverlayOnGraphClick(overlay);
  document.addEventListener("keydown", (ev) => {
    // Escape: close when open.
    if (ev.key === "Escape" && !overlay.classList.contains("hidden")) {
      overlay.classList.add("hidden");
      ev.stopPropagation();
      return;
    }
    // `?` toggles, but only when no input is focused — otherwise we'd
    // swallow the operator typing `?` into the jq filter / search box.
    if (ev.key === "?") {
      const active = document.activeElement;
      if (active && (active.tagName === "INPUT" || active.tagName === "TEXTAREA")) return;
      toggle();
      ev.preventDefault();
    }
  });
}

/** Wire the fleet-stats sidebar's click-to-diagnostics behavior + the
 *  WS-based "fleet alive / quiet" enhancement on the connection
 *  indicator. The compact sidebar surfaces PEERS / TOPICS / MSG/s /
 *  TOTAL / VENDORS at a glance; operators have asked repeatedly "what
 *  exactly are these counting and how often do they update?" — the
 *  diagnostics overlay answers that without bloating the sidebar.
 *
 *  Two responsibilities in one install function because they share the
 *  1Hz tick + the `status` reference:
 *
 *  1. **Click on sidebar (or press `i`)** → open the diagnostics overlay.
 *     Esc / × closes. Same overlay vocabulary as LEGEND + KEYS so the
 *     three reference surfaces read as a set.
 *  2. **Fabric-alive sub-state on the connection-indicator.** When WS
 *     is `"live"` but no delta event landed in the last 10 seconds, the
 *     indicator transitions to a "quiet" state (visually dim, text
 *     reads `live · quiet`). Reflects the operator-meaningful question
 *     "is data actually flowing right now?" — distinct from the raw
 *     WS connection state. The HUD's existing STALE tag stays
 *     HTTP-polling-honest; this new sub-state is WS-event-driven, so
 *     the two together give the operator both readings without
 *     conflating them. (Task #13 of the polling-coalesce arc.)
 */
function installFleetDiagnostics(status: WelcomeStatus): void {
  const sidebar = document.getElementById("fleet-stats-sidebar");
  const overlay = document.getElementById("fleet-diagnostics-overlay");
  const closeBtn = document.getElementById("fleet-diagnostics-close");
  const indicator = document.getElementById("connection-indicator");
  if (!sidebar || !overlay) return;

  // Cached refs to the live-value spans — one DOM lookup at startup,
  // then we just rewrite textContent on every tick.
  const tierEl = document.getElementById("fleet-diag-tier");
  const wsEl = document.getElementById("fleet-diag-ws");
  const lastEventEl = document.getElementById("fleet-diag-last-event");
  const pollEl = document.getElementById("fleet-diag-poll");
  const thresholdEl = document.getElementById("fleet-diag-threshold");
  const imageEl = document.getElementById("fleet-diag-image");

  /** Time-since-last-event threshold above which a "live" WS connection
   *  reads as "quiet" on the indicator. Matches operator intuition:
   *  10s without a single delta event is long enough that the fabric
   *  has genuinely gone quiet (or panoscope's subscription isn't
   *  receiving), short enough that a healthy fabric never crosses it. */
  const QUIET_THRESHOLD_MS = 10_000;

  const refreshDiagnosticValues = () => {
    if (tierEl) {
      tierEl.textContent =
        `${currentTier.toUpperCase()} — ${POLICY[currentTier].badgeText}`;
    }
    if (wsEl) {
      const lastEvent = status.lastEventMs;
      const quiet =
        status.ws === "live" &&
        lastEvent != null &&
        Date.now() - lastEvent > QUIET_THRESHOLD_MS;
      wsEl.textContent = quiet ? `${status.ws} · quiet` : status.ws;
    }
    if (lastEventEl) {
      if (status.lastEventMs == null) {
        lastEventEl.textContent = "no events yet";
      } else {
        const ageS = Math.round((Date.now() - status.lastEventMs) / 1000);
        lastEventEl.textContent = `${ageS} sec ago`;
      }
    }
    if (pollEl) {
      pollEl.textContent = `${currentSnapshotPollMs()} ms (this tier)`;
    }
    if (thresholdEl) {
      const t = Math.round((4 * currentSnapshotPollMs()) / 100) / 10;
      thresholdEl.textContent = `${t} sec`;
    }
  };

  const refreshIndicatorState = () => {
    if (!indicator) return;
    const wsState = status.ws;
    const lastEvent = status.lastEventMs;
    let effectiveState: string = wsState;
    if (
      wsState === "live" &&
      lastEvent != null &&
      Date.now() - lastEvent > QUIET_THRESHOLD_MS
    ) {
      effectiveState = "quiet";
    }
    if (indicator.getAttribute("data-state") !== effectiveState) {
      indicator.setAttribute("data-state", effectiveState);
      indicator.textContent =
        effectiveState === "quiet"
          ? "live · quiet"
          : effectiveState === "live"
            ? "live"
            : effectiveState === "connecting"
              ? "connecting…"
              : effectiveState;
    }
  };

  const open = async () => {
    overlay.classList.remove("hidden");
    refreshDiagnosticValues();
    // Lazy-fetch the running version on first open; cache on the
    // element. Avoids hitting /version on every page load when the
    // operator never opens diagnostics.
    if (imageEl && (imageEl.textContent === "—" || imageEl.textContent === "")) {
      try {
        const resp = await fetch("/version", {
          cache: "no-store",
          signal: AbortSignal.timeout(2000),
        });
        if (resp.ok) {
          const v = (await resp.json()) as { name: string; version: string };
          imageEl.textContent = `${v.name} ${v.version}`;
        }
      } catch {
        if (imageEl) imageEl.textContent = "(version lookup unavailable)";
      }
    }
  };
  const close = () => overlay.classList.add("hidden");
  const toggle = () => {
    if (overlay.classList.contains("hidden")) void open();
    else close();
  };

  // Click anywhere on the sidebar opens the overlay (no nested
  // interactive elements today; the closest-button guard is defensive
  // for future additions).
  sidebar.addEventListener("click", (ev) => {
    if ((ev.target as HTMLElement).closest("button")) return;
    toggle();
  });
  closeBtn?.addEventListener("click", close);

  // Click on the main COP canvas dismisses the overlay — operators
  // treat clicking the graph as the "back to the COP" gesture, so any
  // open side overlay should step aside. Capture-phase so we close
  // before the graph's own node-pick handler runs; the click still
  // selects a node (or empty space), which is what the operator
  // intended.
  document.getElementById("graph")?.addEventListener(
    "click",
    () => {
      if (!overlay.classList.contains("hidden")) close();
    },
    { capture: true },
  );

  document.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape" && !overlay.classList.contains("hidden")) {
      close();
      ev.stopPropagation();
      return;
    }
    // `i` for "info / inspect" — matches the convention that `?` opens
    // KEYS and `/` opens search. Skipped when an input is focused.
    if (ev.key === "i") {
      const active = document.activeElement;
      if (active && (active.tagName === "INPUT" || active.tagName === "TEXTAREA")) return;
      toggle();
      ev.preventDefault();
    }
  });

  // Permanent 1 Hz tick. Always updates the connection-indicator's
  // effective state (so the "quiet" sub-state stays honest even when
  // the overlay is closed); also refreshes the live diagnostic values
  // when the overlay is open.
  window.setInterval(() => {
    refreshIndicatorState();
    if (!overlay.classList.contains("hidden")) refreshDiagnosticValues();
  }, 1000);
  refreshIndicatorState();
}

/** Show, next to the canvas, that the server regrouped or closed things to
 *  keep the view under its element cap — the canvas is then not the whole
 *  picture at normal detail, and the viewer must be told so. */
function renderTruncation(view: ViewResponse): void {
  let note = document.getElementById("view-truncated");
  const t = view.truncated;
  if (!t) {
    note?.classList.add("hidden");
    return;
  }
  if (!note) {
    note = document.createElement("div");
    note.id = "view-truncated";
    note.className = "view-truncated";
    note.setAttribute("role", "status");
    document.querySelector("main")?.appendChild(note);
  }
  const parts: string[] = [
    `Simplified to fit ${t.max_elements.toLocaleString()} elements: at most ${t.publisher_budget} per publisher.`,
  ];
  if (t.opened_ignored) parts.push("Opened groups are closed until fewer are open.");
  if (t.over_cap) parts.push("Still over the limit — narrow the window or filters.");
  note.textContent = parts.join(" ");
  note.classList.remove("hidden");
}

/** A two- or three-state toolbar button bound to one view parameter. Its
 *  state is read back from the parameters, so a reload, another control or
 *  the EXPAND ALL reset never leaves the label out of step. */
function bindCycleButton<T extends string>(opts: {
  id: string;
  dataAttr: string;
  read: (p: ViewParams) => T;
  write: (p: ViewParams, v: T) => ViewParams;
  order: readonly T[];
  label: (v: T) => string;
}): void {
  const btn = document.getElementById(opts.id);
  if (!btn) return;
  const sync = (p: ViewParams) => {
    const v = opts.read(p);
    btn.setAttribute(opts.dataAttr, v);
    btn.textContent = opts.label(v);
  };
  btn.addEventListener("click", () => {
    const cur = opts.read(viewClient.get());
    const next = opts.order[(opts.order.indexOf(cur) + 1) % opts.order.length];
    viewClient.update((p) => opts.write(p, next));
  });
  viewClient.onChange(sync);
  sync(viewClient.get());
}

/** SOURCE: ALL → REAL ONLY → NOTIONAL ONLY. "real" drops peers whose
 *  schema registration carried synthetic=true; "notional" keeps only those. */
function installSourceToggle(): void {
  bindCycleButton({
    id: "fleet-source-toggle",
    dataAttr: "data-source",
    read: (p) => p.source,
    write: (p, source) => ({ ...p, source }),
    order: ["all", "real", "notional"] as const,
    label: (m) => (m === "all" ? "ALL" : m === "real" ? "REAL ONLY" : "NOTIONAL ONLY"),
  });
}

/** INFRA: SHOW ALL → HIDE INFRA → ONLY INFRA. "hide" drops every node the
 *  backend classed `node_class: "infra"` (router / observer subscriber /
 *  schema-registry / operator-system / chaos / self-declared); "only"
 *  isolates the substrate. Composes with SOURCE. */
function installInfraToggle(): void {
  bindCycleButton({
    id: "fleet-infra-toggle",
    dataAttr: "data-infra",
    read: (p) => p.infra,
    write: (p, infra) => ({ ...p, infra }),
    order: ["all", "hide", "only"] as const,
    label: (m) => (m === "all" ? "SHOW ALL" : m === "hide" ? "HIDE INFRA" : "ONLY INFRA"),
  });
}

/** TEST: HIDDEN (default) ↔ ONLY. Test publishers are those whose every topic
 *  is loadtest data (the ADR 1103 interim provenance classifier). There is
 *  deliberately no "test alongside operational" state. */
function installTestToggle(): void {
  bindCycleButton({
    id: "fleet-test-toggle",
    dataAttr: "data-test",
    read: (p) => p.test,
    write: (p, test) => ({ ...p, test }),
    order: ["hide", "only"] as const,
    label: (m) => (m === "hide" ? "TEST HIDDEN" : "TEST ONLY"),
  });
}

/** History-lens window (docs/design/panoscope-history-lens.md): how far back
 *  the view shows peers and topics active within. */
function installHistoryWindow(): void {
  const sel = document.getElementById("fleet-window-select") as HTMLSelectElement | null;
  if (!sel) return;
  const sync = (p: ViewParams) => {
    sel.value = String(p.window_hours);
  };
  sel.addEventListener("change", () => {
    const h = Number(sel.value);
    if (!HISTORY_WINDOW_OPTIONS.some((o) => o.hours === h)) return;
    viewClient.update((p) => ({ ...p, window_hours: h }));
  });
  viewClient.onChange(sync);
  sync(viewClient.get());
}

/** SIMPLIFY: every publisher drawn as one unit. Opening a publisher's group
 *  still works while it is on. */
function installDetailToggle(): void {
  const btn = document.getElementById("fleet-aggressive-toggle") as HTMLButtonElement | null;
  if (!btn) return;
  const sync = (p: ViewParams) => {
    const on = p.detail === "simplified";
    btn.setAttribute("aria-pressed", on ? "true" : "false");
    btn.classList.toggle("active", on);
  };
  btn.addEventListener("click", () => {
    viewClient.update((p) => ({ ...p, detail: p.detail === "simplified" ? "normal" : "simplified" }));
  });
  viewClient.onChange(sync);
  sync(viewClient.get());
}

/** COLLAPSE ALL / EXPAND ALL. Collapse draws every organisation as one node;
 *  expand returns to the default grouping — no collapsed organisations, no
 *  folded peers, no opened groups. Both stay clickable in any state. */
function installVendorCollapseControls(renderer: GraphRenderer): void {
  const collapseBtn = document.getElementById("fleet-vendors-collapse-all");
  const expandBtn = document.getElementById("fleet-vendors-expand-all");
  collapseBtn?.addEventListener("click", () => renderer.collapseAllVendors());
  expandBtn?.addEventListener("click", () => renderer.expandAllVendors());
}

/** Wire the panel × close button + keyboard shortcuts:
 *    Escape  → clears selection (panel hides via onSelectionChange)
 *    ]       → toggle panel collapse/expand without affecting selection
 *              (lets you peek the full-canvas COP without losing focus)
 */
function installPanelClose(renderer: GraphRenderer): void {
  const btn = document.getElementById("info-close");
  btn?.addEventListener("click", () => renderer.clearSelection());

  // Persistent "panel hidden" mode — uses .collapsed class instead of
  // .hidden so the panel re-appears with the same selection content
  // when toggled back on.
  let collapsed = false;
  const togglePanel = () => {
    const panel = document.getElementById("info-panel");
    if (!panel) return;
    // If panel is fully hidden (no selection), opening collapse-mode
    // is a no-op — instead, focus a node first.
    if (panel.classList.contains("hidden") && !collapsed) return;
    collapsed = !collapsed;
    panel.classList.toggle("collapsed", collapsed);
  };

  document.addEventListener("keydown", (ev) => {
    const overlay = document.getElementById("search-overlay");
    const active = document.activeElement;
    const inInput = active && (active.tagName === "INPUT" || active.tagName === "TEXTAREA");
    if (ev.key === "Escape") {
      // Don't fight the search-overlay's own Escape handler.
      if (overlay && !overlay.classList.contains("hidden")) return;
      // Isolation is the outermost lens: ESC leaves it before it touches the
      // selection or the panel, so one key always means "back out one step".
      if (renderer.focusInfo()) {
        renderer.exitFocus();
        renderFocusBanner(renderer);
        return;
      }
      const panel = document.getElementById("info-panel");
      if (panel && !panel.classList.contains("hidden")) {
        // Drilled deep enough to have a back-trail? Step back one
        // level instead of dropping the whole panel. ESC then keeps
        // popping until the trail is empty and finally clears.
        if (inspectionHistory.length > 1) {
          popInspection();
        } else {
          renderer.clearSelection();
          collapsed = false;
          panel.classList.remove("collapsed");
        }
      }
    } else if ((ev.key === "f" || ev.key === "F") && !inInput) {
      // Isolate what the selection is connected to; pressing it again while
      // isolated re-anchors on whatever is selected now.
      if (renderer.focusInfo() && renderer.getSelection().length === 0) renderer.exitFocus();
      else if (!renderer.focusComponent()) showTransientNote("select something first, then press f to isolate what it is connected to");
      renderFocusBanner(renderer);
      ev.preventDefault();
    } else if (ev.key === "]" && !inInput) {
      togglePanel();
      ev.preventDefault();
    }
  });
}

/** Say something small and short-lived over the canvas. Used where a gesture
 *  cannot do what was asked and silence would read as a broken control. */
function showTransientNote(text: string): void {
  let note = document.getElementById("transient-note");
  if (!note) {
    note = document.createElement("div");
    note.id = "transient-note";
    note.className = "view-truncated";
    document.querySelector("main")?.appendChild(note);
  }
  note.textContent = text;
  note.classList.remove("hidden");
  window.setTimeout(() => note?.classList.add("hidden"), 3000);
}

/** The isolation banner: what is being shown, what is hidden, how to leave.
 *  Rendered after every view and every focus gesture. */
function renderFocusBanner(renderer: GraphRenderer): void {
  const info = renderer.focusInfo();
  let bar = document.getElementById("focus-banner");
  if (!info) {
    bar?.remove();
    return;
  }
  if (!bar) {
    bar = document.createElement("div");
    bar.id = "focus-banner";
    bar.className = "focus-banner";
    bar.setAttribute("role", "status");
    document.querySelector("main")?.appendChild(bar);
  }
  bar.innerHTML = "";
  const label = document.createElement("span");
  label.className = "focus-banner-label";
  label.textContent = `ISOLATED · ${info.label} · ${info.shown} shown, ${info.hidden} hidden`;
  bar.appendChild(label);
  const exit = document.createElement("button");
  exit.type = "button";
  exit.className = "focus-banner-exit";
  exit.textContent = "show everything (esc)";
  exit.addEventListener("click", () => {
    renderer.exitFocus();
    renderFocusBanner(renderer);
  });
  bar.appendChild(exit);
}

/** Stop the auto-refreshing stream timer (called when selection cleared). */
function stopStream(): void {
  if (streamRefreshHandle !== null) {
    window.clearInterval(streamRefreshHandle);
    streamRefreshHandle = null;
  }
  // Selection cleared → LIVE intent goes with the selection. Unwatch
  // whatever we'd been watching and drop the live flag. Per lifecycle
  // invariant in docs/design/panoscope.md §"LIVE-follow on the stream
  // view": the watch set tracks what the operator is actively viewing.
  unwatchAllAndClear();
  if (streamLive) {
    streamLive = false;
    applyStreamLiveButton();
  }
  streamCurrentTopics = [];
  const section = document.getElementById("info-stream");
  if (section) section.classList.add("hidden");
}

/** Send `{op:"watch",topic}` for every topic in `next` not already
 *  watched, and `{op:"unwatch",topic}` for every previously-watched
 *  topic that's no longer in `next`. No-op when streamLive=false:
 *  watch messages only fire while the operator has opted in.
 *  Lifecycle invariant: streamWatchedTopics ⊆ streamCurrentTopics
 *  whenever streamLive is true. */
function reconcileLiveWatch(next: string[]): void {
  if (!streamLive || !__wsClient) {
    return;
  }
  const wanted = new Set(next);
  for (const t of streamWatchedTopics) {
    if (!wanted.has(t)) {
      __wsClient.send({ op: "unwatch", topic: t });
    }
  }
  for (const t of wanted) {
    if (!streamWatchedTopics.has(t)) {
      __wsClient.send({ op: "watch", topic: t });
    }
  }
  streamWatchedTopics = wanted;
}

/** Send unwatch for every currently-watched topic and clear the
 *  local mirror. Used by stopStream / PAUSE / LIVE-off. */
function unwatchAllAndClear(): void {
  if (!__wsClient) {
    streamWatchedTopics.clear();
    return;
  }
  for (const t of streamWatchedTopics) {
    __wsClient.send({ op: "unwatch", topic: t });
  }
  streamWatchedTopics.clear();
}

function applyStreamLiveButton(): void {
  const btn = document.getElementById("info-stream-live");
  if (!btn) return;
  btn.setAttribute("aria-pressed", String(streamLive));
  btn.textContent = streamLive ? "■ LIVE" : "▶ LIVE";
}

/** Wire the ▶ LIVE button next to ⏸ PAUSE. Opt-in promotion of the
 *  currently-selected stream to notify-driven refresh. See
 *  docs/design/panoscope.md §"LIVE-follow on the stream view". */
function installStreamLive(): void {
  const btn = document.getElementById("info-stream-live");
  if (!btn) return;
  btn.addEventListener("click", () => {
    if (streamLive) {
      // Turn off — unwatch everything, fall back to tier-cadenced polling.
      unwatchAllAndClear();
      streamLive = false;
    } else {
      // Turn on — watch the current selection, if any. No-op when
      // no topic is selected (button stays armed; next selection
      // through startStream's reconcile will pick up the watch).
      streamLive = true;
      reconcileLiveWatch(streamCurrentTopics);
    }
    applyStreamLiveButton();
  });
  applyStreamLiveButton();
}

/** Adaptive performance monitor — observes frame-render times and
 *  classifies the current operating tier (LIVE/SLOW/DEGRADED/AUSTERE).
 *  Updates the fleet-bar tier badge + tells the renderer which
 *  animations to gate. The fleet bar's polling timer reads
 *  currentPollMs() so its cadence adapts to the tier too. */
let currentTier: PerfTier = "live";
let __watchfulRenderer: GraphRenderer | null = null;
function installPerformanceMonitor(renderer: GraphRenderer): void {
  __watchfulRenderer = renderer;
  const monitor = new PerformanceMonitor();
  monitor.start();
  const badge = document.getElementById("fleet-tier-badge");
  const apply = (tier: PerfTier) => {
    currentTier = tier;
    const policy = POLICY[tier];
    if (badge) {
      badge.textContent = policy.badgeText;
      badge.setAttribute("data-tier", tier);
    }
    // Tell the renderer (which has its own internal state) what's
    // permitted. Renderer methods are tier-aware no-ops where allowed.
    renderer.setPacketDotsAllowed?.(policy.packetDots);
    renderer.setForceSimEnabled?.(policy.forceSim);
    // AUSTERE viz strip — companion to the policy's compute gates. The
    // SVG gets `austere-mode` so CSS hides edges + dot layer + hull
    // fills, leaving the topology skeleton + the selected slice. See
    // styles.css "AUSTERE viz strip" and design doc §self-aware-degradation.
    renderer.setAustereMode?.(tier === "austere");
    // Tier change can also flip the watchful condition (browser-tier
    // gate is an earlier short-circuit inside isWatchfulWithoutSelection).
    refreshWatchfulBadge();
  };
  monitor.onChange((s) => apply(s.tier));
  apply(monitor.current().tier);
}

/** Self-aware degradation: surface the watchful-tier explanatory badge
 *  in the top bar when (a) the browser tier allows packet-dots in
 *  principle, (b) the renderer's fleet-rate gate is currently
 *  withholding them because aggregate rate is 50-200 msg/s, and
 *  (c) no edge is selected to whitelist a slice. Without this badge
 *  the operator stares at an apparently-frozen viz and can't tell
 *  fabric-quiet from too-busy-to-draw. */
function refreshWatchfulBadge(): void {
  if (!__watchfulRenderer) return;
  const watchful = document.getElementById("fleet-watchful-section");
  const watchfulOn = __watchfulRenderer.isWatchfulWithoutSelection?.() ?? false;
  watchful?.classList.toggle("visible", watchfulOn);

  // Saturated (>200 msg/s): dots off entirely, nothing to select. Distinct
  // badge so a genuinely busy fabric never reads as a frozen/broken viz.
  const saturated = document.getElementById("fleet-saturated-section");
  const saturatedOn = __watchfulRenderer.isRateSaturated?.() ?? false;
  saturated?.classList.toggle("visible", saturatedOn);
  if (saturatedOn) {
    const badge = document.getElementById("fleet-saturated-badge");
    const rate = Math.round(__watchfulRenderer.currentFleetMsgPerSec?.() ?? 0);
    if (badge) badge.textContent = `BUSY · DOTS OFF (${rate}/s)`;
  }
}
function currentSnapshotPollMs(): number {
  return POLICY[currentTier].snapshotPollMs;
}
function currentStreamPollMs(): number {
  return POLICY[currentTier].streamPollMs;
}

/** Wire the always-on fleet stats bar — peers / topics / msg/s /
 *  total / vendors. Polls /api/stats + /api/view at the tier's
 *  snapshotPollMs cadence.
 *
 *  Self-aware staleness: the bar tracks `lastSuccessAt` and
 *  `consecutiveFailures`. If two refreshes in a row fail, OR the last
 *  success is older than 2× the current poll interval, the sidebar
 *  gains `.stale` (values desaturate) and the freshness tag flips to
 *  STALE with the age in seconds. On every successful refresh the tag
 *  reads "live · Ns ago" so the operator can always answer "are these
 *  numbers true right now?" without guessing. Replaces the prior
 *  silent try/catch that hid every fetch failure. */
function installFleetBar(): void {
  const sidebar = document.getElementById("fleet-stats-sidebar");
  const peers = document.getElementById("fleet-peers");
  const topics = document.getElementById("fleet-topics");
  const rate = document.getElementById("fleet-rate");
  const total = document.getElementById("fleet-total");
  const vendors = document.getElementById("fleet-vendors");
  const freshness = document.getElementById("fleet-stat-freshness");
  if (!sidebar || !peers || !topics || !rate || !total || !vendors || !freshness) return;
  // Nothing has answered yet, so the fabric's size is unknown. A zero here
  // reads as "the fabric is empty", which is a claim this page cannot make
  // before its first response (the same reason the welcome card waits).
  for (const el of [peers, topics, rate, total, vendors]) el.textContent = "—";

  let lastSuccessAt = 0;
  let consecutiveFailures = 0;
  // EMA of successful round-trip fetch durations (stats+snapshot pair).
  // Seeds the latency-aware staleness floor below. 0 until the first
  // success lands; the first sample initializes it outright (no decay
  // toward a meaningless 0).
  let emaFetchMs = 0;

  const updateFreshness = () => {
    // Two independent triggers — fail-count is the fast signal,
    // age-vs-poll-interval is the slow one for hangs that don't throw
    // (e.g. fetch resolves but with garbage). Either trips `.stale`.
    //
    // The age threshold is LATENCY-AWARE, not a flat multiple of the
    // poll interval. The flat `4× poll` gate (6 s at LIVE) assumed the
    // fetch itself is ~free, which holds on a fast link but breaks on a
    // relay-routed / high-RTT mesh path: there the effective refresh
    // cycle is `poll + fetchDuration`, so steady-state age already sits
    // at ~poll+fetch. When fetch ≈ 2 s the flat 6 s gate leaves under
    // one fetch-duration of headroom, and a SINGLE slow or timed-out
    // cycle blows past it — STALE flickers (or sticks for the 8 s
    // timeout) even though the feed is live and WS dots keep animating.
    // That false-positive reads as "panoscope goes stale all the time"
    // on a slow path. The fix: widen the age floor to also clear a
    // few observed fetch-durations of slack, so STALE means "the feed
    // stopped," not "the network to this observer is slow." On a fast
    // link emaFetchMs≈0 and this collapses back to the original 4× poll.
    // `consecutiveFailures >= 2` stays the hard real-outage trigger,
    // untouched by link latency (two back-to-back failed fetches trip
    // it instantly regardless of the age floor).
    const pollMs = currentSnapshotPollMs();
    const ageMs = lastSuccessAt === 0 ? Number.POSITIVE_INFINITY : Date.now() - lastSuccessAt;
    const stale = isStale({ consecutiveFailures, ageMs, pollMs, emaFetchMs });
    sidebar.classList.toggle("stale", stale);
    if (lastSuccessAt === 0) {
      freshness.textContent = consecutiveFailures > 0 ? "STALE · never" : "connecting…";
    } else {
      const ageS = Math.round(ageMs / 1000);
      // Use a clear unit word ("sec") with a space so the operator
      // can't misread the "s" as a 5 next to the digit count
      // (operator-reported confusion).
      freshness.textContent = stale
        ? `STALE · ${ageS} sec ago`
        : `live · ${ageS} sec ago`;
    }
  };

  const refreshOnce = async () => {
    // A hidden tab has no reader and a signed-out page cannot be served
    //. The timer keeps ticking; this returns immediately, so the bar
    // resumes by itself when the viewer comes back — and the freshness clock
    // keeps ageing, which is honest: the view IS old.
    if (!shouldPoll(document.hidden, isPageSignedOut())) return;
    // Stamp the round-trip start so a successful cycle can fold its
    // observed duration into emaFetchMs (drives the latency-aware
    // staleness floor in updateFreshness). Measured start→parse on the
    // success path only; failed/timed-out cycles don't pollute the EMA.
    const fetchStart = Date.now();
    try {
      // Per-fetch hard timeout via AbortSignal.timeout — see
      // FLEET_FETCH_TIMEOUT_MS comment at top of file. Without this,
      // a hung backend keeps the connection slot occupied forever and
      // saturates the per-origin pool. Independent signals so a slow
      // /api/stats doesn't pin a fast /api/view or vice versa.
      const [statsResp, outcome] = await Promise.all([
        fetch("/api/stats", {
          cache: "no-store",
          signal: AbortSignal.timeout(FLEET_FETCH_TIMEOUT_MS),
        }),
        viewClient.fetchView(),
      ]);
      // A 401 on either half is the session, not the fleet: stop rather than
      // counting it as a failure and retrying at the same cadence forever.
      if (handleResponse(statsResp)) return;
      if (outcome.kind === "http" && handleResponse({ status: outcome.status })) return;
      if (!statsResp.ok || outcome.kind === "http") {
        const viewStatus = outcome.kind === "http" ? outcome.status : 200;
        throw new Error(`fleet-bar fetch non-2xx: stats=${statsResp.status} view=${viewStatus}`);
      }
      const stats = await statsResp.json();
      const rawTotal = stats.data_samples_recorded ?? 0;
      total.textContent = formatCompactNumber(rawTotal);
      // Full-precision count on hover so the operator can still read
      // the exact figure when compact form has compressed it.
      total.title = `${rawTotal.toLocaleString()} samples`;
      // "superseded": parameters changed mid-flight and a fetch for the new
      // ones is already queued. "unchanged" (304): the view on screen is
      // current. Both count as a live feed.
      if (outcome.kind === "view") {
        const { view, params } = outcome;
        publishView(view, params);
        // The HUD counts what the viewer can see, not what is drawn: a
        // group of 1,204 topics adds 1,204. msg/s is the publish rate —
        // one packet dot animates per publish sample, so it is the load
        // the packet-dot gate protects.
        const t = view.totals;
        rate.textContent = t.msg_per_sec.toFixed(2);
        const r = (window as unknown as { __panoscopeRenderer?: GraphRenderer }).__panoscopeRenderer;
        r?.setFleetMsgPerSec(t.msg_per_sec);
        peers.textContent = String(t.peers);
        topics.textContent = String(t.topics);
        vendors.textContent = String(t.orgs);
      }
      const now = Date.now();
      // Fold this cycle's round-trip into the fetch-latency EMA (α=0.3).
      // First sample initializes outright so the floor reflects the real
      // link immediately rather than decaying up from 0. This is what
      // makes the staleness floor track a slow/relay-routed path.
      emaFetchMs = foldFetchEma(emaFetchMs, now - fetchStart);
      lastSuccessAt = now;
      consecutiveFailures = 0;
      // Watchful condition is downstream of fleet rate; re-evaluate
      // after every successful refresh (selection-driven re-eval is
      // separate, in onSelectionChange).
      refreshWatchfulBadge();
    } catch (err) {
      consecutiveFailures += 1;
      // Not silent: counters freezing without a signal was the gap that
      // motivated this rewrite. Two failures in a row trip the staleness
      // surface; before that the operator already sees the freshness
      // counter growing.
      console.warn("fleet-bar refresh failed", err);
    }
    updateFreshness();
  };
  // One fetch at a time. A request made while one is in flight (a parameter
  // changed) runs once more when it lands, so the latest parameters always
  // get a fetch and requests never stack.
  let inFlight: Promise<void> | null = null;
  let again = false;
  const refresh = async (): Promise<void> => {
    if (inFlight) {
      again = true;
      return inFlight;
    }
    inFlight = (async () => {
      do {
        again = false;
        await refreshOnce();
      } while (again);
    })();
    try {
      await inFlight;
    } finally {
      inFlight = null;
    }
  };
  requestViewNow = () => void refresh();
  // Test hook: the e2e specs change the mocked backend, then ask for a fetch
  // instead of waiting out the poll.
  (window as unknown as { __panoscopeRefreshView: () => Promise<void> }).__panoscopeRefreshView = refresh;
  void refresh();
  // Continuously update the freshness clock between refreshes so the
  // age ticks visibly — without this, a stuck fetch would freeze both
  // the values and the freshness tag and look the same as success.
  window.setInterval(updateFreshness, 1000);
  // Self-rescheduling interval — re-reads currentSnapshotPollMs() each
  // tick so a tier transition (LIVE→SLOW etc) immediately slows the
  // polling cadence without needing to clearInterval + setInterval.
  const reschedule = () => {
    window.setTimeout(async () => {
      await refresh();
      reschedule();
    }, currentSnapshotPollMs());
  };
  reschedule();
}

/** Wire the `/`-triggered search overlay. Matches drawn nodes (peers by alias,
 *  name, vendor or id; topic pins by key) immediately, then topics inside
 *  groups from /api/view/search. ↑/↓ to navigate, Enter to commit (replace
 *  selection), Cmd+Enter to append. */
function installSearchOverlay(renderer: GraphRenderer): void {
  const overlay = document.getElementById("search-overlay");
  const input = document.getElementById("search-input") as HTMLInputElement | null;
  const matchesEl = document.getElementById("search-matches");
  if (!overlay || !input || !matchesEl) return;

  // A match is either a drawn node (commit = select it) or a topic inside a
  // group (commit = open the groups holding it, then select its pin). Grouped
  // topics come from the server, which searches every topic the viewer can
  // see, so any substring of any key is findable whatever the grouping.
  interface Match {
    id: string;
    label: string;
    reveal?: { key: string; publisher: string; group: string };
  }
  const LIMIT = 30;
  const SERVER_DEBOUNCE_MS = 150;
  let activeMatches: Match[] = [];
  let activeIdx = 0;
  let query = 0;
  let serverTimer: number | null = null;

  const close = () => {
    overlay.classList.add("hidden");
    input.value = "";
    matchesEl.innerHTML = "";
    activeMatches = [];
    query++;
  };
  const open = (prefill = "") => {
    overlay.classList.remove("hidden");
    input.value = prefill;
    input.focus();
    input.select();
    runQuery(prefill);
  };
  // Expose open(prefill) to the group inspector's escape-hatch.
  openFleetSearch = (prefill: string) => open(prefill);

  const paint = () => {
    matchesEl.innerHTML = "";
    activeMatches.forEach((m, i) => {
      const div = document.createElement("div");
      div.className = "search-overlay-match" + (i === activeIdx ? " active" : "");
      div.textContent = m.label;
      div.addEventListener("click", (ev) => {
        commitMatch(m, ev.metaKey || ev.ctrlKey);
      });
      matchesEl.appendChild(div);
    });
  };

  const runQuery = (q: string) => {
    const mine = ++query;
    const needle = q.trim().toLowerCase();
    const items: Match[] = [];
    for (const n of renderer.allNodeIds()) {
      if (n.is_singleton_super) continue; // groups are searched by member below
      const isTopic = n.id.startsWith("topic:");
      // Display label mirrors the canvas label preference (ADR 1039):
      // slot_alias > display_name > vendor > short(id) for peers, the
      // stripped key for topics.
      const label = isTopic
        ? n.id.slice("topic:".length)
        : (n.slot_alias ?? n.display_name ?? n.vendor ?? shortId(n.id));
      // Match across every human-facing handle AND the canonical id, so a
      // substring of the alias / display_name / vendor / UUID hits anywhere.
      const haystack = [label, n.slot_alias, n.display_name, n.vendor, n.id]
        .filter((s): s is string => typeof s === "string" && s.length > 0)
        .join(" ")
        .toLowerCase();
      if (needle === "" || haystack.includes(needle)) {
        items.push({ id: n.id, label });
      }
    }
    activeMatches = items.slice(0, LIMIT);
    activeIdx = 0;
    paint();
    if (serverTimer !== null) window.clearTimeout(serverTimer);
    if (needle === "") return;
    serverTimer = window.setTimeout(() => {
      serverTimer = null;
      void viewClient
        .search(q.trim(), LIMIT)
        .then((res) => {
          if (!res || mine !== query) return;
          const view = renderer.currentView();
          const labelOf = new Map((view?.groups ?? []).map((g) => [g.id, g.label]));
          const grouped: Match[] = [];
          for (const h of res.hits) {
            const pin = `topic:${h.key}`;
            if (h.drawn_as === pin) continue; // already matched as a drawn node
            grouped.push({
              id: pin,
              label: `${h.key}  · in ${labelOf.get(h.drawn_as) ?? "a group"}`,
              reveal: { key: h.key, publisher: h.publisher, group: h.drawn_as },
            });
          }
          const activeId = activeMatches[activeIdx]?.id;
          activeMatches = activeMatches.concat(grouped).slice(0, LIMIT);
          activeIdx = Math.max(0, activeMatches.findIndex((m) => m.id === activeId));
          paint();
        })
        .catch(() => {
          /* transient — the drawn-node matches stand */
        });
    }, SERVER_DEBOUNCE_MS) as unknown as number;
  };

  // Update canvas selection AND pivot the side panel for the matched node,
  // so the stream pane follows the search rather than staying on the prior
  // selection.
  const commitMatch = (m: Match, append: boolean): void => {
    if (m.reveal) {
      // A grouped topic: show its topic panel now; the pin is selected when
      // the view that draws it arrives. When the server cannot open it (its
      // group is too large to draw), select the group and list it there.
      const { key, publisher, group } = m.reveal;
      const topicNode = {
        id: m.id,
        display_name: null,
        vendor: null,
        role: "topic",
        publishes: [],
        subscribes: [],
        first_seen_ms: 0,
        last_seen_ms: 0,
      };
      renderNodeInfo(topicNode);
      void renderer.revealTopic(key, publisher).then((shown) => {
        if (shown) return;
        const g = renderer.allNodeIds().find((nn) => nn.id === group);
        if (!g) return;
        renderer.setSelection([group]);
        const panel = document.getElementById("info-panel");
        const title = document.getElementById("info-title");
        const body = document.getElementById("info-body");
        if (panel && title && body) renderGroupInspector(g, panel, title, body, key);
      });
    } else {
      renderer.setSelection([m.id], { append });
      const node = renderer.allNodeIds().find((n) => n.id === m.id);
      if (node) renderNodeInfo(node);
    }
    close();
  };

  input.addEventListener("input", () => runQuery(input.value));
  input.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") {
      close();
      ev.preventDefault();
    } else if (ev.key === "Enter") {
      // Commit ONLY the active match (the keyboard-highlighted row),
      // not all 30. Pre-selecting every match in one go composed badly
      // with the old canvas-click rhythm, where a follow-up click on a
      // selected member collapsed its vendor. The unified
      // model retired that gesture (single click never folds), so the
      // hazard is gone — but active-match-only is still the right commit
      // behavior. Cmd / ctrl appends to the existing
      // selection so the operator can compose multi-selects across
      // several search queries.
      if (activeMatches.length > 0) {
        const m = activeMatches[activeIdx] ?? activeMatches[0];
        commitMatch(m, ev.metaKey || ev.ctrlKey);
      } else {
        close();
      }
      ev.preventDefault();
    } else if (ev.key === "ArrowDown") {
      activeIdx = Math.min(activeIdx + 1, activeMatches.length - 1);
      updateActive();
      ev.preventDefault();
    } else if (ev.key === "ArrowUp") {
      activeIdx = Math.max(activeIdx - 1, 0);
      updateActive();
      ev.preventDefault();
    }
  });

  const updateActive = () => {
    const children = matchesEl.querySelectorAll(".search-overlay-match");
    children.forEach((el, i) => el.classList.toggle("active", i === activeIdx));
    const active = children[activeIdx] as HTMLElement | undefined;
    active?.scrollIntoView({ block: "nearest" });
  };

  // Global keyboard: `/` opens search (unless an input is already
  // focused). Escape closes whether or not overlay was the target.
  document.addEventListener("keydown", (ev) => {
    if (ev.key === "/") {
      const active = document.activeElement;
      if (active && (active.tagName === "INPUT" || active.tagName === "TEXTAREA")) return;
      open();
      ev.preventDefault();
    } else if (ev.key === "Escape" && !overlay.classList.contains("hidden")) {
      close();
      ev.preventDefault();
    }
  });
}

document.addEventListener("DOMContentLoaded", () => {
  try {
    main();
  } catch (e) {
    console.error("panoscope bootstrap failed", e);
  }
});
