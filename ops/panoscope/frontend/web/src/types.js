// SPDX-License-Identifier: Apache-2.0
//
// JSDoc typedefs for the backend wire shapes. No module imports this file
// at run time; it documents the wire for readers and for editors that read
// JSDoc. Shapes come from:
//   - src/snapshot_view.rs (NodeView, EdgeView)
//   - src/main.rs (Snapshot, Legend)
//   - src/zenoh_client.rs (WsEvent)
//   - src/ws.rs (OutgoingMessage)

/**
 * @typedef {"source" | "sink" | "deriver" | "topic" | "unknown"} Role
 */

/**
 * Substrate-level node kind learned from the Zenoh admin namespace.
 * Distinct from `Role`, which is computed from pub/sub overlap. `null`
 * for nodes only seen via schema-decl / data-tail.
 * @typedef {"router" | "peer" | "subscriber"} NodeKind
 */

/**
 * Participant class — the single axis separating underlying
 * infrastructure from application data participants. Computed
 * backend-side (PeerInfo::node_class); the frontend reads it to badge,
 * fill, and show/hide/isolate infra as a group rather than special-
 * casing routers / trashgoat / reserved-namespace publishers one at a
 * time. See docs/design/panoscope.md §"Infrastructure participant class".
 * @typedef {"infra" | "app"} NodeClass
 */

/**
 * @typedef {Object} NodeView
 * @property {string} peer_id
 * @property {string | null} [slot_alias] Slot alias (ADR 1039) — the operator-assigned human name for this
 *   node's slot anchor (e.g. "pat-operator", "tarsier-alpha"), resolved
 *   by the backend from the portal registry when `peer_id` is a known
 *   32-hex anchor. Absent when no alias applies (random ZID, unknown
 *   anchor, or portal-down) so the `display_name`/peer_id fallback is used.
 *   Preferred as the node label when present. `peer_id` stays the
 *   canonical UUID for copy/deep-link — the alias is display-only.
 * @property {string | null} [display_name] Friendly per-peer label (e.g. "tarsier-alpha") declared in the
 *   schema-decl JSON `display_name` field. Falls back to vendor/peer_id
 *   in the renderer when null — preferred for the node label since the
 *   cert-bound vendor is uniform after the notional namespace collapse.
 * @property {string | null} vendor
 * @property {Role} role
 * @property {string[]} publishes
 * @property {string[]} subscribes
 * @property {number} first_seen_ms
 * @property {number} last_seen_ms
 * @property {string | null} [site]
 * @property {string | null} [role_class]
 * @property {string | null} [format_hint]
 * @property {boolean | null} [synthetic]
 * @property {NodeKind | null} [node_kind] Substrate-level node kind from the admin namespace.
 * @property {string | null} [attest_source] Provenance: `"schema_decl"` / `"admin"` / `"data_tail"`.
 * @property {string | null} [locator] Transport locator (e.g. `tcp/198.51.100.7:7447`) from admin payload.
 * @property {NodeClass | null} [node_class] Participant class — `"infra"` for underlying infrastructure, `"app"`
 *   for an application data participant. The axis the INFRA fleet toggle
 *   filters on. Always present from the backend; treated as `"app"` when
 *   absent (older snapshot).
 * @property {string | null} [node_subkind] Infrastructure sub-type key when `node_class === "infra"` (`router`
 *   / `subscriber` / `chaos` / `schema-registry` / `operator-system` /
 *   `infra`), else null. Maps to the side-panel label + explainer.
 * @property {boolean | null} [universal_observer] True when this peer subscribes to the universal `**` wildcard — a
 *   "watches everything" observer. The backend suppresses its per-topic
 *   fan-out edges (they'd be N×M noise); the frontend badges it.
 * @property {NodeHygiene | null} [hygiene] Compact pub/sub-hygiene verdict for this publisher (key-shape only).
 *   Absent for non-publishers. The full report (finding prose + fixes +
 *   the live storage retrievability probe) is fetched on demand from
 *   `/api/publisher/hygiene?source=<peer_id>`.
 */

/**
 * Snapshot-embedded hygiene summary — verdict + stable finding codes.
 * @typedef {Object} NodeHygiene
 * @property {"good" | "review" | "poor"} verdict
 * @property {string[]} codes
 */

/**
 * Full per-publisher hygiene report from `/api/publisher/hygiene`.
 * @typedef {Object} PublisherHygiene
 * @property {"good" | "review" | "poor"} verdict
 * @property {HygieneFinding[]} findings
 * @property {"confirmed" | "unconfirmed" | "notprobed"} retrievability
 * @property {string[]} probe_selectors
 */

/**
 * @typedef {Object} HygieneFinding
 * @property {string} code
 * @property {"info" | "advisory" | "warn"} severity
 * @property {string} message
 * @property {string} fix
 */

/**
 * @typedef {Object} EdgeView
 * @property {string} source
 * @property {string | null} sink
 * @property {string} topic
 * @property {number} ema_eps
 * @property {number} projected_eps
 * @property {number} total_events
 * @property {number} last_event_ms
 * @property {string | null} schema_family
 * @property {string | null} format Latest classified wire format for this topic. One of
 *   "json" / "cbor" / "protobuf" / "text" / "hex" / "empty". `null`
 *   until the first sample for the topic has been buffered.
 * @property {number | null} [folded_singletons_count] Server-side fold marker. When set, this edge is the collapse of N
 *   per-topic edges under the same (publisher, namespace) — the wire-
 *   payload bound from the snapshot fold. The renderer routes these
 *   to the singletons-super substrate (PR #573 visual + interaction)
 *   rather than the regular topic-virtual-node path. Drill-in is
 *   lazy: the namespace inspector's "show on canvas" or double-click fires
 *   GET /api/snapshot/expand?source=<source>&namespace=<folded_namespace>
 *   to fetch the per-topic edges this synthetic edge stands in for.
 * @property {string | null} [folded_namespace] Namespace the server fold grouped on (e.g. "sensorgrid/v1/entities").
 *   Always present with `folded_singletons_count`; the frontend uses
 *   this to build the expand URL without parsing the synthetic topic
 *   string.
 */

/**
 * /api/snapshot/expand response — un-folded per-topic edges for one
 * (source, namespace) singletons-super group, fetched lazily when the
 * operator drills into a server-folded super-edge.
 * @typedef {Object} ExpandedFold
 * @property {string} source
 * @property {string} namespace
 * @property {EdgeView[]} edges
 * @property {number} generated_at_ms
 */

/**
 * @typedef {Object} Snapshot
 * @property {NodeView[]} nodes
 * @property {EdgeView[]} edges
 * @property {Record<string, number>} sampling_drops
 * @property {number} generated_at_ms
 */

// Single honest state: the schema resolver is the compiled-in family-map
// floor. Per-peer edge coloring rides schema-decl self-registration on top
// (see ops/panoscope/src/schema.rs). It is never a portal-
// reachability verdict — the old portal-fetch path was removed.
/**
 * @typedef {"family-map"} ResolverStatus
 */

/**
 * Sibling-surface base URLs (Block 94 cross-surface deep-link
 * contract; see docs/design/zenoh-surface-deep-links.md). Each field
 * is `null`/missing when no sibling is configured at this deployment.
 * The frontend hides the corresponding outbound button rather than
 * emitting a dead href.
 * @typedef {Object} SiblingUrls
 * @property {string | null} [stethoscope] Stethoscope origin (e.g. `https://stethoscope.nova.netbird.nova.example`).
 *   Drives the side-panel `diagnose ↗` button.
 * @property {string | null} [flashlight] Flashlight origin. Drives the side-panel `tail ↗` button.
 * @property {string | null} [portal] Portal origin. Used by the existing schema-page `↗` link. Prefer
 *   over the legacy flat `portal_base_url` field when present.
 */

/**
 * Why the slot-alias registry holds what it holds (ADR 1039 honesty arm).
 * `disabled` = no portal API key was rendered into the container, so the
 * refresher never polls and every peer shows a raw UUID — a deploy gap.
 * `unreached` = configured but no poll has ever succeeded. `stale` = a
 * last-good registry kept across a failed poll. `ok` = current, and an
 * `aliases` of 0 then genuinely means none are assigned.
 * @typedef {"disabled" | "unreached" | "ok" | "stale"} AliasRegistryState
 */

/**
 * @typedef {Object} AliasRegistryHealth
 * @property {AliasRegistryState} state
 * @property {number} aliases
 * @property {number} [last_ok_ms]
 * @property {string} [last_error]
 */

/**
 * @typedef {Object} Legend
 * @property {ResolverStatus} schema_resolver_status
 * @property {number} schema_resolver_known_families
 * @property {AliasRegistryHealth} [alias_registry] Slot-alias registry health. Optional for backwards-compat with a
 *   backend older than the honesty arm — treated as "unknown" and the
 *   legend cell stays `—` rather than asserting a state.
 * @property {Record<string, number>} sampling_drops
 * @property {number} ema_tau_secs
 * @property {number} ws_coalesce_ms
 * @property {number} generated_at_ms
 * @property {string} portal_base_url Origin of the goat-zenoh-portal. Empty when unconfigured.
 *   Backwards-compat: prefer `siblings.portal` when present.
 * @property {SiblingUrls} siblings Sibling-surface base URLs (Block 94). Always present as an object;
 *   inner fields are individually optional.
 * @property {boolean} decoder_enabled True when panoscope's decoded-stream view is wired (portal URL +
 *   API key both configured). When false, the frontend keeps the
 *   Decoded toggle disabled with a deploy-gap tooltip rather than
 *   firing per-topic /api/samples/decoded calls that all 404.
 * @property {LegendLexicon} [lexicon] Display-only lexicon — goatnet-appropriate participant noun +
 *   fabric label. Read on bootstrap to relex JS-generated labels (see
 *   src/lex.ts). Optional for backwards-compat with an older backend.
 */

/**
 * Display lexicon shape on /api/legend (snake_case, mirrors the Rust
 * `config::Lexicon`). See docs/design/portal-lexicon.md §"Panoscope".
 * @typedef {Object} LegendLexicon
 * @property {string} org
 * @property {string} orgs
 * @property {string} org_upper
 * @property {string} orgs_upper
 * @property {string} fabric_label
 */

/**
 * /api/samples/decoded response — protobuf-decoded sample stream
 * rendered in the side-panel "Decoded" tab. Mirrors decoder.rs.
 * @typedef {Object} TopicDecodeOutcome
 * @property {string} topic
 * @property {"decoded" | "passthrough" | "no_schema" | "portal_disabled" | "empty_descriptor" | { transient: string; }} status
 * @property {DecodedSampleRow[]} samples
 */

/**
 * @typedef {Object} DecodedSampleRow
 * @property {number} ts_ms
 * @property {string | null} source_peer
 * @property {number} size_bytes
 * @property {{ decoded: unknown; } | { passthrough: { format: string; body: unknown; }; } | { error: string; } | "no_raw_bytes"} result Variant { passthrough: { format: string; body: unknown; }; }: No protobuf schema; raw JSON/text shown as-is (NOT schema-validated).
 *     `format` is "json" (body = parsed value) or "text" (body = string).
 * @property {boolean} [stale] True when this sample was recalled from Zenoh storage on an empty
 *   live ring (panoscope-stale-recall B1) rather than observed live.
 *   `ts_ms` is then the original publish time. Absent/false for live
 *   samples.
 */

// WsEvent — tagged union matching zenoh_client::WsEvent serde shape
// (#[serde(tag = "kind", rename_all = "snake_case")])

/**
 * Variant kind "sample_arrived": Sample landed in the per-topic ring buffer. Filtered by the
 *   backend against the per-connection watch set, so only topics
 *   the operator has opted into via LIVE-follow reach the browser.
 *   Drives the operator-opt-in LIVE-follow refresh on the stream
 *   pane (see docs/design/panoscope.md §"LIVE-follow on the stream
 *   view"). Carries no payload bytes — the browser still fetches
 *   /api/samples to render.
 * @typedef {(
 *   | { kind: "peer_seen"; peer_id: string; ts_ms: number; }
 *   | { kind: "publish_sampled"; peer_id: string; topic: string; ts_ms: number; ema_eps: number; }
 *   | { kind: "publish_dropped"; peer_id: string; topic: string; ts_ms: number; }
 *   | { kind: "sample_arrived"; topic: string; ts_ms: number; }
 * )} WsEvent
 */

// OutgoingMessage — what /ws sends. Tagged union via `type` discriminator
// matching ws::OutgoingMessage (#[serde(tag = "type", rename_all = "snake_case")]).

/**
 * @typedef {(
 *   | ({ type: "snapshot"; } & Snapshot)
 *   | { type: "delta"; events: WsEvent[]; }
 *   | { type: "lagged"; missed: number; }
 * )} OutgoingMessage
 */

/**
 * Inbound — what the client sends to /ws. Tagged union on `op`
 * matching ws::InboundMessage (#[serde(tag = "op", rename_all =
 * "snake_case")]). Used by the LIVE-follow opt-in on the stream
 * pane to add/remove the currently-painted topic from the
 * per-connection watch set.
 * @typedef {{ op: "watch"; topic: string; } | { op: "unwatch"; topic: string; }} InboundMessage
 */

// ── View model (POST /api/view) ────────────────────────────────────────────
// The server owns grouping (docs/design/panoscope-view-model.md). The browser
// sends ViewParams and draws the ViewResponse it gets back; it never folds.

/**
 * @typedef {"normal" | "simplified"} Detail
 */
/**
 * @typedef {"all" | "real" | "notional"} SourceFilterMode
 */
/**
 * @typedef {"all" | "hide" | "only"} InfraFilterMode
 */
/**
 * @typedef {"hide" | "only"} TestFilterMode
 */

/**
 * @typedef {Object} ViewParams
 * @property {number} window_hours
 * @property {Detail} detail
 * @property {string[]} expand Group ids the operator opened.
 * @property {string[]} fold_peers Peers folded to a single unit.
 * @property {string[]} collapse_orgs Organisations drawn as one node; "*" = all.
 * @property {SourceFilterMode} source
 * @property {InfraFilterMode} infra
 * @property {TestFilterMode} test
 */

/**
 * A peer as this viewer sees it: NodeView's fields, with `publishes` and
 * `subscribes` cut to what the viewer may see.
 * Extends `NodeView`.
 * @typedef {Object} PeerOut
 * @property {number} visible_topics
 * @property {boolean} folded
 */

/**
 * @typedef {Object} TopicOut
 * @property {string} id
 * @property {string} key
 * @property {string[]} publishers
 * @property {number} projected_eps
 * @property {number} total_events
 * @property {number} last_event_ms
 * @property {string | null} format
 * @property {string | null} schema_family
 * @property {number} subscriber_count
 */

/**
 * @typedef {"family" | "subtree" | "publisher"} GroupKind
 */

/**
 * @typedef {Object} GroupOut
 * @property {string} id
 * @property {GroupKind} kind
 * @property {string} publisher
 * @property {string} label
 * @property {string} selector
 * @property {number} topic_count
 * @property {number} projected_eps
 * @property {number} total_events
 * @property {number} last_event_ms
 * @property {Record<string, number>} formats
 * @property {number} subscriber_count
 */

/**
 * @typedef {Object} OrgOut
 * @property {string} id
 * @property {string} org
 * @property {number} peer_count
 * @property {number} topic_count
 * @property {number} projected_eps
 */

/**
 * @typedef {Object} ViewEdge
 * @property {string} from
 * @property {string} to
 * @property {"publish" | "subscribe"} kind
 * @property {number} projected_eps
 * @property {number} total_events
 * @property {number} last_event_ms
 * @property {number} topic_count
 */

/**
 * @typedef {Object} ViewTotals
 * @property {number} peers
 * @property {number} topics
 * @property {number} groups
 * @property {number} msg_per_sec
 * @property {number} total_events
 * @property {number} orgs
 * @property {number} elements
 */

/**
 * @typedef {Object} ViewTruncation
 * @property {number} max_elements
 * @property {number} publisher_budget
 * @property {boolean} over_cap
 * @property {boolean} opened_ignored The opened groups were ignored: with them the view did not fit.
 */

/**
 * @typedef {Object} ViewLimits
 * @property {number} max_elements
 * @property {number} publisher_budget
 * @property {number} max_open_units A publisher draws at most this many units with groups opened in it; a
 *   group whose opening would exceed it stays closed.
 */

/**
 * @typedef {Object} ViewResponse
 * @property {number} generation
 * @property {number} built_at_ms
 * @property {string} etag
 * @property {PeerOut[]} peers
 * @property {TopicOut[]} topics
 * @property {GroupOut[]} groups
 * @property {OrgOut[]} orgs
 * @property {ViewEdge[]} edges
 * @property {ViewTotals} totals
 * @property {ViewTruncation | null} truncated
 * @property {ViewLimits} limits
 * @property {Record<string, number>} sampling_drops
 */

/**
 * @typedef {Object} GroupMemberRow
 * @property {string} key
 * @property {number} projected_eps
 * @property {number} total_events
 * @property {number} last_event_ms
 * @property {string | null} format
 * @property {string | null} schema_family
 * @property {number} subscriber_count
 */

/**
 * @typedef {Object} GroupMembers
 * @property {string} id
 * @property {number} generation
 * @property {number} total
 * @property {number} matched
 * @property {GroupMemberRow[]} rows
 */

/**
 * @typedef {Object} SearchHit
 * @property {string} key
 * @property {string} publisher
 * @property {string} drawn_as
 */

/**
 * @typedef {Object} SearchResult
 * @property {number} generation
 * @property {number} total
 * @property {SearchHit[]} hits
 */

/**
 * @typedef {Object} RevealResult
 * @property {string[]} expand
 * @property {string | null} topic_id
 */
export {};
