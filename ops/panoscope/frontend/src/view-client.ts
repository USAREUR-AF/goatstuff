// SPDX-License-Identifier: Apache-2.0
//
// The browser half of the view model (docs/design/panoscope-view-model.md).
//
// Holds the operator's ViewParams — window, detail, opened groups, folded
// peers, collapsed organisations, filters — and asks the server for the view
// they describe. Nothing here folds, filters or counts: the server owns all of
// it, so every tab, the inspector and search agree by construction.
//
// Parameters persist per browser (localStorage) so a reload keeps the view.
// Requests carry If-None-Match; an unchanged model answers 304 with no body.

import type {
  GroupMembers,
  RevealResult,
  SearchResult,
  ViewParams,
  ViewResponse,
} from "./types";

const STORE_KEY = "panoscope.viewParams.v1";
/** Same bound as the fleet poll: a slow build must not stack requests. */
export const VIEW_FETCH_TIMEOUT_MS = 8_000;

export function defaultParams(): ViewParams {
  return {
    window_hours: 6,
    detail: "normal",
    expand: [],
    fold_peers: [],
    collapse_orgs: [],
    source: "all",
    infra: "all",
    test: "hide",
  };
}

/** Merge a stored or partial object onto the defaults, dropping anything of
 *  the wrong shape rather than sending it to the server. */
export function normalizeParams(raw: unknown): ViewParams {
  const d = defaultParams();
  if (!raw || typeof raw !== "object") return d;
  const r = raw as Record<string, unknown>;
  const strs = (v: unknown): string[] =>
    Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
  const oneOf = <T extends string>(v: unknown, allowed: readonly T[], dflt: T): T =>
    typeof v === "string" && (allowed as readonly string[]).includes(v) ? (v as T) : dflt;
  return {
    window_hours:
      typeof r.window_hours === "number" && Number.isFinite(r.window_hours) && r.window_hours >= 0
        ? Math.floor(r.window_hours)
        : d.window_hours,
    detail: oneOf(r.detail, ["normal", "simplified"] as const, d.detail),
    expand: strs(r.expand),
    fold_peers: strs(r.fold_peers),
    collapse_orgs: strs(r.collapse_orgs),
    source: oneOf(r.source, ["all", "real", "notional"] as const, d.source),
    infra: oneOf(r.infra, ["all", "hide", "only"] as const, d.infra),
    test: oneOf(r.test, ["hide", "only"] as const, d.test),
  };
}

/** Pre-view-model settings a returning viewer already chose: the history
 *  window and the SIMPLIFY toggle. Read once, when no view parameters are
 *  stored yet, so the upgrade does not reset them. */
const LEGACY_WINDOW_KEY = "panoscope.historyWindowHours";
const LEGACY_SIMPLIFY_KEY = "panoscope.aggressiveView";

export function loadParams(): ViewParams {
  try {
    const raw = localStorage.getItem(STORE_KEY);
    if (raw) return normalizeParams(JSON.parse(raw));
    const legacy: Record<string, unknown> = {};
    const w = Number(localStorage.getItem(LEGACY_WINDOW_KEY));
    if (w > 0) legacy.window_hours = w;
    if (localStorage.getItem(LEGACY_SIMPLIFY_KEY) === "1") legacy.detail = "simplified";
    return normalizeParams(legacy);
  } catch {
    return defaultParams();
  }
}

function saveParams(p: ViewParams): void {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(p));
  } catch {
    /* private window / blocked storage: the view still works, it just
       does not survive a reload */
  }
}

/** Outcome of one fetch: a new view, "unchanged" (304), or an HTTP status the
 *  caller's session/back-off rules act on. */
export type FetchOutcome =
  | { kind: "view"; view: ViewResponse; params: ViewParams }
  | { kind: "unchanged" }
  /** The parameters changed while the request was in flight; the answer
   *  describes a view nobody is asking for any more. */
  | { kind: "superseded" }
  | { kind: "http"; status: number };

type Listener = (p: ViewParams) => void;

export class ViewClient {
  private params: ViewParams;
  private etag: string | null = null;
  private listeners: Listener[] = [];

  constructor(initial: ViewParams = loadParams()) {
    this.params = initial;
  }

  get(): ViewParams {
    return this.params;
  }

  /** Apply a change, persist it, drop the cached ETag (a different request),
   *  and tell listeners so the page refetches now rather than at next poll. */
  update(change: (p: ViewParams) => ViewParams): void {
    const next = normalizeParams(change({ ...this.params, expand: [...this.params.expand], fold_peers: [...this.params.fold_peers], collapse_orgs: [...this.params.collapse_orgs] }));
    if (JSON.stringify(next) === JSON.stringify(this.params)) return;
    this.params = next;
    this.etag = null;
    saveParams(next);
    for (const l of this.listeners) l(next);
  }

  onChange(l: Listener): void {
    this.listeners.push(l);
  }

  // ── gestures, as parameter edits ──────────────────────────────────────

  openGroup(id: string): void {
    this.update((p) => ({ ...p, expand: p.expand.includes(id) ? p.expand : [...p.expand, id] }));
  }

  /** Close every opened group belonging to one publisher (the side panel's
   *  "fold back"): group ids are `grp:<publisher>:…`. */
  closeGroupsOf(publisher: string): void {
    const prefix = `grp:${publisher}:`;
    this.update((p) => ({ ...p, expand: p.expand.filter((g) => !g.startsWith(prefix)) }));
  }

  togglePeerFold(peer: string): void {
    this.update((p) => ({
      ...p,
      fold_peers: p.fold_peers.includes(peer) ? p.fold_peers.filter((x) => x !== peer) : [...p.fold_peers, peer],
    }));
  }

  toggleOrg(org: string): void {
    this.update((p) => ({
      ...p,
      collapse_orgs: p.collapse_orgs.includes(org)
        ? p.collapse_orgs.filter((x) => x !== org)
        : [...p.collapse_orgs.filter((x) => x !== "*"), org],
    }));
  }

  collapseAllOrgs(): void {
    this.update((p) => ({ ...p, collapse_orgs: ["*"] }));
  }

  expandAll(): void {
    this.update((p) => ({ ...p, collapse_orgs: [], fold_peers: [], expand: [] }));
  }

  // ── requests ──────────────────────────────────────────────────────────

  async fetchView(signal?: AbortSignal): Promise<FetchOutcome> {
    const params = this.params;
    const headers: Record<string, string> = { "content-type": "application/json" };
    if (this.etag) headers["if-none-match"] = this.etag;
    const resp = await fetch("/api/view", {
      method: "POST",
      cache: "no-store",
      headers,
      body: JSON.stringify(params),
      signal: signal ?? AbortSignal.timeout(VIEW_FETCH_TIMEOUT_MS),
    });
    if (this.params !== params) return { kind: "superseded" };
    if (resp.status === 304) return { kind: "unchanged" };
    if (!resp.ok) return { kind: "http", status: resp.status };
    const view = (await resp.json()) as ViewResponse;
    if (this.params !== params) return { kind: "superseded" };
    this.etag = resp.headers.get("etag") ?? view.etag ?? null;
    return { kind: "view", view, params };
  }

  async groupMembers(id: string, q = "", offset = 0, limit = 200): Promise<GroupMembers | null> {
    const resp = await fetch("/api/view/group", {
      method: "POST",
      cache: "no-store",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ params: this.params, id, q, offset, limit }),
      signal: AbortSignal.timeout(VIEW_FETCH_TIMEOUT_MS),
    });
    return resp.ok ? ((await resp.json()) as GroupMembers) : null;
  }

  async search(q: string, limit = 30): Promise<SearchResult | null> {
    const resp = await fetch("/api/view/search", {
      method: "POST",
      cache: "no-store",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ params: this.params, q, limit }),
      signal: AbortSignal.timeout(VIEW_FETCH_TIMEOUT_MS),
    });
    return resp.ok ? ((await resp.json()) as SearchResult) : null;
  }

  /** Ask which groups to open so `key` is its own pin, then open them. */
  async reveal(key: string, publisher: string): Promise<string | null> {
    const resp = await fetch("/api/view/reveal", {
      method: "POST",
      cache: "no-store",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ params: this.params, key, publisher }),
      signal: AbortSignal.timeout(VIEW_FETCH_TIMEOUT_MS),
    });
    if (!resp.ok) return null;
    const r = (await resp.json()) as RevealResult;
    if (r.expand.length > 0) {
      this.update((p) => ({ ...p, expand: [...p.expand, ...r.expand.filter((g) => !p.expand.includes(g))] }));
    }
    return r.topic_id;
  }
}
