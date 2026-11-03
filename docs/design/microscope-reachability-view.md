# microscope reachability view — the mesh graph an entity sits inside

| Field | Value |
|---|---|
| Status | **Proposed.** Design only; no code. The data reads all exist. |
| Scope | A second microscope view: instead of a structured event log, the mesh **paths** an entity's peers have — permitted, reachable now, recently used — as a clickable graph. Bounded deliberately to the NetBird layer. |
| Sibling | `netvis.md` — the same resolver at fabric scale (unpopulated = the policy set, populated = the live fabric). This view is per-ENTITY; netvis is per-GOATNET, and the scope parameter that separates them must never have a default. |
| Related | `microscope.md` (the surface this joins), `actor-relationship-graph.md` (ADR 1065 — declared structure, a different graph), `mesh-acl.md`, [`zenoh-surface-deep-links.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/zenoh-surface-deep-links.md) (Block 94 — the clickthrough vocabulary), [`design-system.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/design/design-system.md) |

## The ask, and the scope decision that makes it tractable

> "a second view … more organic and dynamic and clickthroughable, that shows the behaviors of
> the entity over time and through the various dimensions and counterparts they interact with.
> Visualizing which paths are possible, which are currently active and which have been used
> recently."

The first design pass reached for the **application** domain — Zenoh topics, grants, data
access — and immediately hit two walls: the cardinality is enormous, and "which paths are
possible" is unanswerable there, because Zenoh denies silently (F-177)
and has no central enumerable ACL. A faint "possible" edge set in that domain would invite
the reader to infer that undrawn edges are impossible. They would only be *unenumerated*.

**Scoping this view to the NetBird layer dissolves that problem rather than managing it.**
The NetBird policy set is a complete, readable matrix: `GET /api/policies`, `GET /api/groups`,
`GET /api/peers` return the whole thing. Within that layer the complement genuinely *is*
denied, and "possible" is an honest word. This document is deliberately about that layer and
no other. The application domain may get its own view later; it does not get folded into this
one, because the two have opposite honesty properties.

## Why the cardinality is smaller than it looks

NetBird policy is expressed **group → group**. Peers are only membership. So a graph whose
nodes are *groups* is not a summarization of the policy — it is an exact representation of it.
Nothing about the permitted-path set is lost by not drawing peers.

```
[entity's peers] ── member-of ──> [source groups] ──policy rule──> [destination groups]
                                                                          │
                                                                   (click to expand)
                                                                          v
                                                                    [member peers]
```

At the ADR 0215 ceiling — fleet 6000 peers —
the group-level graph stays in the tens of nodes. Peer expansion is a click, never a default.
The high-cardinality thing the operator worried about is the one thing the graph need not draw.

## Direction and transport limits are part of the permission

A path is not a boolean. `A → B` and `A ↔ B` are different grants, and
"reaches B" means something very different on ICMP than on every port. Showing
an undifferentiated line for all of these makes a policy look more permissive
than it is — or, worse, less.

**The first cut got this wrong in the dangerous direction.** The table printed
strings like `(icmp)` and `(web/dns/fabric tcp)` that came from the human-written
policy NAME, not from the rule. `NbPolicyRule` deserialized only `sources`,
`destinations` and `enabled`; protocol, ports, port ranges, `bidirectional` and
`action` were never read. A policy *named* `…(icmp)` whose rule permits every
protocol on every port would have rendered as tightly scoped while being wide
open — a label doing the work of a control, on the surface people use to reason
about posture.

Now derived from the rule:

| column | source | reads |
|---|---|---|
| direction | `rule.bidirectional` | `→ outbound only` / `↔ both ways` |
| protocol · ports | `protocol` + `ports` + `port_ranges` | `ICMP`, `TCP 443`, `all protocols · all ports` |

Three deliberate conservatisms, because every default here should read narrow:

- `bidirectional` defaults **false**, so a field the API stops sending can never
  silently widen the claim.
- An edge is drawn two-way only when EVERY rule behind it is. One one-way rule
  keeps the whole edge one-way; the return path must earn its own arrow.
- `action: drop` rules are skipped. They were previously counted as permits.

A bidirectional rule also registers the REVERSE group pair, so the return path
appears as its own edge instead of being implied — asymmetry is the case worth
seeing, and it was invisible when only the forward pair was recorded.

**On reading a broad fan of lines as a ZTA failure:** it may be one, and the page
must let you tell. Whether a wide fan is acceptable depends on the transport
limits, which is exactly what the display was omitting; the legend now says so
out loud — *a line is not unrestricted access* — because the picture alone
invites the harsher reading.

## The three states, and the one we cannot honestly show

| state | source | status |
|---|---|---|
| **Permitted** | `GET /api/policies` — the enabled rules referencing the entity's groups | **Complete.** Enumerable, and the complement is validly "denied" at this layer. |
| **Reachable now** | peer `connected` + `last_seen` on both ends | **Available** — but it means *reachable*, not *in use*, and must be labelled so. |
| **Recently used** | — | **NOT AVAILABLE.** See below. |

**Recently used is the one that would be fabricated.** The NetBird management API records that
peers connected; it has no pairwise flow record, so it cannot say peer A talked to peer B.
Drawing "recently used" from what the management plane holds would be inventing it.

An honest source does exist, and it is not the management API: WireGuard's per-peer
`latest handshake` and rx/tx counters on the host are genuine pairwise evidence of a real
session. Collecting them means an on-host reader (goat-client or snitch), which is a different
piece of work from this view. **Decide before building:** either that collection lands first,
or this view ships with two states and says on the page that usage is not recorded — never a
third state inferred from the first two.

One more caveat worth a staleness stamp rather than a redesign: what the management API returns
is the **declared** policy. Enforcement is distributed to peers, so a peer that has not synced
can diverge from it.

## Framework — D3, and specifically not `d3-force`

**Recommendation: D3 as a toolkit** — scales, transitions, `d3-brush` for the time scrubber,
zoom, path generators — over a **deterministic layout**.

The trap is force-directed layout, which is what "organic" makes everyone reach for. It is
wrong here for reasons this repo has already written down in another form:

- Position encodes nothing, so the most salient visual property of the picture is meaningless.
- Positions differ every load, so an operator can never build the spatial memory that makes a
  recurring view fast to read. "Was that node there yesterday?" becomes unanswerable.
- It animates continuously, and **continuous drift reads as activity when nothing is
  happening.** This is precisely the failure `netbird_peers.rs` already rejects in the
  timeline, where a connected peer is deliberately not emitted as an event because a
  continuously-refreshed `last_seen` was "liveness STATE wearing an event costume." A jiggling
  graph is that same bug in two dimensions.

Layout instead: entity peers at the centre, groups in stable rings or columns by dimension,
ordered by a stable key so the same graph draws identically every time.

Runner-up considered: **Cytoscape.js** — more batteries-included (expand/collapse, hit-testing
for free). Rejected because we would disable its layouts anyway, it fights custom edge
encoding, and edge encoding is most of the work here.

Two things make D3 cheap in this repo specifically: single-file JS libraries are already
vendored (`maplibre-gl.js`, `highlight.min.js`, swagger), so no bundler and no new toolchain
against the [ADR 1016](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1016-complexity-budget-substrate-encapsulation.md) complexity
budget; and Block 94 already defines the clickthrough vocabulary (`peer=`, `vendor=`, `key=`,
`site=`), so "click a node, land in stethoscope or portal" is wiring an existing contract.

## Server-computed model and layout

Compute the graph **and its node positions in Rust**, serve JSON plus a server-rendered SVG
skeleton, and let D3 own interaction only (hover, select, expand, time-brush, transitions).

This is not stylistic. Microscope's tests assert on rendered markup, and that discipline caught
two real defects in the kind/outcome work — a classifier inversion and a discarded error
message — before either shipped. A client-side-only graph puts the whole view beyond those
tests. Server-side layout also means the page renders something meaningful with JS disabled,
matching [`minimalism-under-network-constraints.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/design/minimalism-under-network-constraints.md).

## Spine wrinkle: one entity is several peers

An entity commonly maps to several peers — a laptop, a phone, an Orin they have custody of.
Microscope joins those by handle prefix and grades the join `Convention`, not `Strong`. The
graph must therefore render the entity's peers as **distinct nodes**, not collapse them into
one dot: collapsing would launder a convention-grade join into a confident single identity.

## What is needed to build it

1. Extend `NbPolicyRule` — it currently deserializes only `sources`, `destinations`, `enabled`
   (all the membership-transfer coverage gate needed). Protocol, ports and direction are in the
   API and not yet read.
2. A graph model in `microscope`: entity peers → groups → policy edges → destination groups,
   with deterministic layout.
3. The decision above on "recently used": collect WireGuard handshake data first, or ship two
   states and say so.
4. Vendor D3; render SVG server-side; wire Block 94 links outbound.

## Explicitly out of scope

Application-domain reachability (Zenoh topics, grants, data-access lattice). Different honesty
properties, different cardinality, and it needs its own design — not a layer on this one.
