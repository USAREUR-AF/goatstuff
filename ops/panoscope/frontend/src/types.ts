// SPDX-License-Identifier: Apache-2.0
//
// TypeScript types matching the backend wire shapes from:
//   - src/snapshot_view.rs (NodeView, EdgeView)
//   - src/main.rs (Snapshot, Legend)
//   - src/zenoh_client.rs (WsEvent)
//   - src/ws.rs (OutgoingMessage)

export type Role = "source" | "sink" | "deriver" | "topic" | "unknown";

/** Substrate-level node kind learned from the Zenoh admin namespace
 *. Distinct from `Role`, which is computed from pub/sub
 *  overlap. `null` for nodes only seen via schema-decl / data-tail. */
export type NodeKind = "router" | "peer" | "subscriber";

/** Participant class — the single axis separating underlying
 *  infrastructure from application data participants. Computed
 *  backend-side (PeerInfo::node_class); the frontend reads it to badge,
 *  fill, and show/hide/isolate infra as a group rather than special-
 *  casing routers / trashgoat / reserved-namespace publishers one at a
 *  time. See docs/design/panoscope.md §"Infrastructure participant class". */
export type NodeClass = "infra" | "app";

export interface NodeView {
  peer_id: string;
  /** Slot alias (ADR 1039) — the operator-assigned human name for this
   *  node's slot anchor (e.g. "pat-operator", "tarsier-alpha"), resolved
   *  by the backend from the portal registry when `peer_id` is a known
   *  32-hex anchor. Absent when no alias applies (random ZID, unknown
   *  anchor, or portal-down) so the `display_name`/peer_id fallback is used.
   *  Preferred as the node label when present. `peer_id` stays the
   *  canonical UUID for copy/deep-link — the alias is display-only. */
  slot_alias?: string | null;
  /** Friendly per-peer label (e.g. "tarsier-alpha") declared in the
   *  schema-decl JSON `display_name` field. Falls back to vendor/peer_id
   *  in the renderer when null — preferred for the node label since the
   *  cert-bound vendor is uniform after the notional namespace collapse. */
  display_name?: string | null;
  vendor: string | null;
  role: Role;
  publishes: string[];
  subscribes: string[];
  first_seen_ms: number;
  last_seen_ms: number;
  site?: string | null;
  role_class?: string | null;
  format_hint?: string | null;
  synthetic?: boolean | null;
  /** Substrate-level node kind from the admin namespace. */
  node_kind?: NodeKind | null;
  /** Provenance: `"schema_decl"` / `"admin"` / `"data_tail"`. */
  attest_source?: string | null;
  /** Transport locator (e.g. `tcp/198.51.100.7:7447`) from admin payload. */
  locator?: string | null;
  /** Participant class — `"infra"` for underlying infrastructure, `"app"`
   *  for an application data participant. The axis the INFRA fleet toggle
   *  filters on. Always present from the backend; treated as `"app"` when
   *  absent (older snapshot). */
  node_class?: NodeClass | null;
  /** Infrastructure sub-type key when `node_class === "infra"` (`router`
   *  / `subscriber` / `chaos` / `schema-registry` / `operator-system` /
   *  `infra`), else null. Maps to the side-panel label + explainer. */
  node_subkind?: string | null;
  /** True when this peer subscribes to the universal `**` wildcard — a
   *  "watches everything" observer. The backend suppresses its per-topic
   *  fan-out edges (they'd be N×M noise); the frontend badges it. */
  universal_observer?: boolean | null;
  /** Compact pub/sub-hygiene verdict for this publisher (key-shape only).
   *  Absent for non-publishers. The full report (finding prose + fixes +
   *  the live storage retrievability probe) is fetched on demand from
   *  `/api/publisher/hygiene?source=<peer_id>`. */
  hygiene?: NodeHygiene | null;
}

/** Snapshot-embedded hygiene summary — verdict + stable finding codes. */
export interface NodeHygiene {
  verdict: "good" | "review" | "poor";
  codes: string[];
}

/** Full per-publisher hygiene report from `/api/publisher/hygiene`. */
export interface PublisherHygiene {
  verdict: "good" | "review" | "poor";
  findings: HygieneFinding[];
  retrievability: "confirmed" | "unconfirmed" | "notprobed";
  probe_selectors: string[];
}

export interface HygieneFinding {
  code: string;
  severity: "info" | "advisory" | "warn";
  message: string;
  fix: string;
}

export interface EdgeView {
  source: string;
  sink: string | null;
  topic: string;
  ema_eps: number;
  projected_eps: number;
  total_events: number;
  last_event_ms: number;
  schema_family: string | null;
  /** Latest classified wire format for this topic. One of
   *  "json" / "cbor" / "protobuf" / "text" / "hex" / "empty". `null`
   *  until the first sample for the topic has been buffered. */
  format: string | null;
  /** Server-side fold marker. When set, this edge is the collapse of N
   *  per-topic edges under the same (publisher, namespace) — the wire-
   *  payload bound from the snapshot fold. The renderer routes these
   *  to the singletons-super substrate (PR #573 visual + interaction)
   *  rather than the regular topic-virtual-node path. Drill-in is
   *  lazy: the namespace inspector's "show on canvas" or double-click fires
   *  GET /api/snapshot/expand?source=<source>&namespace=<folded_namespace>
   *  to fetch the per-topic edges this synthetic edge stands in for. */
  folded_singletons_count?: number | null;
  /** Namespace the server fold grouped on (e.g. "sensorgrid/v1/entities").
   *  Always present with `folded_singletons_count`; the frontend uses
   *  this to build the expand URL without parsing the synthetic topic
   *  string. */
  folded_namespace?: string | null;
}

/** /api/snapshot/expand response — un-folded per-topic edges for one
 *  (source, namespace) singletons-super group, fetched lazily when the
 *  operator drills into a server-folded super-edge. */
export interface ExpandedFold {
  source: string;
  namespace: string;
  edges: EdgeView[];
  generated_at_ms: number;
}

export interface Snapshot {
  nodes: NodeView[];
  edges: EdgeView[];
  sampling_drops: Record<string, number>;
  generated_at_ms: number;
}

// Single honest state: the schema resolver is the compiled-in family-map
// floor. Per-peer edge coloring rides schema-decl self-registration on top
// (see ops/panoscope/src/schema.rs). It is never a portal-
// reachability verdict — the old portal-fetch path was removed.
export type ResolverStatus = "family-map";

/** Sibling-surface base URLs (Block 94 cross-surface deep-link
 *  contract; see docs/design/zenoh-surface-deep-links.md). Each field
 *  is `null`/missing when no sibling is configured at this deployment.
 *  The frontend hides the corresponding outbound button rather than
 *  emitting a dead href. */
export interface SiblingUrls {
  /** Stethoscope origin (e.g. `https://stethoscope.nova.netbird.nova.example`).
   *  Drives the side-panel `diagnose ↗` button. */
  stethoscope?: string | null;
  /** Flashlight origin. Drives the side-panel `tail ↗` button. */
  flashlight?: string | null;
  /** Portal origin. Used by the existing schema-page `↗` link. Prefer
   *  over the legacy flat `portal_base_url` field when present. */
  portal?: string | null;
}

/** Why the slot-alias registry holds what it holds (ADR 1039 honesty arm).
 *  `disabled` = no portal API key was rendered into the container, so the
 *  refresher never polls and every peer shows a raw UUID — a deploy gap.
 *  `unreached` = configured but no poll has ever succeeded. `stale` = a
 *  last-good registry kept across a failed poll. `ok` = current, and an
 *  `aliases` of 0 then genuinely means none are assigned. */
export type AliasRegistryState = "disabled" | "unreached" | "ok" | "stale";

export interface AliasRegistryHealth {
  state: AliasRegistryState;
  aliases: number;
  last_ok_ms?: number;
  last_error?: string;
}

export interface Legend {
  schema_resolver_status: ResolverStatus;
  schema_resolver_known_families: number;
  /** Slot-alias registry health. Optional for backwards-compat with a
   *  backend older than the honesty arm — treated as "unknown" and the
   *  legend cell stays `—` rather than asserting a state. */
  alias_registry?: AliasRegistryHealth;
  sampling_drops: Record<string, number>;
  ema_tau_secs: number;
  ws_coalesce_ms: number;
  generated_at_ms: number;
  /** Origin of the goat-zenoh-portal. Empty when unconfigured.
   *  Backwards-compat: prefer `siblings.portal` when present. */
  portal_base_url: string;
  /** Sibling-surface base URLs (Block 94). Always present as an object;
   *  inner fields are individually optional. */
  siblings: SiblingUrls;
  /** True when panoscope's decoded-stream view is wired (portal URL +
   *  API key both configured). When false, the frontend keeps the
   *  Decoded toggle disabled with a deploy-gap tooltip rather than
   *  firing per-topic /api/samples/decoded calls that all 404. */
  decoder_enabled: boolean;
  /** Display-only lexicon — goatnet-appropriate participant noun +
   *  fabric label. Read on bootstrap to relex JS-generated labels (see
   *  src/lex.ts). Optional for backwards-compat with an older backend. */
  lexicon?: LegendLexicon;
}

/** Display lexicon shape on /api/legend (snake_case, mirrors the Rust
 *  `config::Lexicon`). See docs/design/portal-lexicon.md §"Panoscope". */
export interface LegendLexicon {
  org: string;
  orgs: string;
  org_upper: string;
  orgs_upper: string;
  fabric_label: string;
}

/** /api/samples/decoded response — protobuf-decoded sample stream
 *  rendered in the side-panel "Decoded" tab. Mirrors decoder.rs. */
export interface TopicDecodeOutcome {
  topic: string;
  status:
    | "decoded"
    | "passthrough"
    | "no_schema"
    | "portal_disabled"
    | "empty_descriptor"
    | { transient: string };
  samples: DecodedSampleRow[];
}

export interface DecodedSampleRow {
  ts_ms: number;
  source_peer: string | null;
  size_bytes: number;
  result:
    | { decoded: unknown }
    // No protobuf schema; raw JSON/text shown as-is (NOT schema-validated).
    // `format` is "json" (body = parsed value) or "text" (body = string).
    | { passthrough: { format: string; body: unknown } }
    | { error: string }
    | "no_raw_bytes";
  /** True when this sample was recalled from Zenoh storage on an empty
   *  live ring (panoscope-stale-recall B1) rather than observed live.
   *  `ts_ms` is then the original publish time. Absent/false for live
   *  samples. */
  stale?: boolean;
}

// WsEvent — tagged union matching zenoh_client::WsEvent serde shape
// (#[serde(tag = "kind", rename_all = "snake_case")])

export type WsEvent =
  | { kind: "peer_seen"; peer_id: string; ts_ms: number }
  | {
      kind: "publish_sampled";
      peer_id: string;
      topic: string;
      ts_ms: number;
      ema_eps: number;
    }
  | {
      kind: "publish_dropped";
      peer_id: string;
      topic: string;
      ts_ms: number;
    }
  /** Sample landed in the per-topic ring buffer. Filtered by the
   *  backend against the per-connection watch set, so only topics
   *  the operator has opted into via LIVE-follow reach the browser.
   *  Drives the operator-opt-in LIVE-follow refresh on the stream
   *  pane (see docs/design/panoscope.md §"LIVE-follow on the stream
   *  view"). Carries no payload bytes — the browser still fetches
   *  /api/samples to render. */
  | { kind: "sample_arrived"; topic: string; ts_ms: number };

// OutgoingMessage — what /ws sends. Tagged union via `type` discriminator
// matching ws::OutgoingMessage (#[serde(tag = "type", rename_all = "snake_case")]).

export type OutgoingMessage =
  | ({ type: "snapshot" } & Snapshot)
  | { type: "delta"; events: WsEvent[] }
  | { type: "lagged"; missed: number };

/** Inbound — what the client sends to /ws. Tagged union on `op`
 *  matching ws::InboundMessage (#[serde(tag = "op", rename_all =
 *  "snake_case")]). Used by the LIVE-follow opt-in on the stream
 *  pane to add/remove the currently-painted topic from the
 *  per-connection watch set. */
export type InboundMessage =
  | { op: "watch"; topic: string }
  | { op: "unwatch"; topic: string };

// ── View model (POST /api/view) ────────────────────────────────────────────
// The server owns grouping (docs/design/panoscope-view-model.md). The browser
// sends ViewParams and draws the ViewResponse it gets back; it never folds.

export type Detail = "normal" | "simplified";
export type SourceFilterMode = "all" | "real" | "notional";
export type InfraFilterMode = "all" | "hide" | "only";
export type TestFilterMode = "hide" | "only";

export interface ViewParams {
  window_hours: number;
  detail: Detail;
  /** Group ids the operator opened. */
  expand: string[];
  /** Peers folded to a single unit. */
  fold_peers: string[];
  /** Organisations drawn as one node; "*" = all. */
  collapse_orgs: string[];
  source: SourceFilterMode;
  infra: InfraFilterMode;
  test: TestFilterMode;
}

/** A peer as this viewer sees it: NodeView's fields, with `publishes` and
 *  `subscribes` cut to what the viewer may see. */
export interface PeerOut extends NodeView {
  visible_topics: number;
  folded: boolean;
}

export interface TopicOut {
  id: string;
  key: string;
  publishers: string[];
  projected_eps: number;
  total_events: number;
  last_event_ms: number;
  format: string | null;
  schema_family: string | null;
  subscriber_count: number;
}

export type GroupKind = "family" | "subtree" | "publisher";

export interface GroupOut {
  id: string;
  kind: GroupKind;
  publisher: string;
  label: string;
  selector: string;
  topic_count: number;
  projected_eps: number;
  total_events: number;
  last_event_ms: number;
  formats: Record<string, number>;
  subscriber_count: number;
}

export interface OrgOut {
  id: string;
  org: string;
  peer_count: number;
  topic_count: number;
  projected_eps: number;
}

export interface ViewEdge {
  from: string;
  to: string;
  kind: "publish" | "subscribe";
  projected_eps: number;
  total_events: number;
  last_event_ms: number;
  topic_count: number;
}

export interface ViewTotals {
  peers: number;
  topics: number;
  groups: number;
  msg_per_sec: number;
  total_events: number;
  orgs: number;
  elements: number;
}

export interface ViewTruncation {
  max_elements: number;
  publisher_budget: number;
  over_cap: boolean;
  /** The opened groups were ignored: with them the view did not fit. */
  opened_ignored: boolean;
}

export interface ViewLimits {
  max_elements: number;
  publisher_budget: number;
  /** A publisher draws at most this many units with groups opened in it; a
   *  group whose opening would exceed it stays closed. */
  max_open_units: number;
}

export interface ViewResponse {
  generation: number;
  built_at_ms: number;
  etag: string;
  peers: PeerOut[];
  topics: TopicOut[];
  groups: GroupOut[];
  orgs: OrgOut[];
  edges: ViewEdge[];
  totals: ViewTotals;
  truncated: ViewTruncation | null;
  limits: ViewLimits;
  sampling_drops: Record<string, number>;
}

export interface GroupMemberRow {
  key: string;
  projected_eps: number;
  total_events: number;
  last_event_ms: number;
  format: string | null;
  schema_family: string | null;
  subscriber_count: number;
}

export interface GroupMembers {
  id: string;
  generation: number;
  total: number;
  matched: number;
  rows: GroupMemberRow[];
}

export interface SearchHit {
  key: string;
  publisher: string;
  drawn_as: string;
}

export interface SearchResult {
  generation: number;
  total: number;
  hits: SearchHit[];
}

export interface RevealResult {
  expand: string[];
  topic_id: string | null;
}
