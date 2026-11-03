// SPDX-License-Identifier: Apache-2.0
//
// A mocked panoscope backend for the e2e specs.
//
// The browser draws only what `/api/view` returns; grouping, filtering and
// organisation collapse are the server's (docs/design/panoscope-view-model.md)
// and are tested in Rust (`ops/panoscope/src/view/`). This mock implements
// just enough of the view contract for a spec to exercise the browser side
// against a small fixture: the parameters the page sends, the elements it
// draws, and the gestures that edit the parameters.
//
// What it emulates: `test` / `infra` / `source` filters, `collapse_orgs`,
// `fold_peers` (a folded peer's topics become one publisher group), and
// explicit fixture groups that stay closed unless their id is in `expand`.
// What it does not: time windows, the per-publisher budget, the element cap.
// A spec that needs those responses supplies them with `overrideView`.

import type { Page, Route } from "@playwright/test";
import type {
  GroupOut,
  RevealResult,
  SearchResult,
  ViewEdge,
  ViewParams,
  ViewResponse,
} from "../../src/types";

export interface FxPeer {
  id: string;
  vendor?: string | null;
  role?: "source" | "sink" | "deriver" | "unknown";
  publishes?: string[];
  subscribes?: string[];
  /** Load-test publisher: hidden unless `test: "only"`. */
  test?: boolean;
  infra?: boolean;
  synthetic?: boolean;
  format?: string;
}

export interface FxGroup {
  id: string;
  kind: GroupOut["kind"];
  publisher: string;
  label: string;
  selector: string;
  /** Member topic keys; each must be one of the publisher's `publishes`. */
  members: string[];
}

export interface BackendOptions {
  peers: FxPeer[];
  groups?: FxGroup[];
  legend?: Record<string, unknown>;
  /** `/api/samples?…` body; default no entries. */
  samples?: (url: URL) => unknown;
  /** `/api/samples/decoded?…` body; default portal disabled. */
  decoded?: (url: URL) => unknown;
}

export interface Backend {
  /** Every parameter set the page asked `/api/view` for, in order. */
  viewRequests: ViewParams[];
  /** Replace the answer to `/api/view` (return a status code to fail it). */
  overrideView(fn: ((params: ViewParams, built: ViewResponse) => unknown) | null): void;
  /** The last parameters the page sent. */
  lastParams(): ViewParams | undefined;
}

const NOW = () => Date.now();

function json(route: Route, body: unknown, status = 200): Promise<void> {
  return route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
}

function selectorMatches(selector: string, key: string): boolean {
  const s = selector.split("/");
  const k = key.split("/");
  let i = 0;
  for (; i < s.length; i++) {
    if (s[i] === "**") return true;
    if (i >= k.length) return false;
    if (s[i] !== "*" && s[i] !== k[i]) return false;
  }
  return i === k.length;
}

/** The grouping of one view: which topic keys each group holds. */
function groupsFor(opts: BackendOptions, params: ViewParams, visible: FxPeer[]): FxGroup[] {
  const out: FxGroup[] = [];
  for (const p of visible) {
    const pubs = p.publishes ?? [];
    if (pubs.length === 0) continue;
    if (params.fold_peers.includes(p.id) || params.detail === "simplified") {
      out.push({ id: `grp:${p.id}:pub`, kind: "publisher", publisher: p.id, label: "**", selector: "**", members: pubs });
      continue;
    }
    for (const g of opts.groups ?? []) {
      if (g.publisher === p.id && !params.expand.includes(g.id)) out.push(g);
    }
  }
  return out;
}

export function buildView(opts: BackendOptions, params: ViewParams, generation: number): ViewResponse {
  const now = NOW();
  const collapses = (v: string | null | undefined) =>
    !!v && (params.collapse_orgs.includes("*") || params.collapse_orgs.includes(v));
  const visible = opts.peers.filter((p) => {
    if (params.test === "only" ? !p.test : p.test) return false;
    if (params.infra === "hide" && p.infra) return false;
    if (params.infra === "only" && !p.infra) return false;
    if (params.source === "real" && p.synthetic) return false;
    if (params.source === "notional" && !p.synthetic) return false;
    return true;
  });
  const drawnPeers = visible.filter((p) => !collapses(p.vendor));
  const groups = groupsFor(opts, params, drawnPeers);
  const grouped = new Set(groups.flatMap((g) => g.members.map((k) => `${g.publisher}|${k}`)));

  const orgs = new Map<string, { peers: number; topics: number }>();
  for (const p of visible) {
    if (!collapses(p.vendor)) continue;
    const o = orgs.get(p.vendor!) ?? { peers: 0, topics: 0 };
    o.peers += 1;
    o.topics += (p.publishes ?? []).length;
    orgs.set(p.vendor!, o);
  }

  const pinPublishers = new Map<string, string[]>();
  for (const p of drawnPeers) {
    for (const k of p.publishes ?? []) {
      if (grouped.has(`${p.id}|${k}`)) continue;
      pinPublishers.set(k, [...(pinPublishers.get(k) ?? []), p.id]);
    }
  }
  const edges: ViewEdge[] = [];
  const edge = (from: string, to: string, kind: ViewEdge["kind"], topics: number): void => {
    edges.push({ from, to, kind, projected_eps: topics, total_events: 100 * topics, last_event_ms: now, topic_count: topics });
  };
  for (const [key, pubs] of pinPublishers) {
    for (const pub of pubs) edge(pub, `topic:${key}`, "publish", 1);
    for (const s of drawnPeers) {
      if ((s.subscribes ?? []).includes(key)) edge(`topic:${key}`, s.id, "subscribe", 1);
    }
  }
  for (const g of groups) edge(g.publisher, g.id, "publish", g.members.length);

  const allTopics = new Set(visible.flatMap((p) => p.publishes ?? []));
  const view: ViewResponse = {
    generation,
    built_at_ms: now,
    etag: `"mock-${generation}"`,
    peers: drawnPeers.map((p) => ({
      peer_id: p.id,
      slot_alias: null,
      display_name: p.id,
      vendor: p.vendor ?? null,
      role: p.role ?? "source",
      publishes: p.publishes ?? [],
      subscribes: p.subscribes ?? [],
      first_seen_ms: now - 60_000,
      last_seen_ms: now,
      site: null,
      role_class: null,
      format_hint: p.format ?? "json",
      synthetic: !!p.synthetic,
      node_kind: "peer",
      attest_source: "schema_decl",
      locator: null,
      node_class: p.infra ? "infra" : "app",
      node_subkind: null,
      universal_observer: false,
      hygiene: null,
      visible_topics: (p.publishes ?? []).length,
      folded: params.fold_peers.includes(p.id),
    })),
    topics: [...pinPublishers].map(([key, pubs]) => ({
      id: `topic:${key}`,
      key,
      publishers: pubs,
      projected_eps: 1,
      total_events: 100,
      last_event_ms: now,
      format: "json",
      schema_family: "demo",
      subscriber_count: drawnPeers.filter((s) => (s.subscribes ?? []).includes(key)).length,
    })),
    groups: groups.map((g) => ({
      id: g.id,
      kind: g.kind,
      publisher: g.publisher,
      label: g.label,
      selector: g.selector,
      topic_count: g.members.length,
      projected_eps: g.members.length,
      total_events: 100 * g.members.length,
      last_event_ms: now,
      formats: { json: g.members.length },
      subscriber_count: 0,
    })),
    orgs: [...orgs].map(([org, o]) => ({ id: `org:${org}`, org, peer_count: o.peers, topic_count: o.topics, projected_eps: 0 })),
    edges,
    totals: {
      peers: visible.length,
      topics: allTopics.size,
      groups: groups.length,
      // A modest fleet rate on purpose: above 200 msg/s the renderer disables
      // packet dots by design, which a fixture should not trip by accident.
      msg_per_sec: 4.2,
      total_events: 100 * allTopics.size,
      orgs: new Set(visible.map((p) => p.vendor).filter(Boolean)).size,
      elements: 0,
    },
    truncated: null,
    limits: { max_elements: 5_000, publisher_budget: 40, max_open_units: 2_000 },
    sampling_drops: {},
  };
  view.totals.elements = view.peers.length + view.topics.length + view.groups.length + view.orgs.length + edges.length;
  return view;
}

/** Install the backend mocks. Call before `page.goto`. */
export async function installBackend(page: Page, opts: BackendOptions): Promise<Backend> {
  const viewRequests: ViewParams[] = [];
  let override: ((params: ViewParams, built: ViewResponse) => unknown) | null = null;
  let generation = 0;
  const bodyParams = (route: Route): ViewParams => route.request().postDataJSON() as ViewParams;

  await page.route("**/api/legend", (r) =>
    json(r, {
      schema_resolver_status: "family-map",
      schema_resolver_known_families: 0,
      sampling_drops: {},
      ema_tau_secs: 30,
      ws_coalesce_ms: 250,
      generated_at_ms: NOW(),
      portal_base_url: "",
      siblings: {},
      decoder_enabled: false,
      ...(opts.legend ?? {}),
    }),
  );
  await page.route("**/api/stats", (r) => json(r, { data_samples_recorded: 1234 }));
  await page.route(/\/api\/samples\?/, (r) =>
    json(r, opts.samples ? opts.samples(new URL(r.request().url())) : { entries: [] }),
  );
  await page.route(/\/api\/samples\/decoded\?/, (r) => {
    const url = new URL(r.request().url());
    return json(r, opts.decoded ? opts.decoded(url) : { topic: url.searchParams.get("topic") ?? "", status: "portal_disabled", samples: [] });
  });

  await page.route("**/api/view", (r) => {
    const params = bodyParams(r);
    viewRequests.push(params);
    const built = buildView(opts, params, ++generation);
    if (!override) return json(r, built);
    const answer = override(params, built);
    return typeof answer === "number" ? json(r, { error: "mock" }, answer) : json(r, answer);
  });

  await page.route("**/api/view/group", (r) => {
    const q = r.request().postDataJSON() as { params: ViewParams; id: string; q: string; offset: number; limit: number };
    const visible = opts.peers.filter((p) => (q.params.test === "only" ? p.test : !p.test));
    const g = groupsFor(opts, q.params, visible).find((x) => x.id === q.id);
    if (!g) return json(r, { error: "not in view" }, 404);
    const needle = (q.q ?? "").toLowerCase();
    const matched = [...g.members].sort().filter((k) => !needle || k.toLowerCase().includes(needle));
    return json(r, {
      id: g.id,
      generation,
      total: g.members.length,
      matched: matched.length,
      rows: matched.slice(q.offset, q.offset + q.limit).map((key, i) => ({
        key,
        projected_eps: 1,
        total_events: 100 + i,
        last_event_ms: NOW(),
        format: "json",
        schema_family: "demo",
        subscriber_count: 0,
      })),
    });
  });

  await page.route("**/api/view/search", (r) => {
    const q = r.request().postDataJSON() as { params: ViewParams; q: string; limit: number };
    const view = buildView(opts, q.params, generation);
    const groups = groupsFor(opts, q.params, opts.peers);
    const needle = q.q.toLowerCase();
    const hits: SearchResult["hits"] = [];
    for (const t of view.topics) {
      if (t.key.toLowerCase().includes(needle)) hits.push({ key: t.key, publisher: t.publishers[0], drawn_as: t.id });
    }
    for (const g of groups) {
      if (!view.groups.some((vg) => vg.id === g.id)) continue;
      for (const k of g.members) {
        if (k.toLowerCase().includes(needle)) hits.push({ key: k, publisher: g.publisher, drawn_as: g.id });
      }
    }
    const body: SearchResult = { generation, total: hits.length, hits: hits.slice(0, q.limit) };
    return json(r, body);
  });

  await page.route("**/api/view/reveal", (r) => {
    const q = r.request().postDataJSON() as { params: ViewParams; key: string; publisher: string };
    const holder = groupsFor(opts, q.params, opts.peers).find(
      (g) => g.publisher === q.publisher && g.members.includes(q.key) && selectorMatches(g.selector, q.key),
    );
    const body: RevealResult = holder
      ? { expand: [holder.id], topic_id: `topic:${q.key}` }
      : { expand: [], topic_id: opts.peers.some((p) => (p.publishes ?? []).includes(q.key)) ? `topic:${q.key}` : null };
    return json(r, body);
  });

  // WebSocket: opens and stays quiet; `window.__mockWs.push(obj)` delivers a
  // frame (publish_sampled deltas, for example).
  await page.addInitScript(() => {
    type Listener = (ev: Event) => void;
    class MockWebSocket {
      static CONNECTING = 0;
      static OPEN = 1;
      static CLOSING = 2;
      static CLOSED = 3;
      readyState = 0;
      url: string;
      onopen: Listener | null = null;
      onmessage: Listener | null = null;
      onclose: Listener | null = null;
      onerror: Listener | null = null;
      private listeners: Record<string, Listener[]> = { open: [], message: [], close: [], error: [] };
      constructor(url: string) {
        this.url = url;
        (window as unknown as { __mockWs: MockWebSocket }).__mockWs = this;
        queueMicrotask(() => {
          this.readyState = 1;
          const ev = new Event("open");
          this.onopen?.(ev);
          for (const fn of this.listeners.open) fn(ev);
        });
      }
      push(obj: unknown) {
        const ev = new MessageEvent("message", { data: JSON.stringify(obj) });
        this.onmessage?.(ev);
        for (const fn of this.listeners.message) fn(ev);
      }
      addEventListener(ev: string, fn: Listener) {
        (this.listeners[ev] ??= []).push(fn);
      }
      removeEventListener(ev: string, fn: Listener) {
        const arr = this.listeners[ev];
        if (arr) this.listeners[ev] = arr.filter((f) => f !== fn);
      }
      send(_: unknown) {}
      close() {
        this.readyState = 3;
        const ev = new Event("close");
        this.onclose?.(ev);
        for (const fn of this.listeners.close) fn(ev);
      }
    }
    (window as unknown as { WebSocket: unknown }).WebSocket = MockWebSocket;
  });

  return {
    viewRequests,
    overrideView: (fn) => {
      override = fn;
    },
    lastParams: () => viewRequests[viewRequests.length - 1],
  };
}

// ── page helpers shared by the specs ─────────────────────────────────────

type Probe = {
  __panoscopeRenderer?: {
    allNodeIds: () => { id: string }[];
    getSelection: () => string[];
    setSelection: (ids: string[]) => void;
  };
  __panoscopeRefreshView?: () => Promise<void>;
};

export async function nodeIds(page: Page): Promise<string[]> {
  return page.evaluate(() => (window as unknown as Probe).__panoscopeRenderer?.allNodeIds().map((n) => n.id) ?? []);
}

export async function waitForNodes(page: Page, ids: string[], timeout = 5_000): Promise<void> {
  await page.waitForFunction(
    (want) => {
      const have = (window as unknown as Probe).__panoscopeRenderer?.allNodeIds().map((n) => n.id) ?? [];
      return want.every((id) => have.includes(id));
    },
    ids,
    { timeout },
  );
}

export async function waitForNoNodes(page: Page, ids: string[], timeout = 5_000): Promise<void> {
  await page.waitForFunction(
    (gone) => {
      const have = (window as unknown as Probe).__panoscopeRenderer?.allNodeIds().map((n) => n.id) ?? [];
      return gone.every((id) => !have.includes(id));
    },
    ids,
    { timeout },
  );
}

export async function selectedIds(page: Page): Promise<string[]> {
  return page.evaluate(() => (window as unknown as Probe).__panoscopeRenderer?.getSelection() ?? []);
}

export async function refreshView(page: Page): Promise<void> {
  await page.evaluate(() => (window as unknown as Probe).__panoscopeRefreshView?.());
}

/** Dispatch a mouse event on the `<g.node>` bound to `id`. */
export async function nodeEvent(
  page: Page,
  id: string,
  type: "click" | "dblclick" = "click",
  opts: { metaKey?: boolean } = {},
): Promise<void> {
  await page.evaluate(
    ({ id, type, metaKey }) => {
      for (const el of Array.from(document.querySelectorAll("g.node"))) {
        const d = (el as unknown as { __data__?: { id?: string } }).__data__;
        if (d?.id === id) {
          el.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, metaKey: !!metaKey }));
          return;
        }
      }
      throw new Error(`no g.node bound to id=${id}`);
    },
    { id, type, metaKey: !!opts.metaKey },
  );
}

/** Dispatch a double-click on the cluster hull bound to `vendor`. */
export async function hullDblClick(page: Page, vendor: string): Promise<void> {
  await page.evaluate((v) => {
    for (const el of Array.from(document.querySelectorAll("path.hull"))) {
      const d = (el as unknown as { __data__?: { vendor?: string } }).__data__;
      if (d?.vendor === v) {
        el.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
        return;
      }
    }
    throw new Error(`no path.hull bound to vendor=${v}`);
  }, vendor);
}
