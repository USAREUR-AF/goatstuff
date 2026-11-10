// SPDX-License-Identifier: Apache-2.0
//
// D3-driven topology + flow renderer.
//
// Visualization primitives per docs/design/panoscope.md §"Frontend
// visualization":
//
//   - Node = peer; color encodes role; size encodes cumulative
//     bytes-out-per-second EMA proxy (total_events recency).
//   - Edge = topic flow from publisher → (subscriber|null); stroke
//     color encodes registered schema family; stroke width tracks
//     projected EPS (decayed to "now"); transparent when projected
//     EPS is near zero.
//   - Packet dot = brief animated circle traveling along an edge each
//     time a publish_sampled event arrives.

// d3 v7.9.0 is one vendored file (../vendor/d3/README.md). The file is a UMD
// bundle; loaded as a module it sets globalThis.d3.
import "../vendor/d3/d3.min.js";
import { shortId } from "./ids.js";
import { partitionDanglingEdges } from "./edge-guard.js";
import { topologySignature, topologyChanged } from "./layout-settle.js";

const d3 = globalThis.d3;

/**
 * Extends `d3.SimulationNodeDatum`.
 * @typedef {Object} RNode
 * @property {string} id
 * @property {string | null} [slot_alias] Slot alias (ADR 1039) — operator-assigned human name for the node's
 *   slot anchor (e.g. "pat-operator"). Preferred as the node label when
 *   present; `id` stays the canonical UUID. Absent → display_name/vendor
 *   fallback.
 * @property {string | null} [display_name]
 * @property {string | null} vendor
 * @property {import("./types.js").Role} role
 * @property {string | null} [role_class] Schema-decl role-class (`sensor` / `effector` / `data-service` /
 *   `c2` / `observer` / `sink`). Drives fill color when present so the
 *   6-class taxonomy reads at a glance; falls through to the pub/sub-
 *   derived `role` color when absent.
 * @property {string | null} [site] Operating site (`alpha-fob` / `bravo-fob` / `charlie-fob` /
 *   `theater`). Carried so future encodings (stroke, hull tint) can
 *   read it without a re-binding pass.
 * @property {string | null} [node_kind] Substrate-level node kind from the admin namespace (`router` /
 *   `peer` / `subscriber`). When set to `"router"` the renderer flips
 *   to the infrastructure visual class (square shape, subdued
 *   desaturated fill, label "zenoh-router") so the Zenoh broker
 *   doesn't read as a peer that lost its identity. `null` for
 *   application peers (schema-decl populated).
 * @property {string | null} [node_class] Participant class — `"infra"` (underlying infrastructure: router,
 *   observer subscriber, schema-registry / operator-system publisher,
 *   chaos generator, or self-declared) vs `"app"`. Computed backend-side;
 *   the single axis the infra fill + INFRA fleet filter key off of, so
 *   the renderer never re-derives "is this substrate?" per glyph.
 * @property {string | null} [node_subkind] Infrastructure sub-type key (`router` / `subscriber` / `chaos` /
 *   `schema-registry` / `operator-system` / `infra`) when infra, else
 *   null. Drives the side-panel label + explainer.
 * @property {boolean | null} [universal_observer] True when this peer subscribes to `**` — a universal observer whose
 *   per-topic fan-out edges the backend suppresses; rendered with a marker
 *   so an edgeless watch-all node reads as intentional, not orphaned.
 * @property {import("./types.js").NodeHygiene | null} [hygiene] Compact pub/sub-hygiene verdict for this publisher (key-shape only);
 *   carried through to the side panel which badges it + lazily fetches the
 *   full report. Absent for non-publishers.
 * @property {string[]} publishes
 * @property {string[]} subscribes
 * @property {number} total_events
 * @property {number} first_seen_ms
 * @property {number} last_seen_ms
 * @property {boolean | null} [synthetic]
 * @property {boolean} [is_vendor_super] Vendor-collapse super-node marker. When true, this RNode represents
 *   a whole vendor whose peers + intra-vendor topics have been hidden
 *   into a single composite glyph (chunky rounded square, always
 *   labeled "VENDOR  Np·Mt"). Clicking it expands the vendor back to
 *   its constituent peers. Set on synthetic ids of the form
 *   `org:<vendor>`.
 * @property {number} [super_peer_count] When `is_vendor_super`, the count of peers folded inside. Drives
 *   the super-node label + sidebar summary.
 * @property {number} [super_topic_count] When `is_vendor_super`, the count of distinct topics folded
 *   inside (key_exprs whose first segment is this vendor).
 * @property {boolean} [is_singleton_super] Group marker: this node is a server group from /api/view — many of
 *   one publisher's topics drawn as one element (id `grp:<publisher>:…`).
 *   Hover shows the member count; a click opens the group inspector.
 * @property {string} [singleton_source_peer] When `is_singleton_super`, the publishing peer the group hangs off.
 * @property {string} [singleton_namespace] When `is_singleton_super`, the group's display pattern (`a/b/*`,
 *   `a/b/*` + `/…`, `a/b/**`, or `**` for the whole publisher).
 * @property {number} [singleton_count] When `is_singleton_super`, the number of topics in the group.
 * @property {boolean} [has_collapsed_singletons] Peer-side indicator: this peer publishes at least one group. The
 *   renderer overlays a darker inner dot so the operator can see at a
 *   glance which peers have grouped topics.
 * @property {boolean} [manually_collapsed] This peer is folded (double-click): all its topics are one group. Same
 *   inner-dot visual as has_collapsed_singletons; distinguished by intent.
 * @property {string} [group_kind] Server group kind (`family` / `subtree` / `publisher`) when this node is
 *   a group from /api/view.
 */

/**
 * Extends `d3.SimulationLinkDatum<RNode>`.
 * @typedef {Object} REdge
 * @property {string | RNode} source
 * @property {string | RNode} target
 * @property {string} topic
 * @property {number} ema_eps
 * @property {number} projected_eps
 * @property {number} total_events
 * @property {string | null} schema_family
 * @property {number} last_event_ms
 * @property {"publish" | "subscribe"} [edge_kind]
 */

/**
 * Where gestures go. The server owns grouping (docs/design/panoscope-view-model.md),
 * so folding a peer, opening a group or collapsing an organisation is a
 * ViewParams edit; the page's ViewClient applies it and refetches.
 * @typedef {Object} ViewIntents
 * @property {(id: string) => void} openGroup
 * @property {(peer: string) => void} togglePeerFold
 * @property {(org: string) => void} toggleOrg
 * @property {() => void} collapseAllOrgs
 * @property {() => void} expandAll
 * @property {(key: string, publisher: string) => Promise<string | null>} reveal
 */

const ROLE_COLORS = {
  source: "var(--role-source)",
  sink: "var(--role-sink)",
  deriver: "var(--role-deriver)",
  topic: "var(--role-topic, #4a5568)",
  unknown: "var(--role-unknown)",
};

/** Per-role-class fill palette (schema-decl driven). When a peer's
 *  schema-decl carries a `role_class`, the renderer prefers this map
 *  over ROLE_COLORS so the 6-class fabric taxonomy renders distinctly
 *  even when every peer is pub-only (all `Role::Source`) under one
 *  cert-bound vendor — the situation the namespace collapse
 *  creates on a trial fabric. Unknown role_class values + peers
 *  without a schema-decl fall through to ROLE_COLORS in `fillFor` below. */
const ROLE_CLASS_COLORS = {
  sensor: "var(--role-class-sensor)",
  effector: "var(--role-class-effector)",
  "data-service": "var(--role-class-data-service)",
  c2: "var(--role-class-c2)",
  observer: "var(--role-class-observer)",
  sink: "var(--role-class-sink)",
};

function fillFor(n) {
  // Vendor-collapse super-node gets the elevated background tone so
  // the chunky glyph reads as a panel ("a thing you click to expand")
  // rather than blending with one of the peer roles. The vendor border
  // (set in render path) still carries the vendor identity.
  if (n.is_vendor_super) {
    return "var(--bg-elevated)";
  }
  // Singletons-super: same elevated tone as vendor-super so both
  // "thing you click to expand" affordances read as panels, not peers.
  // Distinguished from vendor-super by shape (chunky rounded square vs
  // pill) so the eye can tell them apart.
  if (n.is_singleton_super) {
    return "var(--bg-elevated)";
  }
  // Underlying infrastructure (router, observer subscriber, schema-
  // registry / operator-system publisher, chaos generator, self-declared)
  // gets a subdued desaturated fill that reads as "substrate, not peer" —
  // sits in the visual background of the COP so the eye doesn't latch onto
  // it as an application data participant. The shared fill is the common
  // signal across the whole infra class; the per-subkind glyph (square /
  // star / role shape) carries which kind of infra it is. Generalized
  // from the router-only check so every infra node reads as one class.
  if (n.node_class === "infra") {
    return "var(--role-class-infra)";
  }
  if (n.role_class && ROLE_CLASS_COLORS[n.role_class]) {
    return ROLE_CLASS_COLORS[n.role_class];
  }
  return ROLE_COLORS[n.role];
}

const SCHEMA_COLORS = {
  lattice: "var(--schema-lattice)",
  ecn: "var(--schema-ecn)",
  cot: "var(--schema-cot)",
  community: "var(--schema-community)",
  caracal: "var(--schema-caracal)",
};

function schemaColor(family) {
  if (!family) return "var(--schema-unknown)";
  return SCHEMA_COLORS[family] ?? "var(--schema-unknown)";
}

function nodeRadius(n) {
  // Singletons-super: chunkier than a topic-pin (it represents many)
  // but smaller than a vendor-super (it's narrower in scope). Scales
  // with the folded-topic count so a 4500-topic fold reads visibly
  // weightier than a 12-topic fold.
  if (n.is_singleton_super) {
    const base = 16;
    return base + Math.min(12, Math.log1p(n.singleton_count ?? 0) * 2.5);
  }
  // Topic-virtual-nodes render small so the eye reads them as
  // mailboxes, not peers.
  if (n.role === "topic") return 6;
  // Super-nodes render visibly chunkier than peers so the eye reads
  // them as "many things folded together." Scales gently with the
  // folded-peer count (capped) so a 100-peer vendor is bigger than a
  // 3-peer vendor without dwarfing the canvas.
  if (n.is_vendor_super) {
    const base = 22;
    return base + Math.min(14, Math.log1p(n.super_peer_count ?? 0) * 3);
  }
  // Logarithmic so very-busy peers don't dominate the canvas.
  const base = 10;
  return base + Math.min(20, Math.log1p(n.total_events) * 2);
}

function edgeWidth(e) {
  // EMA up to ~50 eps maps to 0.5..6 px
  const eps = Math.max(0.01, e.projected_eps);
  return 0.5 + Math.min(5.5, Math.log1p(eps) * 1.5);
}

function edgeOpacity(e) {
  // Fade to near-invisible when projected EPS drops near zero.
  return Math.max(0.08, Math.min(0.9, 0.1 + e.projected_eps / 10));
}

/** A peer with no sample for this long reads as "not live" — the history-lens
 *  selected window may still include it (it published within the window), but
 *  it should render faded so live vs historical is unmistakable. Topic/super
 *  nodes carry last_seen_ms=0 and are never aged. (last_seen_ms is server time;
 *  client/server skew of a few seconds is immaterial against this threshold,
 *  and the window membership itself is decided server-side.) */
const AGED_AFTER_MS = 120_000;
function isAgedNode(d) {
  return d.last_seen_ms > 0 && Date.now() - d.last_seen_ms > AGED_AFTER_MS;
}

/**
 * @typedef {Object} RendererOptions
 * @property {(n: RNode) => void} [onNodeClick]
 * @property {(e: REdge) => void} [onEdgeClick]
 * @property {(selectedIds: string[]) => void} [onSelectionChange]
 */

/** Convert client-space coordinates to SVG-local coordinates. */
function clientToSvgPoint(svg, x, y) {
  const pt = svg.createSVGPoint();
  pt.x = x;
  pt.y = y;
  const ctm = svg.getScreenCTM();
  if (!ctm) return { x: 0, y: 0 };
  const inv = pt.matrixTransform(ctm.inverse());
  return { x: inv.x, y: inv.y };
}

/** Top-level key_expr prefixes that are reserved on the fabric (per
 *  the Zenoh data-fabric design, twelve total) but are NOT actively
 *  used as data-bearing namespaces on a trial fabric. They
 *  show up in the topology because of fabric-internal plumbing
 *  (e.g. synth-vendor's 30s schema-decl heartbeat publishes onto
 *  `attest/schema/<vendor>/<peer>/v1`), but the long-term
 *  schema-enforcer / data-dictionary consumers of this namespace are
 *  not deployed — the goat-zenoh-portal stores schemas in
 *  a Postgres registry today as a facilitation expedient (see
 *  docs/design/portal-schema-publishing.md §"Long-term evolution").
 *  The panoscope viz tags these clusters as "future design" so an
 *  operator viewing the COP for the first time doesn't mistake the
 *  schema-decl heartbeat for live application traffic. */
const FUTURE_DESIGN_PREFIXES = new Set(["attest"]);

function isFutureCluster(vendor) {
  return FUTURE_DESIGN_PREFIXES.has(vendor.toLowerCase());
}

/** True if a topic key_expr lives under a future-design top-level
 *  prefix (e.g. `attest/schema/...`). Used by the side panel to
 *  surface the "future design" hint on per-topic selection. */
export function isFutureDesignKeyExpr(keyExpr) {
  const head = keyExpr.split("/")[0]?.toLowerCase() ?? "";
  return FUTURE_DESIGN_PREFIXES.has(head);
}

/** Extract a vendor key (namespace) from a node id. */
function vendorOf(node) {
  if (node.vendor) return node.vendor;
  if (node.id.startsWith("org:")) {
    return node.id.slice("org:".length);
  }
  if (node.id.startsWith("grp:")) {
    // A group of a publisher with no vendor (e.g. a live publisher whose
    // cert lacks a vendor URI-SAN). Don't derive a vendor from the id —
    // return the orphan sentinel so the cluster-hull filter skips it.
    return "(orphan)";
  }
  if (node.role === "topic" && node.id.startsWith("topic:")) {
    const segs = node.id.slice("topic:".length).split("/").filter(Boolean);
    return segs[0] ?? "?";
  }
  // ZID-shaped → no derivable vendor
  if (/^[0-9a-f]{16,32}$/i.test(node.id)) return "(peer)";
  // topic-prefix fallback shape `<vendor>/<system>`
  const segs = node.id.split("/").filter(Boolean);
  return segs[0] ?? "?";
}

/** Parent-namespace of a topic key_expr: everything but the last `/`
 *  segment. Used by the singleton-fanout-collapse to group sibling
 *  1-msg topic-broker pins (e.g. `vendor-c/fleet/entities/v3/<uuid>` →
 *  `vendor-c/fleet/entities/v3`). Returns the empty string for keys
 *  with no `/`, which then bucket together as a degenerate group. */
export function topicNamespaceOf(key) {
  const i = key.lastIndexOf("/");
  return i > 0 ? key.slice(0, i) : "";
}

/** Hover-badge warning: opening a family of more than this many topics draws
 *  each one, so the badge carries "⚠ may slow browser". Matches the group
 *  inspector's "show on canvas" warning (CANVAS_OPEN_WARN in main.ts). */
const GROUP_OPEN_WARN_THRESHOLD = 150;

/** How long a gesture waits for the view that draws its result before giving
 *  up on selecting it. Well past one poll plus a slow response. */
const PENDING_SELECT_MS = 15_000;

export class GraphRenderer {
  svg;
  viewport;
  nodes = [];
  edges = [];
  simulation;
  hullGroup;
  nodeGroup;
  edgeGroup;
  dotGroup;
  hullLabelGroup;
  lassoRect;
  width = 800;
  height = 600;
  options;
  // Selection drives all label/opacity/animation decisions. Empty
  // selection = default-overview state (cluster labels only, no node
  // labels, packet-dots throttled).
  selectedIds = new Set();
  neighborIds = new Set(); // 2-hop graph walkout from selection
  /** Fleet aggregate rate (sum of all edge projected_eps) — fed in by
   *  main.ts from the view totals; drives adaptive packet-dot throttle. */
  fleetMsgPerSec = 0;
  /** The last view drawn, and the parameters it was requested with. */
  lastView = null;
  viewParams = null;
  /** Gesture sink, set by main.ts (`setViewIntents`). */
  intents = null;
  /** What to select once a view that draws it arrives: a revealed topic's
   *  pin, a folded peer's group, a collapsed organisation. Expires so a
   *  request the server could not honour never selects something later. */
  pendingSelect = null;
  /** Component isolation (the "show me only what this is connected to"
   *  gesture). `focusAnchors` is what the operator picked; the member set is
   *  recomputed from the drawn graph on every view, so a component that grows
   *  or shrinks stays honest. Local to this browser and this session: it is a
   *  lens over what the server already served, not a view parameter. */
  focusAnchors = null;
  focusHidden = 0;
  /** Packet-dot lookup: `publisher|topic` → the publish edge drawing that
   *  topic's pin, and per publisher the groups (selector + publish edge). */
  pinEdgeIndex = new Map();
  groupEdgeIndex = new Map();
  // Signatures of the last *rendered* topology (the derived node-id set and
  // the derived edge source|target|topic set). The force layout is re-heated
  // only when one of these changes — data-only refreshes (rates, counts,
  // labels arriving on the ~1.5s snapshot poll) update in place and let the
  // simulation keep cooling to equilibrium instead of jiggling forever.
  // Empty initial value → the first frame always reads as changed → reheats.
  lastTopologyNodeSig = "";
  lastTopologyEdgeSig = "";
  /** Latest classified wire format keyed by topic key_expr — sourced
   *  from EdgeView.format on the most recent snapshot. The side-panel
   *  reads this to render per-topic format pills. */
  topicFormat = new Map();
  /** Origin of the goat-zenoh-portal — set from the periodic /api/legend
   *  refresh so the side-panel can link out to `/schema.html?key=<t>`.
   *  Mirrors `siblings.portal` for backwards-compat with callers that
   *  used `getPortalBaseUrl()` directly before Block 94. */
  portalBaseUrl = "";
  /** Sibling-surface base URLs (Block 94 cross-surface deep-link
   *  contract; see docs/design/zenoh-surface-deep-links.md). Drives
   *  the side-panel diagnose/tail/schema-page outbound buttons. */
  siblings = {};

  constructor(svgEl, options = {}) {
    this.options = options;
    this.svg = d3.select(svgEl);
    this.viewport = this.svg.append("g").attr("class", "viewport");
    // Z-order (bottom → top): cluster hulls, hull labels, edges,
    // packet dots, nodes. Lasso rect sits above everything (in svg
    // overlay, not transformed by zoom).
    this.hullGroup = this.viewport.append("g").attr("class", "hulls");
    this.hullLabelGroup = this.viewport.append("g").attr("class", "hull-labels");
    this.edgeGroup = this.viewport.append("g").attr("class", "edges");
    this.dotGroup = this.viewport.append("g").attr("class", "dots");
    this.nodeGroup = this.viewport.append("g").attr("class", "nodes");
    this.lassoRect = this.svg
      .append("rect")
      .attr("class", "lasso-rect")
      .style("display", "none");

    this.simulation = d3.forceSimulation().force(
        "link",
        d3.forceLink().id((d) => d.id)
          // Longer links (140→190) give topic pins room to sit between
          // their publisher and subscribers instead of being crushed into
          // a congested center where edges hairpin back on themselves.
          .distance(190)
          .strength(0.3),
      )
      // Stronger charge repulsion (-220→-360) spaces nodes out so the
      // dense pub→topic→sub web is legible rather than a hairball.
      .force("charge", d3.forceManyBody().strength(-360))
      .force("center", d3.forceCenter(this.width / 2, this.height / 2))
      // Explicit x/y gravity around viewport center — forceCenter alone
      // doesn't actively pull, it just shifts the centroid. With few
      // nodes and no edges between them, charge-repulsion wins and
      // nodes drift toward the canvas corners. forceX/Y restores a
      // gentle pull back to center.
      .force("x", d3.forceX(this.width / 2).strength(0.04))
      .force("y", d3.forceY(this.height / 2).strength(0.04))
      // Vendor-clustering force: pull each node toward the centroid of
      // its vendor cluster. Together with the convex-hull rendering
      // below, this gives the spatial-grouping cue the Data-COP needs
      // when peer counts grow past single-digits.
      .force("cluster", this.makeVendorClusterForce(0.08))
      // Selection-focus force: when the operator selects a node (or
      // lassos a set), pull selected + 1-hop neighbors into the center
      // and push the rest of the graph out to a peripheral ring so the
      // structure of the selected subgraph becomes legible. No-op when
      // selection is empty.
      .force("focus", this.makeSelectionFocusForce())
      .force(
        "collide",
        d3.forceCollide().radius((d) => nodeRadius(d) + 10),
      )
      .on("tick", () => this.onTick());

    this.svg.call(
      d3.zoom().filter((ev) => {
          // Ignore zoom drags initiated with shift held — those start
          // the lasso-rect selection (mousedown handler below).
          if (ev.type === "mousedown" && ev.shiftKey) return false;
          return !ev.ctrlKey && !ev.button;
        })
        .on("zoom", (ev) => {
          this.viewport.attr("transform", ev.transform.toString());
        }),
    );
    // Reclaim double-click for the fold gesture. d3.zoom binds
    // `dblclick.zoom` (zoom-in on double-click) by default; that would
    // race the node/hull double-click fold-toggle and zoom the canvas
    // when an operator double-clicks near a node. Operators zoom with
    // scroll / pinch; double-click is exclusively fold/unfold.
    this.svg.on("dblclick.zoom", null);

    // Click on empty canvas deselects everything; shift-drag begins
    // lasso-rect selection.
    this.installCanvasInteractions();
    this.installResize();
  }

  /** Public: set selection from outside (e.g. search-overlay commits). */
  setSelection(ids, options = {}) {
    if (!options.append) this.selectedIds.clear();
    for (const id of ids) this.selectedIds.add(id);
    this.refreshSelectionVisuals();
  }

  /** Public: clear selection. */
  clearSelection() {
    this.selectedIds.clear();
    this.refreshSelectionVisuals();
  }

  /** Public: get current selection ids. */
  getSelection() {
    return Array.from(this.selectedIds);
  }

  /** Public: all node IDs known to the renderer. */
  allNodeIds() {
    return this.nodes.slice();
  }

  /** Public: feed the adaptive packet-dot throttle. main.ts pushes the
   *  view's publish rate (`totals.msg_per_sec`) here. */
  setFleetMsgPerSec(rate) {
    this.fleetMsgPerSec = rate;
  }

  /** Public: topics this peer CONSUMES, based on the last snapshot's
   *  publisher→subscriber edges. Used by the side-panel stream view
   *  so a sink peer (or any subscriber) can show the inbound data
   *  flow, not just outbound publishes. */
  consumedTopicsFor(peerId) {
    const consumed = new Set();
    const labels = new Map();
    for (const t of this.lastView?.topics ?? []) labels.set(t.id, t.key);
    for (const g of this.lastView?.groups ?? []) labels.set(g.id, `${g.label} (${g.topic_count} topics)`);
    for (const e of this.lastView?.edges ?? []) {
      if (e.kind === "subscribe" && e.to === peerId) consumed.add(labels.get(e.from) ?? e.from);
    }
    return Array.from(consumed);
  }

  /** Public: latest wire format observed on the given topic (e.g.
   *  "json" / "protobuf" / "cbor" / "text" / "hex" / "empty"), or null
   *  if no sample has landed since startup. Source: EdgeView.format
   *  from the most recent snapshot. */
  formatForTopic(topic) {
    return this.topicFormat.get(topic) ?? null;
  }

  /** Public: set the portal origin (e.g.
   *  `https://portal.nova.netbird.nova.example`). main.ts pushes
   *  this from /api/legend; the side-panel uses it to build links to
   *  the portal's `/schema.html?key=<topic>` viewer.
   *
   *  **Backwards-compat shim:** new callers should pass the full
   *  sibling set via `setSiblings()`; this method keeps existing tests
   *  + the legacy `portal_base_url` field on /api/legend working
   *  without a flag-day rewrite. */
  setPortalBaseUrl(u) {
    this.portalBaseUrl = u;
    // Mirror into the siblings struct unless an explicit
    // siblings.portal override has already landed.
    if (!this.siblings.portal && u) {
      this.siblings = { ...this.siblings, portal: u };
    }
  }

  /** Public: portal origin (empty string when unconfigured). */
  getPortalBaseUrl() {
    return this.siblings.portal ?? this.portalBaseUrl;
  }

  /** Public: set the full sibling-surface URL set (Block 94 cross-
   *  surface deep-link contract). Composes with `setPortalBaseUrl` —
   *  the portal field here wins over the legacy mirror. */
  setSiblings(s) {
    this.siblings = { ...s };
    if (s.portal) this.portalBaseUrl = s.portal;
  }

  /** Public: read the current sibling-URL set. Used by side-panel
   *  renderers to construct outbound diagnose/tail/schema links. */
  getSiblings() {
    return this.siblings;
  }

  /** Public: tier-policy gate (perf.ts POLICY). When the browser frame
   *  rate degrades, packet-dot animation is disabled entirely — the
   *  edge-thickness EMA still encodes flow rate without per-event DOM
   *  churn. */
  packetDotsAllowed = true;
  setPacketDotsAllowed(allowed) {
    this.packetDotsAllowed = allowed;
  }

  /** Public: pause / resume the D3 force-layout engine. In DEGRADED
   *  tier the layout is frozen so the canvas stops re-laying-out every
   *  tick — nodes hold their last positions; pan/zoom still works. */
  setForceSimEnabled(enabled) {
    if (enabled) {
      // Re-HEAT on re-enable, not just resume. The tier gate can `stop()` the
      // sim mid-layout on a cold-load frame spike: the initial snapshot's
      // alpha(0.6) heat is itself what spikes frames while ~130 nodes spread
      // from their seeded phyllotaxis spiral, so the perf monitor's first
      // (~2s) evaluation demotes → stop() freezes the nodes half-spread. When
      // frames recover and the tier re-enables, alphaTarget(0).restart() would
      // resume from the near-zero leftover alpha and never finish spreading —
      // the layout stays piled until a node click's reheat injects energy (the
      // reported "only behaves after I click one"). Bump alpha so the
      // interrupted layout completes on its own. Max() so a still-warm sim
      // isn't cooled.
      this.simulation.alpha(Math.max(this.simulation.alpha(), 0.3)).restart();
    } else {
      this.simulation.stop();
    }
  }

  /** Public: AUSTERE-tier visual strip. When on, the SVG carries class
   *  `austere-mode`; CSS hides all unselected edges + hull fills + the
   *  packet-dot layer so what remains is the topology skeleton + the
   *  selected slice (if any). Pairs with POLICY.austere (packetDots off,
   *  forceSim off, 30s polling) — the policy gates compute, this gate
   *  preserves "what's left is the most important + true + live"
   *  legibility instead of leaving a busy-but-frozen scene on screen. */
  austereMode = false;
  setAustereMode(on) {
    if (this.austereMode === on) return;
    this.austereMode = on;
    this.svg.classed("austere-mode", on);
  }

  /** Public: predicate for the watchful-tier explanatory badge in
   *  main.ts. True when the fleet-rate gate inside emitPacket would
   *  hide ALL packet-dots because aggregate rate exceeds the
   *  comfortable threshold AND no edge is selected to whitelist. Lets
   *  main.ts surface "WATCHFUL — flow shown on selection only" instead
   *  of letting the operator stare at an apparently-frozen viz that is
   *  actually degrading gracefully. */
  isWatchfulWithoutSelection() {
    if (!this.packetDotsAllowed) return false; // browser-tier already explains it
    if (this.fleetMsgPerSec <= 50) return false;
    if (this.fleetMsgPerSec > 200) return false;
    return this.selectedIds.size === 0;
  }

  /** Public: predicate for the SATURATED badge in main.ts. True when the
   *  fleet-rate gate is hiding ALL packet-dots because aggregate exceeds
   *  the hostile threshold (>200 msg/s) — at which point edge-thickness
   *  EMA carries the load and there's nothing the operator can select to
   *  bring dots back. Distinct from WATCHFUL (50-200, select-to-see):
   *  here the right message is "the fabric is genuinely this busy," so a
   *  high-rate fabric never reads as a frozen/broken viz. (This is the
   *  signal that was missing when a duplicate-edge bug inflated the rate
   *  sum and silently crossed this threshold.) */
  isRateSaturated() {
    if (!this.packetDotsAllowed) return false; // browser-tier already explains it
    return this.fleetMsgPerSec > 200;
  }

  /** Current fleet rate the dot-gate sees — for the SATURATED badge text. */
  currentFleetMsgPerSec() {
    return this.fleetMsgPerSec;
  }

  /** Public: connect gestures to the page's ViewClient. */
  setViewIntents(intents) {
    this.intents = intents;
  }

  /** Public: the test-data mode of the parameters behind the current view. */
  getTestFilter() {
    return this.viewParams?.test ?? "hide";
  }

  /** Public: toggle the fold state of any node — the double-click gesture and
   *  the side-panel fold button. A collapsed organisation opens; a peer folds
   *  or unfolds; a folded peer's single unit returns to the peer; any other
   *  group opens on the graph one level down, or — when it is too large to
   *  draw — opens the inspector that lists it.
   *  Topic pins and infrastructure are not foldable. */
  toggleFold(d) {
    if (d.is_vendor_super && d.vendor) {
      this.intents?.toggleOrg(d.vendor);
      return;
    }
    if (d.is_singleton_super) {
      const peer = d.singleton_source_peer ?? "";
      const g = this.groupById(d.id);
      if (peer && this.viewParams?.fold_peers.includes(peer)) {
        this.selectWhenDrawn((n) => n.id === peer);
        this.intents?.togglePeerFold(peer);
      } else if (g && this.lastView && (g.kind !== "family" || g.topic_count <= this.lastView.limits.max_open_units)) {
        this.intents?.openGroup(d.id);
      } else {
        // Too large to draw: the inspector lists it instead.
        this.options.onNodeClick?.(d);
      }
      return;
    }
    if (d.role === "topic" || d.node_class === "infra") return;
    const peer = d.id;
    // Folding selects the peer's group, so the canvas re-centres on it.
    if (!this.viewParams?.fold_peers.includes(peer)) {
      this.selectWhenDrawn((n) => !!n.is_singleton_super && n.singleton_source_peer === peer);
    }
    this.intents?.togglePeerFold(peer);
  }

  selectWhenDrawn(match) {
    this.pendingSelect = { match, until: Date.now() + PENDING_SELECT_MS };
  }

  /** Public: collapse one organisation into a single node. Idempotent. */
  collapseVendor(vendor) {
    if (!vendor || this.collapsedVendors().includes(vendor)) return;
    this.selectWhenDrawn((n) => !!n.is_vendor_super && n.vendor === vendor);
    this.intents?.toggleOrg(vendor);
  }

  /** Public: expand a collapsed organisation. Idempotent. */
  expandVendor(vendor) {
    if (this.collapsedVendors().includes(vendor)) this.intents?.toggleOrg(vendor);
  }

  toggleVendorCollapse(vendor) {
    this.intents?.toggleOrg(vendor);
  }

  /** Public: COLLAPSE ALL — every organisation becomes one node. */
  collapseAllVendors() {
    this.intents?.collapseAllOrgs();
  }

  /** Public: EXPAND ALL — no collapsed organisations, folded peers or opened
   *  groups: the view the server groups by default. */
  expandAllVendors() {
    this.intents?.expandAll();
  }

  /** Public: draw only what the selected elements are connected to, and hide
   *  the rest. The component is the graph-theoretic one: every element
   *  reachable through publish and subscribe edges, in either direction.
   *  Called again while focused, it re-anchors on the current selection.
   *  Returns false when there is nothing selected to anchor on. */
  focusComponent(anchorIds) {
    const anchors = (anchorIds && anchorIds.length > 0 ? anchorIds : this.getSelection()).filter((id) =>
      this.nodes.some((n) => n.id === id),
    );
    if (anchors.length === 0) return false;
    this.focusAnchors = anchors;
    this.redrawCurrentView();
    // Selecting the whole component is the other half of the ask: the side
    // panel, the labels and the packet-dot throttle all key off selection.
    const members = this.nodes.map((n) => n.id);
    this.setSelection(members.length > 0 ? members : anchors);
    return true;
  }

  /** Public: leave component isolation and draw the whole view again. */
  exitFocus() {
    if (!this.focusAnchors) return;
    const anchors = this.focusAnchors;
    this.focusAnchors = null;
    this.focusHidden = 0;
    this.redrawCurrentView();
    this.setSelection(anchors.filter((id) => this.nodes.some((n) => n.id === id)));
  }

  /** Public: what isolation is showing, for the banner. Null when off. */
  focusInfo() {
    if (!this.focusAnchors) return null;
    const first = this.nodes.find((n) => n.id === this.focusAnchors[0]);
    const label = first ? labelOf(first) || shortId(first.id) : shortId(this.focusAnchors[0]);
    return {
      anchors: [...this.focusAnchors],
      label: this.focusAnchors.length > 1 ? `${label} +${this.focusAnchors.length - 1}` : label,
      shown: this.nodes.length,
      hidden: this.focusHidden,
    };
  }

  /** Re-run the apply for the view already on screen (a local lens changed,
   *  not the data). No-op before the first view. */
  redrawCurrentView() {
    if (this.lastView && this.viewParams) this.setView(this.lastView, this.viewParams);
  }

  /** Cut `nodes`/`edges` down to the component containing the anchors. The
   *  graph is undirected here: a subscriber is connected to what it hears. */
  applyComponentFocus() {
    if (!this.focusAnchors) {
      this.focusHidden = 0;
      return;
    }
    const present = this.focusAnchors.filter((id) => this.nodes.some((n) => n.id === id));
    if (present.length === 0) {
      // Everything it was anchored on has left the view; isolation would hide
      // the whole canvas, which is worse than dropping the lens.
      this.focusAnchors = null;
      this.focusHidden = 0;
      return;
    }
    this.focusAnchors = present;
    const adjacency = new Map();
    const link = (a, b) => {
      const list = adjacency.get(a);
      if (list) list.push(b);
      else adjacency.set(a, [b]);
    };
    for (const e of this.edges) {
      const a = nodeId(e.source), b = nodeId(e.target);
      link(a, b);
      link(b, a);
    }
    const members = new Set(present);
    const queue = [...present];
    while (queue.length > 0) {
      const cur = queue.pop();
      for (const next of adjacency.get(cur) ?? []) {
        if (!members.has(next)) {
          members.add(next);
          queue.push(next);
        }
      }
    }
    const before = this.nodes.length;
    this.nodes = this.nodes.filter((n) => members.has(n.id));
    this.edges = this.edges.filter((e) => members.has(nodeId(e.source)) && members.has(nodeId(e.target)));
    this.focusHidden = before - this.nodes.length;
  }

  /** Public: the last view drawn, for the HUD, inspector and search. */
  currentView() {
    return this.lastView;
  }

  /** Public: one group of the current view by id. */
  groupById(id) {
    return this.lastView?.groups.find((g) => g.id === id);
  }

  /** Public: organisations collapsed in the current view. */
  collapsedVendors() {
    return (this.lastView?.orgs ?? []).map((o) => o.org);
  }

  /** Public: open a group on the canvas (inspector "show on canvas"). */
  openGroup(groupId) {
    this.intents?.openGroup(groupId);
  }

  /** Public: make a topic its own pin and select it — the search overlay's
   *  commit. The server says which groups to open; the pin is selected when
   *  the view that draws it arrives. Resolves false when the server cannot
   *  draw the topic on its own (not visible, or its group is too large). */
  async revealTopic(topicKey, publisher) {
    const id = `topic:${topicKey}`;
    if (this.nodes.some((n) => n.id === id)) {
      this.setSelection([id]);
      return true;
    }
    const match = (n) => n.id === id;
    this.selectWhenDrawn(match);
    const got = (await this.intents?.reveal(topicKey, publisher).catch(() => null)) ?? null;
    if (got === null && this.pendingSelect?.match === match) this.pendingSelect = null;
    return got !== null;
  }

  /** Public: unfold a folded peer. */
  manuallyExpandNode(peerId) {
    if (this.viewParams?.fold_peers.includes(peerId)) this.intents?.togglePeerFold(peerId);
  }

  /** Selection-focus force: reorganizes layout around the current
   *  selection. Three concentric regions sized off the smaller viewport
   *  dimension:
   *
   *    selected   → pulled hard toward viewport center
   *    neighbors  → pulled toward an inner ring (~22% of min dim)
   *    others     → pushed toward an outer ring (~50% of min dim) AND
   *                 given a small radial-outward kick proportional to the
   *                 inverse of their distance from center (anti-gravity)
   *
   *  The dual-mechanism for "others" — ring-pull plus anti-gravity —
   *  makes the selected subgraph visibly recede the unrelated mass to
   *  the periphery instead of just leaving it as a backdrop. No-op when
   *  selection is empty. */
  makeSelectionFocusForce() {
    let nodesRef = [];
    const force = ((alpha) => {
      if (this.selectedIds.size === 0) return;
      const cx = this.width / 2;
      const cy = this.height / 2;
      const minDim = Math.min(this.width, this.height);
      const ringInner = minDim * 0.20;
      const ringOuter = minDim * 0.50;
      // Strengths chosen so the simulation settles within ~1s of selection
      // change without visible "snap" — alpha already decays the impulse.
      const STR_SELECTED = 0.35;
      const STR_NEIGHBOR = 0.22;
      const STR_OTHER_RING = 0.16;     // pull toward outer ring
      const STR_OTHER_REPEL = 60_000;   // anti-gravity coefficient
      for (const n of nodesRef) {
        if (n.x === undefined || n.y === undefined) continue;
        const dx = n.x - cx;
        const dy = n.y - cy;
        const dist = Math.sqrt(dx * dx + dy * dy) || 0.01;
        const isSelected = this.selectedIds.has(n.id);
        const isNeighbor = !isSelected && this.neighborIds.has(n.id);
        if (isSelected) {
          // Pull toward center proportional to current distance.
          n.vx = (n.vx ?? 0) - dx * STR_SELECTED * alpha;
          n.vy = (n.vy ?? 0) - dy * STR_SELECTED * alpha;
        } else if (isNeighbor) {
          // Pull toward the inner-ring radius along the current bearing.
          const tx = cx + (dx / dist) * ringInner;
          const ty = cy + (dy / dist) * ringInner;
          n.vx = (n.vx ?? 0) + (tx - n.x) * STR_NEIGHBOR * alpha;
          n.vy = (n.vy ?? 0) + (ty - n.y) * STR_NEIGHBOR * alpha;
        } else {
          // Unrelated nodes: pull toward outer ring (always, not just when
          // inside it — so they actively recede from center) PLUS an
          // inverse-distance anti-gravity kick outward from center. Two
          // mechanisms compose: the ring sets a comfortable terminal
          // orbit, the anti-gravity makes the "getting there" feel like
          // motion away from the focus.
          const tx = cx + (dx / dist) * ringOuter;
          const ty = cy + (dy / dist) * ringOuter;
          n.vx = (n.vx ?? 0) + (tx - n.x) * STR_OTHER_RING * alpha;
          n.vy = (n.vy ?? 0) + (ty - n.y) * STR_OTHER_RING * alpha;
          const repel = (STR_OTHER_REPEL / (dist * dist)) * alpha;
          n.vx = (n.vx ?? 0) + (dx / dist) * repel;
          n.vy = (n.vy ?? 0) + (dy / dist) * repel;
        }
      }
    });
    force.initialize = (nodes) => {
      nodesRef = nodes;
    };
    return force;
  }

  /** Vendor-cluster force: each node accelerates toward its vendor
   *  centroid. Centroids are recomputed every tick from current
   *  positions — cheap because vendor counts are tiny in v1. */
  makeVendorClusterForce(strength) {
    let nodesRef = [];
    const force = ((alpha) => {
      // Recompute centroid for each vendor.
      const centroidByVendor = new Map();
      for (const n of nodesRef) {
        if (n.x === undefined || n.y === undefined) continue;
        // Topic pins are positioned by their pub/sub links, not pulled to a
        // uuid centroid (see computeClusters) — keeps them next to their
        // publisher instead of hairpinning to a far uuid-cluster.
        if (n.id.startsWith("topic:")) continue;
        const v = vendorOf(n);
        const c = centroidByVendor.get(v) ?? { x: 0, y: 0, count: 0 };
        c.x += n.x;
        c.y += n.y;
        c.count += 1;
        centroidByVendor.set(v, c);
      }
      for (const c of centroidByVendor.values()) {
        if (c.count > 0) {
          c.x /= c.count;
          c.y /= c.count;
        }
      }
      for (const n of nodesRef) {
        if (n.x === undefined || n.y === undefined) continue;
        const v = vendorOf(n);
        const c = centroidByVendor.get(v);
        if (!c) continue;
        n.vx = (n.vx ?? 0) + (c.x - n.x) * strength * alpha;
        n.vy = (n.vy ?? 0) + (c.y - n.y) * strength * alpha;
      }
    });
    force.initialize = (nodes) => {
      nodesRef = nodes;
    };
    return force;
  }

  installCanvasInteractions() {
    const svgNode = this.svg.node();
    let lassoStart = null;

    this.svg.on("mousedown", (ev) => {
      // Shift-drag = lasso rect selection on empty canvas.
      if (!ev.shiftKey) return;
      const target = ev.target;
      if (target.closest("g.node") || target.closest("path.hull")) return;
      const pt = clientToSvgPoint(svgNode, ev.clientX, ev.clientY);
      lassoStart = pt;
      this.lassoRect
        .style("display", "")
        .attr("x", pt.x)
        .attr("y", pt.y)
        .attr("width", 0)
        .attr("height", 0);
      ev.preventDefault();
    });

    this.svg.on("mousemove", (ev) => {
      if (!lassoStart) return;
      const pt = clientToSvgPoint(svgNode, ev.clientX, ev.clientY);
      const x = Math.min(pt.x, lassoStart.x);
      const y = Math.min(pt.y, lassoStart.y);
      const w = Math.abs(pt.x - lassoStart.x);
      const h = Math.abs(pt.y - lassoStart.y);
      this.lassoRect.attr("x", x).attr("y", y).attr("width", w).attr("height", h);
    });

    this.svg.on("mouseup", (ev) => {
      if (!lassoStart) {
        // Plain click on empty canvas → clear selection.
        const target = ev.target;
        if (!target.closest("g.node") && !target.closest("path.hull")) {
          this.clearSelection();
        }
        return;
      }
      // Lasso commit — pick all nodes inside the rect (transformed
      // back through the zoom-viewport transform).
      const x1 = parseFloat(this.lassoRect.attr("x"));
      const y1 = parseFloat(this.lassoRect.attr("y"));
      const w = parseFloat(this.lassoRect.attr("width"));
      const h = parseFloat(this.lassoRect.attr("height"));
      const x2 = x1 + w, y2 = y1 + h;
      // Apply inverse of current zoom transform to convert SVG-space
      // box → viewport-space box so we can test against node x/y
      // (which live in viewport coords).
      const transform = d3.zoomTransform(svgNode);
      const inv = (px, py) => {
        const [vx, vy] = transform.invert([px, py]);
        return { x: vx, y: vy };
      };
      const a = inv(x1, y1), b = inv(x2, y2);
      const bx1 = Math.min(a.x, b.x), bx2 = Math.max(a.x, b.x);
      const by1 = Math.min(a.y, b.y), by2 = Math.max(a.y, b.y);
      const hits = this.nodes
        .filter((n) =>
          n.x !== undefined && n.y !== undefined &&
          n.x >= bx1 && n.x <= bx2 && n.y >= by1 && n.y <= by2,
        )
        .map((n) => n.id);
      if (hits.length > 0) {
        this.setSelection(hits, { append: ev.metaKey || ev.ctrlKey });
        // Tell the host so the side panel updates.
        if (this.options.onNodeClick && this.nodes.find((n) => n.id === hits[0])) {
          this.options.onNodeClick(this.nodes.find((n) => n.id === hits[0]));
        }
      } else if (!(ev.metaKey || ev.ctrlKey)) {
        this.clearSelection();
      }
      this.lassoRect.style("display", "none");
      lassoStart = null;
    });
  }

  refreshSelectionVisuals(reheat = true) {
    // `reheat` controls the selection-focus reheat at the tail. A genuine
    // selection change (setSelection / clearSelection / modifier-click-out)
    // wants it so the focus force visibly rearranges the graph; a routine
    // snapshot repaint (applySnapshot, ~1.5s poll) passes false so a stable
    // topology is left to settle instead of re-heating every poll (F-210
    // follow-on — this was the SECOND unconditional reheat alongside the
    // applySnapshot alpha(0.6), and the one the layout-settle gate didn't cover).
    // Compute 2-hop neighbors of the selection for graph-walkout
    // visual hint. The broker-pattern edge split (publisher → topic →
    // subscriber) means 1-hop from a selected source peer reaches
    // ONLY the topic-broker pins between it and any subscriber — the
    // actual subscriber peers stay dimmed at 1-hop. 2-hop crosses the
    // topic pin and surfaces the downstream consumers (derivers + sinks
    // for a sensor selection; sources + topics for a sink selection).
    // BFS from selection out to depth 2; cap there to keep the
    // walkout focused (3-hop in v1 would reach >50% of the fabric on
    // the omniwatch hub).
    this.neighborIds.clear();
    if (this.selectedIds.size > 0) {
      const visited = new Set(this.selectedIds);
      let frontier = new Set(this.selectedIds);
      const MAX_HOPS = 2;
      for (let hop = 0; hop < MAX_HOPS; hop++) {
        const next = new Set();
        for (const e of this.edges) {
          const s = nodeId(e.source), t = nodeId(e.target);
          if (frontier.has(s) && !visited.has(t)) {
            next.add(t);
            visited.add(t);
          }
          if (frontier.has(t) && !visited.has(s)) {
            next.add(s);
            visited.add(s);
          }
        }
        for (const id of next) this.neighborIds.add(id);
        frontier = next;
        if (frontier.size === 0) break;
      }
    }

    const hasSelection = this.selectedIds.size > 0;
    this.nodeGroup.selectAll("g.node").classed("selected", (d) => this.selectedIds.has(d.id))
      .classed("neighbor", (d) => this.neighborIds.has(d.id))
      .classed("universal-observer", (d) => d.universal_observer === true)
      // History-lens aged marker: a peer within the selected window but not
      // currently live renders faded, so live vs historical is unmistakable
      // (docs/design/panoscope-history-lens.md). Topic/super nodes carry
      // last_seen_ms=0 and are never aged.
      .classed("aged", (d) => isAgedNode(d))
      .classed("dimmed", (d) =>
        hasSelection && !this.selectedIds.has(d.id) && !this.neighborIds.has(d.id));
    this.edgeGroup.selectAll("line.edge")
      .classed("selected", (d) =>
        this.selectedIds.has(nodeId(d.source)) || this.selectedIds.has(nodeId(d.target)))
      .classed("dimmed", (d) =>
        hasSelection &&
        !this.selectedIds.has(nodeId(d.source)) &&
        !this.selectedIds.has(nodeId(d.target)) &&
        !this.neighborIds.has(nodeId(d.source)) &&
        !this.neighborIds.has(nodeId(d.target)));
    this.hullGroup.selectAll("path.hull")
      .classed("dimmed", (d) =>
        hasSelection && !d.ids.some((id) => this.selectedIds.has(id)));
    if (this.options.onSelectionChange) {
      this.options.onSelectionChange(Array.from(this.selectedIds));
    }
    // Reheat the simulation so the selection-focus force visibly
    // rearranges the graph around the new selection. Same alpha as a
    // fresh snapshot — settles in roughly 1-2 seconds without jarring.
    // Skipped on a routine snapshot repaint (reheat=false) so a stable
    // topology settles instead of churning every poll.
    if (reheat) this.simulation.alpha(0.55).restart();
  }

  installResize() {
    const apply = () => {
      const rect = this.svg.node().getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) return;
      this.width = rect.width;
      this.height = rect.height;
      this.simulation.force(
        "center",
        d3.forceCenter(this.width / 2, this.height / 2),
      );
      this.simulation.force("x", d3.forceX(this.width / 2).strength(0.06))
        .force("y", d3.forceY(this.height / 2).strength(0.06));
      this.simulation.alpha(0.3).restart();
    };
    apply();
    window.addEventListener("resize", apply);
    // ResizeObserver catches layout changes that don't fire window
    // resize — e.g. the info-panel opening into a split layout takes
    // half the viewport away from the graph; without this the graph's
    // forceCenter stays calibrated for the old full-width canvas and
    // nodes drift to the left edge.
    if (typeof ResizeObserver !== "undefined") {
      const ro = new ResizeObserver(() => apply());
      ro.observe(this.svg.node());
    }
  }

  /**
   * Public view entry — the single render chokepoint. Draws a /api/view
   * response (docs/design/panoscope-view-model.md): the server has already
   * applied scope, window, filters, grouping and the element cap, so this maps
   * elements to nodes and edges and never folds anything itself.
   *
   * Wrapped in the LAST-GOOD-FRAME guard (docs/findings/F-210): if the apply
   * throws, the previously committed frame is restored and repainted and the
   * error re-thrown for the banner — the surface degrades, it never blanks.
   */
  setView(view, params) {
    const prevNodes = this.nodes;
    const prevEdges = this.edges;
    try {
      this.applyView(view, params);
    } catch (e) {
      this.nodes = prevNodes;
      this.edges = prevEdges;
      try {
        this.simulation.nodes(this.nodes);
        this.simulation.force("link").links(this.edges);
        this.render();
      } catch {
        // The previous frame was valid when committed; if its repaint throws
        // too there is nothing safe to draw.
      }
      throw e;
    }
  }

  applyView(view, params) {
    this.lastView = view;
    this.viewParams = params;
    this.topicFormat.clear();
    for (const t of view.topics) {
      if (t.format) this.topicFormat.set(t.key, t.format);
    }

    // Keep each surviving element's position so a refresh does not reshuffle.
    const nodeById = new Map(this.nodes.map((n) => [n.id, n]));
    const place = (id) => {
      const e = nodeById.get(id);
      return { x: e?.x, y: e?.y, vx: e?.vx, vy: e?.vy };
    };

    const publishedEvents = new Map();
    for (const e of view.edges) {
      if (e.kind === "publish") publishedEvents.set(e.from, (publishedEvents.get(e.from) ?? 0) + e.total_events);
    }
    const groupsOf = new Map();
    for (const g of view.groups) {
      const list = groupsOf.get(g.publisher);
      if (list) list.push(g);
      else groupsOf.set(g.publisher, [g]);
    }
    const vendorOfPeer = new Map(view.peers.map((p) => [p.peer_id, p.vendor]));
    const nameOfPeer = new Map(
      view.peers.map((p) => [p.peer_id, p.slot_alias ?? p.display_name ?? shortId(p.peer_id)]),
    );

    const nodes = [];
    for (const p of view.peers) {
      nodes.push({
        id: p.peer_id,
        slot_alias: p.slot_alias,
        display_name: p.display_name,
        vendor: p.vendor,
        role: p.role,
        role_class: p.role_class,
        site: p.site,
        node_kind: p.node_kind,
        node_class: p.node_class,
        node_subkind: p.node_subkind,
        universal_observer: p.universal_observer,
        hygiene: p.hygiene,
        publishes: p.publishes,
        subscribes: p.subscribes,
        total_events: publishedEvents.get(p.peer_id) ?? 0,
        first_seen_ms: p.first_seen_ms,
        last_seen_ms: p.last_seen_ms,
        synthetic: p.synthetic,
        has_collapsed_singletons: groupsOf.has(p.peer_id),
        manually_collapsed: p.folded,
        ...place(p.peer_id),
      });
    }
    for (const o of view.orgs) {
      nodes.push({
        id: o.id,
        display_name: o.org.toUpperCase(),
        vendor: o.org,
        role: "deriver",
        publishes: [],
        subscribes: [],
        total_events: 0,
        first_seen_ms: 0,
        last_seen_ms: 0,
        is_vendor_super: true,
        super_peer_count: o.peer_count,
        super_topic_count: o.topic_count,
        ...place(o.id),
      });
    }
    for (const t of view.topics) {
      nodes.push({
        id: t.id,
        vendor: null,
        role: "topic",
        publishes: [],
        subscribes: [t.key],
        total_events: t.total_events,
        first_seen_ms: 0,
        last_seen_ms: t.last_event_ms,
        ...place(t.id),
      });
    }
    for (const g of view.groups) {
      nodes.push({
        id: g.id,
        // The publisher's name for a whole-publisher group (its label is
        // `**`); the pattern otherwise.
        display_name: g.kind === "publisher" ? (nameOfPeer.get(g.publisher) ?? null) : g.label,
        vendor: vendorOfPeer.get(g.publisher) ?? null,
        role: "topic",
        publishes: [],
        subscribes: [],
        total_events: g.total_events,
        first_seen_ms: 0,
        last_seen_ms: g.last_event_ms,
        is_singleton_super: true,
        singleton_source_peer: g.publisher,
        singleton_namespace: g.label,
        singleton_count: g.topic_count,
        group_kind: g.kind,
        ...place(g.id),
      });
    }
    this.nodes = nodes;

    const labelOf = new Map();
    for (const t of view.topics) labelOf.set(t.id, t.key);
    for (const g of view.groups) labelOf.set(g.id, g.label);
    this.edges = view.edges.map((e) => ({
      source: e.from,
      target: e.to,
      topic: labelOf.get(e.kind === "publish" ? e.to : e.from) ?? "",
      ema_eps: e.projected_eps,
      projected_eps: e.projected_eps,
      total_events: e.total_events,
      schema_family: null,
      last_event_ms: e.last_event_ms,
      edge_kind: e.kind,
    }));

    // Packet-dot index: a live publish attributes to its pin, else to the
    // publisher's group whose selector matches.
    this.pinEdgeIndex.clear();
    this.groupEdgeIndex.clear();
    const publishEdge = new Map();
    for (const e of this.edges) {
      if (e.edge_kind === "publish") publishEdge.set(`${e.source}|${e.target}`, e);
    }
    for (const t of view.topics) {
      for (const pub of t.publishers) {
        const edge = publishEdge.get(`${pub}|${t.id}`);
        if (edge) this.pinEdgeIndex.set(`${pub}|${t.key}`, edge);
      }
    }
    for (const g of view.groups) {
      const edge = publishEdge.get(`${g.publisher}|${g.id}`);
      if (!edge) continue;
      const list = this.groupEdgeIndex.get(g.publisher) ?? [];
      list.push({ selector: g.selector.split("/"), edge });
      this.groupEdgeIndex.set(g.publisher, list);
    }

    // Graceful-degradation guard (see src/edge-guard.ts + docs/findings/F-210):
    // d3-force's forceLink.initialize() throws a synchronous "node not found:
    // <id>" the instant any edge references a source/target id that isn't in
    // the node set. That throw propagates out of onSnapshot, ws.ts catches it
    // as "Handler error on snapshot", and the ENTIRE render is aborted — one
    // dangling edge blanks the whole canvas (0/0 peers). Drop the unresolvable
    // edges and keep drawing the rest, rather than failing open-to-blank.
    const nodeIdSet = new Set(this.nodes.map((n) => n.id));
    const { kept, dropped, missingIds } = partitionDanglingEdges(
      nodeIdSet,
      this.edges,
    );
    if (dropped.length > 0) {
      this.edges = kept;
      console.warn(
        `panoscope: dropped ${dropped.length} edge(s) referencing ` +
          `unmaterialized node(s): ${missingIds.join(", ")}`,
      );
    }

    // Component isolation, if the operator turned it on: a local lens over
    // what the server drew, recomputed here so it tracks the live fabric.
    this.applyComponentFocus();

    // Settle gate (src/layout-settle.ts + docs/design/panoscope.md §"Layout
    // settle"): tell a real structural change (peer/topic added or removed, a
    // filter collapse/expand altering the derived set) apart from a data-only
    // refresh. Edge source/target are still plain id strings here — d3's
    // forceLink rewrites them to node objects only after .links() below — but
    // topologySignature resolves both shapes, so it's stable either way.
    const sig = topologySignature(this.nodes, this.edges);
    const changed = topologyChanged(
      { nodeSig: this.lastTopologyNodeSig, edgeSig: this.lastTopologyEdgeSig },
      sig,
    );

    this.simulation.nodes(this.nodes);
    this.simulation.force("link").links(this.edges);
    this.render();
    // reheat=false: repaint selection classes without the selection-focus
    // reheat, so a data-only poll doesn't churn the layout (the topology
    // gate below owns the only legitimate snapshot reheat).
    this.refreshSelectionVisuals(false);
    // Re-heat ONLY on a topology change. A data-only poll leaves the layout
    // alone so it can settle (F-210 follow-on: the unconditional reheat here
    // re-heated the sim to alpha 0.6 every ~1.5s, so it never cooled to
    // alphaMin — invisible until panoscope rendered again post-F-210).
    if (changed) {
      this.lastTopologyNodeSig = sig.nodeSig;
      this.lastTopologyEdgeSig = sig.edgeSig;
      this.simulation.alpha(0.6).restart();
    }
    if (this.pendingSelect) {
      const hit = this.nodes.find(this.pendingSelect.match);
      if (hit) {
        this.setSelection([hit.id]);
        this.pendingSelect = null;
      } else if (Date.now() > this.pendingSelect.until) {
        this.pendingSelect = null;
      }
    }
  }

  /**
   * Trigger a packet-dot animation along the edge from source → sink
   * for the given topic. If the edge isn't currently rendered (event
   * arrived before snapshot resync), the dot is dropped silently.
   */
  pingEdge(sourcePeer, topic) {
    const edge =
      this.pinEdgeIndex.get(`${sourcePeer}|${topic}`) ??
      this.groupEdgeIndex.get(sourcePeer)?.find((g) => selectorMatches(g.selector, topic.split("/")))?.edge;
    if (!edge) return;

    // Browser-perf tier gate first — if the device is degraded the
    // fleet-rate threshold doesn't even matter.
    if (!this.packetDotsAllowed) return;
    // Adaptive packet-dot throttle. Three tiers based on fleet
    // aggregate throughput (sum of all edge projected_eps):
    //   <50 msg/s  → comfortable: animate every edge
    //   50–200/s   → watchful: only selected+neighbor edges
    //   >200/s     → hostile: packet-dots disabled (edge thickness EMA
    //                does the work)
    // Rate fed by main.ts from the view totals.
    const fleetRate = this.fleetMsgPerSec;
    if (fleetRate > 200) return;
    if (fleetRate > 50) {
      if (this.selectedIds.size === 0) return;
      const srcId = nodeId(edge.source), tgtId = nodeId(edge.target);
      if (
        !this.selectedIds.has(srcId) &&
        !this.selectedIds.has(tgtId) &&
        !this.neighborIds.has(srcId) &&
        !this.neighborIds.has(tgtId)
      ) {
        return;
      }
    }
    // <50 msg/s: fall through and animate.

    const src = edge.source;
    const dst = edge.target;
    if (
      typeof src !== "object" ||
      typeof dst !== "object" ||
      src.x === undefined ||
      dst.x === undefined
    ) {
      return;
    }

    // Visibility tuning: radius 3 + duration 800ms left
    // dots too small + too fast to track at 30 msg/s fleet rate — the
    // operator's question "are bytes actually moving?" was hard to
    // answer at a glance. Bigger radius + longer travel + a visible
    // start-state opacity gives a clearer "packet in flight" read
    // without overwhelming the canvas at higher rates (the throttle
    // tiers above still gate aggregate dot count by selection at
    // >50 msg/s, so this purely sizes the dots that DO render).
    const dot = this.dotGroup
      .append("circle").attr("class", "packet-dot").attr("r", 5).attr("cx", src.x).attr("cy", src.y)
      .attr("fill", schemaColor(edge.schema_family))
      .style("opacity", 0.95);

    dot.transition().duration(1200).attr("cx", dst.x).attr("cy", dst.y).style("opacity", 0)
      .remove();
  }

  render() {
    // Cluster hulls first (lowest z).
    const clusters = computeClusters(this.nodes);
    const hullSel = this.hullGroup.selectAll("path.hull").data(clusters, (d) => d.vendor);
    hullSel.exit().remove();
    hullSel
      .enter()
      .append("path")
      .attr("class", (d) => isFutureCluster(d.vendor) ? "hull hull-future" : "hull")
      // Hull mirrors the node model: SINGLE click selects the vendor's
      // peers (so the side panel can offer "collapse vendor"); DOUBLE
      // click collapses the whole vendor into its super-node. The hull
      // is the vendor's expanded representation — double-click folds it,
      // double-click the resulting super-node unfolds it (symmetric).
      .on("click", (ev, d) => {
        const append = ev.metaKey || ev.ctrlKey || ev.shiftKey;
        this.setSelection(d.ids, { append });
        ev.stopPropagation();
      })
      .on("dblclick", (ev, d) => {
        ev.preventDefault();
        ev.stopPropagation();
        this.collapseVendor(d.vendor);
      });
    // Update class on existing hulls so a vendor key that changes
    // category (rare but possible — e.g. a re-classified namespace)
    // re-renders with the right styling.
    this.hullGroup.selectAll("path.hull")
      .attr("class", (d) =>
        isFutureCluster(d.vendor)
          ? `hull hull-future${this.selectedIds.size > 0 && !d.ids.some((id) => this.selectedIds.has(id)) ? " dimmed" : ""}`
          : `hull${this.selectedIds.size > 0 && !d.ids.some((id) => this.selectedIds.has(id)) ? " dimmed" : ""}`);
    const hullLabelSel = this.hullLabelGroup.selectAll("g.hull-label")
      .data(clusters, (d) => d.vendor);
    hullLabelSel.exit().remove();
    const hullLabelEnter = hullLabelSel
      .enter()
      .append("g")
      .attr("class", (d) => isFutureCluster(d.vendor) ? "hull-label hull-label-future" : "hull-label");
    hullLabelEnter
      .append("text")
      .attr("class", "hull-label-name")
      // The cluster key under a per-uuid namespace is the `<uuid>/` tenant prefix, so an
      // un-shortened hull label is a full 32-char hex string. Shorten
      // for the canvas; the side panel still carries the full vendor.
      .attr("text-anchor", "middle")
      .text((d) => shortenUuids(d.vendor).toUpperCase());
    hullLabelEnter
      .filter((d) => isFutureCluster(d.vendor))
      .append("text")
      .attr("class", "hull-label-subtitle")
      .attr("text-anchor", "middle")
      .attr("dy", "1.4em")
      .text("FUTURE DESIGN · NOT ACTIVE ON THIS FABRIC");

    // Edges.
    const edgeSel = this.edgeGroup.selectAll("line.edge")
      .data(this.edges, (d) => `${nodeId(d.source)}|${nodeId(d.target)}|${d.topic}`);

    edgeSel.exit().remove();

    const edgeEnter = edgeSel
      .enter()
      .append("line")
      .attr("class", "edge")
      .on("click", (_ev, d) => this.options.onEdgeClick?.(d));
    // Hover tooltip — title element gives native hover behaviour.
    edgeEnter.append("title");

    edgeEnter
      .merge(edgeSel)
      .attr("stroke", (d) => schemaColor(d.schema_family))
      .attr("stroke-width", (d) => edgeWidth(d))
      .attr("stroke-opacity", (d) => edgeOpacity(d))
      // Differentiate publish vs subscribe edges visually. Subscribe
      // edges (topic → sub) get a short dash pattern so direction is
      // legible without arrowheads. Publish edges stay solid.
      .attr("stroke-dasharray", (d) =>
        d.edge_kind === "subscribe"
          ? "4 3"
          : null,
      );
    this.edgeGroup.selectAll("line.edge").select("title").text((d) => edgeTooltip(d));

    // Nodes.
    const nodeSel = this.nodeGroup.selectAll("g.node").data(this.nodes, (d) => d.id);

    nodeSel.exit().remove();

    const nodeEnter = nodeSel
      .enter()
      .append("g")
      .attr("class", (d) => (d.is_vendor_super ? "node vendor-super" : "node"))
      // Unified interaction model (see docs/design/panoscope.md
      // §"Fold/unfold interaction"): a SINGLE click always SELECTS
      // (highlight + center + side panel) for every node type — peer,
      // topic, vendor-super, singletons-super alike. Folding/unfolding
      // is exclusively the DOUBLE click (toggleFold) plus the side-panel
      // button. The two gestures never overlap outcomes, so the operator
      // never has to predict "will this click select or collapse?".
      .on("click", (ev, d) => {
        const append = ev.metaKey || ev.ctrlKey || ev.shiftKey;
        if (append && this.selectedIds.has(d.id)) {
          // Modifier-click on an already-selected node → toggle it out
          // of the multi-selection.
          this.selectedIds.delete(d.id);
          this.refreshSelectionVisuals();
        } else {
          this.setSelection([d.id], { append });
        }
        // onNodeClick drives the side panel. For a group this opens the
        // group inspector (a list browser over its members, with "show on
        // canvas"); for other nodes it surfaces peer/topic detail.
        this.options.onNodeClick?.(d);
        ev.stopPropagation();
      })
      // Double click = toggle the fold state of the node under the
      // cursor. Symmetric: open a folded super-node, fold an expanded
      // peer. d3.zoom's dblclick-to-zoom is unbound above so this is the
      // sole double-click meaning on the canvas.
      .on("dblclick", (ev, d) => {
        ev.preventDefault();
        ev.stopPropagation();
        this.toggleFold(d);
      })
      .call(this.installDrag());

    // Topic nodes get a diamond <path>; peer nodes get a <circle>.
    // Both shapes get a vendor-border stroke + role-fill.
    nodeEnter
      .append("path")
      .attr("class", "node-shape");
    // Inner-dot indicator: a small darker circle in the center,
    // visibility CSS-gated on `.has-collapsed-singletons` /
    // `.manually-collapsed`. Signals "this node has hidden contents"
    // at a glance. Same visual scheme for both the auto-rule and the
    // double-click manual fold cases.
    nodeEnter
      .append("circle")
      .attr("class", "inner-dot")
      .attr("r", 3)
      .attr("cx", 0)
      .attr("cy", 0);
    // Hover-count badge for singletons-super: the folded count (+ an
    // optional "may slow browser" warning when large), revealed only
    // while the cursor is over the node and positioned just above the
    // glyph so it reads near the element's center without colliding with
    // the stacked-disc arcs, edges, or the name label below. The dashed
    // magnitude ring was removed — it collided with surrounding lines /
    // text and duplicated the count that's also in the side-panel
    // inspector.
    const haloGroup = nodeEnter
      .append("g")
      .attr("class", "peek-halo");
    haloGroup
      .append("text")
      .attr("class", "peek-halo-count")
      .attr("text-anchor", "middle")
      .attr("dy", "0.35em");
    haloGroup
      .append("text")
      .attr("class", "peek-halo-warn")
      .attr("text-anchor", "middle");
    nodeEnter.append("title");
    // Two-line label below the node — visibility CSS-gated on
    // `.selected` / `.neighbor`. Default-state nodes have no label so
    // the canvas stays uncluttered.
    nodeEnter
      .append("text")
      .attr("class", "node-label")
      .attr("text-anchor", "middle")
      .attr("dy", "1.6em");

    const merged = nodeEnter.merge(nodeSel);

    merged.select("path.node-shape").attr("d", (d) => shapeFor(d)).attr("fill", (d) => fillFor(d))
      .attr("stroke", (d) => vendorColor(vendorOf(d)))
      .attr("stroke-width", 2)
      .attr("stroke-dasharray", (d) => (isDashedNode(d) ? "4,2" : null));

    // Toggle the "has hidden contents" indicator class on the parent
    // <g> so CSS can show/hide the inner-dot. Auto-rule + manual-rule
    // both light up the same indicator.
    merged
      .classed("has-collapsed-singletons", (d) => !!d.has_collapsed_singletons)
      .classed("manually-collapsed", (d) => !!d.manually_collapsed)
      .classed("singleton-super", (d) => !!d.is_singleton_super);

    // Hover-count geometry. The count sits just above the glyph's top
    // edge (near the element's center, clear of the arcs + the name
    // label below); the warn line sits a little higher and only renders
    // when the count crosses the slow-browser threshold. Visibility is
    // CSS-driven off `:hover` (see styles.css).
    merged
      .filter((d) => !!d.is_singleton_super)
      .each(function (d) {
        const sel = d3.select(this);
        const count = d.singleton_count ?? 0;
        const r = nodeRadius(d);
        const big = d.group_kind === "family" && count > GROUP_OPEN_WARN_THRESHOLD;
        sel.select("text.peek-halo-count").attr("y", -r - 9).classed("warn", big).text(`${count}`);
        sel.select("text.peek-halo-warn").attr("y", -r - 24).text(big ? "⚠ may slow browser" : "");
      });

    merged
      .select("title")
      .text((d) => {
        if (d.role === "topic" && d.id.startsWith("topic:")) {
          return `${d.id.slice("topic:".length)} — topic`;
        }
        return `${labelOf(d)} — ${d.role}`;
      });

    // Render label as multi-line if it contains a slash (topic-prefix
    // or topic-node identities); otherwise single line. Clear existing
    // tspans first so re-renders don't accumulate.
    merged.select("text.node-label").each(function (d) {
      const sel = d3.select(this);
      sel.selectAll("tspan").remove();
      // Shorten any tenant/peer UUID run for the canvas label; the full
      // key remains in the <title> tooltip + the side panel.
      const label = shortenUuids(labelOf(d));
      const parts = label.split("/");
      const lineCount = parts.length;
      const lineHeight = 1.1;
      parts.forEach((part, i) => {
        sel
          .append("tspan")
          .attr("x", 0)
          .attr("dy", i === 0 ? `${-(lineCount - 1) * lineHeight * 0.5}em` : `${lineHeight}em`)
          .text(part);
      });
    });
  }

  installDrag() {
    return d3.drag().on("start", (ev, d) => {
        if (!ev.active) this.simulation.alphaTarget(0.3).restart();
        d.fx = d.x;
        d.fy = d.y;
      })
      .on("drag", (ev, d) => {
        d.fx = ev.x;
        d.fy = ev.y;
      })
      .on("end", (ev, d) => {
        if (!ev.active) this.simulation.alphaTarget(0);
        d.fx = null;
        d.fy = null;
      });
  }

  onTick() {
    this.edgeGroup.selectAll("line.edge").attr("x1", (d) => d.source.x)
      .attr("y1", (d) => d.source.y).attr("x2", (d) => d.target.x).attr("y2", (d) => d.target.y);

    this.nodeGroup.selectAll("g.node").attr("transform", (d) => `translate(${d.x},${d.y})`);

    // Recompute hull paths from current node positions.
    const nodesById = new Map(this.nodes.map((n) => [n.id, n]));
    this.hullGroup.selectAll("path.hull").attr("d", (d) => hullPath(d.ids, nodesById));
    this.hullLabelGroup.selectAll("g.hull-label").attr("transform", (d) => {
        const c = clusterCentroid(d.ids, nodesById);
        return c ? `translate(${c.x},${c.y - 60})` : "translate(-9999,-9999)";
      });
  }
}

// ---------- Cluster + hull helpers ----------

function computeClusters(nodes) {
  const groups = new Map();
  for (const n of nodes) {
    // Super-nodes don't belong to their own vendor's cluster — they
    // ARE the collapsed-state visual, and drawing a 1-node hull around
    // each one would add noise without information.
    if (n.is_vendor_super) continue;
    // Topic-virtual broker pins must NOT form their own hull. `vendorOf`
    // returns a topic's first key segment, which under a per-uuid
    // namespace is the tenant UUID — so every topic pin spawned a spurious
    // uuid-hull (with a uuid label) AND got pulled to that uuid centroid
    // instead of sitting next to its publisher, stretching edges across the
    // canvas and hairpinning them through a congested center. Excluded here
    // (and from the cluster force) so pins are positioned purely by their
    // pub/sub links — near the peers they connect, no spurious uuid-hulls.
    if (n.id.startsWith("topic:")) continue;
    const v = vendorOf(n);
    const arr = groups.get(v) ?? [];
    arr.push(n.id);
    groups.set(v, arr);
  }
  return Array.from(groups.entries())
    .filter(([v]) => v !== "(peer)" && v !== "?" && v !== "(orphan)")
    .map(([vendor, ids]) => ({ vendor, ids }));
}

function clusterCentroid(
  ids,
  nodesById,
) {
  let sx = 0, sy = 0, c = 0;
  for (const id of ids) {
    const n = nodesById.get(id);
    if (!n || n.x === undefined || n.y === undefined) continue;
    sx += n.x;
    sy += n.y;
    c += 1;
  }
  return c > 0 ? { x: sx / c, y: sy / c } : null;
}

/** Compute a rounded "blob" path around the cluster nodes via convex
 *  hull + radius-based smoothing. Works for 1, 2, N nodes. */
function hullPath(ids, nodesById) {
  const points = [];
  for (const id of ids) {
    const n = nodesById.get(id);
    if (!n || n.x === undefined || n.y === undefined) continue;
    points.push([n.x, n.y]);
  }
  if (points.length === 0) return "";
  const pad = 36;
  if (points.length === 1) {
    const [x, y] = points[0];
    return circlePath(x, y, pad);
  }
  if (points.length === 2) {
    // Two-node pill — bounding rounded rectangle.
    const [a, b] = points;
    const cx = (a[0] + b[0]) / 2, cy = (a[1] + b[1]) / 2;
    const dx = b[0] - a[0], dy = b[1] - a[1];
    const len = Math.sqrt(dx * dx + dy * dy);
    const r = pad;
    return circlePath(cx - dx / 2, cy - dy / 2, r) +
      " " + circlePath(cx + dx / 2, cy + dy / 2, r) +
      " " + (len > 0 ? rectAlongLine(a, b, r) : "");
  }
  const hull = d3.polygonHull(points);
  if (!hull) return "";
  // Inflate hull outward along edge normals for padding.
  const cx = hull.reduce((s, p) => s + p[0], 0) / hull.length;
  const cy = hull.reduce((s, p) => s + p[1], 0) / hull.length;
  const inflated = hull.map(([x, y]) => {
    const vx = x - cx, vy = y - cy;
    const len = Math.sqrt(vx * vx + vy * vy);
    if (len === 0) return [x, y];
    return [x + (vx / len) * pad, y + (vy / len) * pad];
  });
  // Smooth via d3 line with monotoneCurve for a rounded blob.
  const line = d3.line().curve(d3.curveCatmullRomClosed).x((d) => d[0]).y((d) => d[1]);
  return line(inflated) ?? "";
}

function circlePath(cx, cy, r) {
  return `M${cx - r},${cy} a${r},${r} 0 1,0 ${2 * r},0 a${r},${r} 0 1,0 ${-2 * r},0 Z`;
}

function rectAlongLine(a, b, r) {
  const dx = b[0] - a[0], dy = b[1] - a[1];
  const len = Math.sqrt(dx * dx + dy * dy);
  const nx = -dy / len, ny = dx / len;
  const p1 = [a[0] + nx * r, a[1] + ny * r];
  const p2 = [b[0] + nx * r, b[1] + ny * r];
  const p3 = [b[0] - nx * r, b[1] - ny * r];
  const p4 = [a[0] - nx * r, a[1] - ny * r];
  return `M${p1[0]},${p1[1]} L${p2[0]},${p2[1]} L${p3[0]},${p3[1]} L${p4[0]},${p4[1]} Z`;
}

/** Five-pointed star path centred at (0,0) with outer radius `r`. Inner
 *  radius is 0.4·r — gives a clear star silhouette without becoming a
 *  blob at typical node sizes. Drawn once per node (same cost as the
 *  hexagon path); no animation, no extra DOM. */
function starPath(r) {
  const outer = r;
  const inner = r * 0.4;
  const pts = [];
  for (let i = 0; i < 10; i++) {
    const radius = i % 2 === 0 ? outer : inner;
    // Start at the top (apex pointing up); 36° per step (10 vertices).
    const angle = -Math.PI / 2 + (i * Math.PI) / 5;
    const x = radius * Math.cos(angle);
    const y = radius * Math.sin(angle);
    pts.push(`${i === 0 ? "M" : "L"}${x.toFixed(2)},${y.toFixed(2)}`);
  }
  pts.push("Z");
  return pts.join(" ");
}

function shapeFor(n) {
  const r = nodeRadius(n);
  // Vendor-collapse super-node: chunky rounded rectangle, distinct
  // from every other glyph in the legend so "a vendor folded up"
  // reads at a glance.
  //   - Topic = diamond (small)
  //   - Router = axis-aligned square (small/medium)
  //   - Source = circle, deriver = hexagon, sink = inverted triangle
  //   - Trashgoat (any role) = five-pointed star — platform-internal
  //     chaos actor, deliberate-trash publisher; distinct from every
  //     real-peer glyph so the eye registers "synthetic / not real
  //     traffic" at a glance
  //   - Super = chunky rounded rectangle (always biggest)
  if (n.is_vendor_super) {
    const w = r * 1.6;
    const h = r * 1.0;
    const rr = 6; // corner radius
    return (
      `M${-w + rr},${-h} L${w - rr},${-h} A${rr},${rr} 0 0 1 ${w},${-h + rr}` +
      ` L${w},${h - rr} A${rr},${rr} 0 0 1 ${w - rr},${h}` +
      ` L${-w + rr},${h} A${rr},${rr} 0 0 1 ${-w},${h - rr}` +
      ` L${-w},${-h + rr} A${rr},${rr} 0 0 1 ${-w + rr},${-h} Z`
    );
  }
  // Singletons-super: stacked-disc glyph — three concentric arcs to
  // suggest "multiple things piled up." Distinct from vendor-super
  // (rectangle) and from topic-pin (diamond) so the legend stays
  // unambiguous.
  if (n.is_singleton_super) {
    const ring1 = r;
    const ring2 = r * 0.66;
    const ring3 = r * 0.36;
    const ringPath = (rad) =>
      `M${-rad},0 a${rad},${rad} 0 1,0 ${2 * rad},0 a${rad},${rad} 0 1,0 ${-2 * rad},0`;
    return `${ringPath(ring1)} ${ringPath(ring2)} ${ringPath(ring3)} Z`;
  }
  // Infrastructure (router) overrides the role-derived shape: an
  // axis-aligned square reads as "substrate" — distinct from the
  // diamond (rotated square = topic-broker pin) and from the circle/
  // hexagon/triangle peer shapes. Pairs with the desaturated fill in
  // fillFor() so the eye registers "infra, not peer" at a glance.
  if (n.node_kind === "router") {
    const s = r * 0.85;
    return `M${-s},${-s} L${s},${-s} L${s},${s} L${-s},${s} Z`;
  }
  // Trashgoat — platform-internal chaos actor publishing deliberate
  // trash under `trashgoat/<experiment>/...`. Five-pointed star
  // distinguishes them from every real-peer shape (circle / hexagon /
  // inverted triangle) regardless of which role the experiment is
  // playing. Paired with a pinned amber vendor border (vendorColor)
  // and a dashed stroke (render path) so the eye registers
  // "synthetic, not real traffic" without scanning the side panel.
  // See docs/design/trashgoat.md + docs/design/panoscope.md
  // §"Trashgoat glyph".
  if (vendorOf(n) === "trashgoat" && n.role !== "topic") {
    return starPath(r);
  }
  switch (n.role) {
    case "topic":
      // Diamond — rotated square (broker pin between pub and sub).
      return `M0,${-r} L${r},0 L0,${r} L${-r},0 Z`;
    case "sink": {
      // Inverted triangle — apex pointing down, "data falls in".
      const h = r * 1.1;
      return `M${-r},${-h * 0.5} L${r},${-h * 0.5} L0,${h} Z`;
    }
    case "deriver": {
      // Regular hexagon — flat top, suggests "processing".
      const s = r;
      const a = s * 0.5;
      const b = s * Math.sqrt(3) / 2;
      return `M${-s},0 L${-a},${-b} L${a},${-b} L${s},0 L${a},${b} L${-a},${b} Z`;
    }
    case "source":
    case "unknown":
    default:
      // Circle.
      return `M${-r},0 a${r},${r} 0 1,0 ${2 * r},0 a${r},${r} 0 1,0 ${-2 * r},0 Z`;
  }
}

const VENDOR_COLORS = [
  "#a371f7",  // caracal — purple
  "#58a6ff",  // community — blue
  "#f78166",  // (next) — orange
  "#2ea043",  // (next) — green
  "#d29922",  // (next) — yellow
  "#f85149",  // (next) — red
];
// Vendors with a pinned identity color — never enter the rotating
// palette assignment so the glyph reads as "this specific thing" on
// every load regardless of first-seen order.
const PINNED_VENDOR_COLORS = {
  // Trashgoat = amber. Pairs with the star shape (shapeFor) + dashed
  // stroke (render path) for the platform-internal chaos-actor
  // visual treatment. Reuses --schema-ecn so we don't add a token.
  trashgoat: "#d29922",
};
const vendorColorAssign = new Map();
function vendorColor(vendor) {
  const pinned = PINNED_VENDOR_COLORS[vendor];
  if (pinned) return pinned;
  if (!vendorColorAssign.has(vendor)) {
    const c = VENDOR_COLORS[vendorColorAssign.size % VENDOR_COLORS.length];
    vendorColorAssign.set(vendor, c);
  }
  return vendorColorAssign.get(vendor);
}

/** True iff the node should render with a dashed border. Trashgoat's
 *  deliberate-trash publishes carry the dashed stroke so the eye
 *  registers "synthetic / intermittent / platform-internal" at a
 *  glance — same vocabulary the existing fillFor() uses to push the
 *  router glyph into the visual background. */
function isDashedNode(n) {
  return vendorOf(n) === "trashgoat" && n.role !== "topic";
}

function edgeTooltip(e) {
  return `${e.topic}\n${(e.projected_eps).toFixed(2)} msg/s · ${e.total_events} total`;
}

function nodeId(ref) {
  return typeof ref === "string" ? ref : ref.id;
}

/** Matches a bare 32-char hex tenant anchor (`<uuid>/<taxonomy>`
 *  slot pool, ADR 1025) or a canonical dashed UUID. Global so every
 *  occurrence in a multi-segment key is shortened. */
const TENANT_UUID_RE =
  /\b[0-9a-f]{32}\b|\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi;

/** The readable tail of a group pattern: every trailing wildcard segment plus
 *  the two literal segments before them. */
export function groupShortLabel(pattern) {
  const parts = pattern.split("/").filter(Boolean);
  let i = parts.length;
  while (i > 0 && (parts[i - 1] === "*" || parts[i - 1] === "**" || parts[i - 1] === "…")) i--;
  return parts.slice(Math.max(0, i - 2)).join("/") || pattern;
}

/** Does a group selector cover a topic? Selectors are segment lists where
 *  a single star matches one segment and a double star any remainder. Display attribution only (which
 *  element a live publish animates on) — never an authorization input. */
export function selectorMatches(selector, topic) {
  let i = 0;
  for (; i < selector.length; i++) {
    if (selector[i] === "**") return true;
    if (i >= topic.length) return false;
    if (selector[i] !== "*" && selector[i] !== topic[i]) return false;
  }
  return i === topic.length;
}

/** Display-only abbreviation: replace any UUID run in `s` with its
 *  first 4 hex chars + ellipsis (e.g. `2cf6fe6d…c5243fc3` → `2cf6…`).
 *  The full string stays the routing/selection key — this only shortens
 *  what's painted. Tenant-uuid prefixes and peer-uuid topic segments
 *  otherwise eat huge horizontal space on the canvas + side panel.
 *  Idempotent on already-short strings (no UUID run → returned
 *  unchanged). Four chars is enough to scan/disambiguate at a glance;
 *  the full value is one hover away. */
export function shortenUuids(s) {
  return s.replace(TENANT_UUID_RE, (m) => {
    const hex = m.replace(/-/g, "");
    return `${hex.slice(0, 4)}…`;
  });
}

function labelOf(n) {
  // Vendor-collapse super-node label: "VENDOR  Np·Mt". Always
  // rendered (not gated on selection like peer labels) since the
  // super-node IS the only visible representation of the vendor in
  // collapsed mode — the cluster hull + name disappear with the
  // peers it grouped.
  if (n.is_vendor_super) {
    const p = n.super_peer_count ?? 0;
    const t = n.super_topic_count ?? 0;
    const name = (n.vendor ?? "").toUpperCase();
    return `${name}  ${p}p·${t}t`;
  }
  // Group label = the tail of its pattern with the wildcards kept, so
  // `…/entities/v3/*` reads as `entities/v3/*` and a family of stream
  // directories as `stream/*/…`. No count: it is on the hover badge. The
  // whole-publisher group reads by its publisher.
  if (n.is_singleton_super) {
    if (n.group_kind === "publisher") {
      return `${n.display_name ?? shortId(n.singleton_source_peer ?? "")} · all`;
    }
    return groupShortLabel(n.singleton_namespace ?? "");
  }
  // Infrastructure label first — a Zenoh router has no schema-decl
  // (it's the substrate, not a publisher), so display_name + vendor
  // are both null. Without this branch, the canvas labels it as a
  // truncated ZID hex string which reads as a mystery peer instead
  // of "the broker every other peer connects through".
  if (n.node_kind === "router") return "zenoh-router";
  // Slot alias (ADR 1039) wins when present — it's the operator-assigned
  // identity for the node's slot anchor (the formal human name behind the
  // UUID), so it outranks the schema-decl display_name. Absent for random
  // ZIDs / unknown anchors / portal-down, in which case we fall through.
  if (n.slot_alias) return n.slot_alias;
  // Prefer the schema-decl `display_name` (e.g. "tarsier-alpha") over
  // the cert-bound `vendor` — after the notional namespace
  // collapse every cert vendor is uniform ("notional"), so labeling by
  // vendor alone produces 22 nodes that all read "notional". The
  // display_name field, populated by the synth-vendor binary's
  // schema-decl publisher, restores the per-peer variety.
  if (n.display_name) return n.display_name;
  if (n.vendor) return n.vendor;
  // Topic-virtual-nodes — abbreviate to the last two segments
  // (`entity/v1` instead of `caracal/tracks-demo/entity/v1`) since
  // the publisher node already encodes the vendor/system prefix.
  // Full keyexpr is in the SVG <title> tooltip on hover.
  if (n.role === "topic" && n.id.startsWith("topic:")) {
    const full = n.id.slice("topic:".length);
    const parts = full.split("/").filter(Boolean);
    if (parts.length >= 2) return parts.slice(-2).join("/");
    return full;
  }
  // Topic-prefix fallback identity (synth-vendor publishers without
  // SourceInfo arrive as `<vendor>/<system>`) — render the whole
  // pair, not the last 8 chars (which produced "wer-demo" from
  // "caracal/power-demo"). Real Zenoh ZIDs are 32-char hex strings;
  // for those we still want the short form to avoid clutter.
  return shortId(n.id);
}
