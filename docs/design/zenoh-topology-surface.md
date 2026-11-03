# Zenoh topology surface — `topology.<site>.<zone>`

| | |
|---|---|
| Status | Designed; build in progress (track `zenoh-topology-surface`) |
| ADR | [1160](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1160-zenoh-topology-surface.md) |
| Component | `ops/goat-topology` — one binary, two modes |
| Evidence it rests on | M-111 (the fleet on `Auto`), M-112 (`Custom([...])` in a rig) |
| Canon it must obey | [ADR 1123](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1123-client-demand-must-not-amplify.md) (no query on request), [ADR 1036](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1036-environment-health-surface.md) (self-publish + roll-up), [ADR 1041](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1041-zenoh-router-zid-pinning-and-source-provenance.md) / [ADR 0844](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0844-zenoh-deployment-topology-and-acl-v1.md) (a ZID is a label, not a credential) |

## 0. Purpose, and what would count as success

**The problem.** `gateway.south` decides two things that matter: how much
routing state a goatnet carries, and who can see whom at the routing layer. It
runs fleet-wide on a default nobody chose (M-111),
which leaves two failure modes invisible — a config that is wrong, and a config
that does not match what is running — plus one genuinely dangerous mistake: the
filter list has the exact shape of an access control while two of its five axes
are asserted by the peer being filtered.

**Who it is for.** The operator carving, debugging, or reviewing a routing
plane. Explicitly **not** a daily-glance surface; nobody should be watching this
at rest, and the design does not pretend otherwise.

**Success is four things:**

1. "What is my routing plane doing" is answerable without a shell on a router host.
2. The peer-asserted-filter mistake is caught at review time, not after someone has scoped a partner with a routing hint.
3. A publisher going quiet reads as **unknown**, never as healthy.
4. Zero fabric cost per viewer, verifiable in a diff.

**Non-goals**, stated so they do not creep in: alerting, any control path, mesh
reachability, and any claim about scale beyond what M-111 and M-112 measured.

### 0.1 What that purpose implies about the layout

The surface is opened when something is wrong or about to change — so it is
ordered by the question actually being asked, most frequent first:

| Order | Question | Answered by |
|---|---|---|
| 1 | Is anything wrong? | The verdict line |
| 2 | Where am I? | The routing graph |
| 3 | What exactly is wrong, and what do I do? | Findings |
| 4 | Did something change? | The change feed |
| 5 | What is the detail? | The routers table |
| 6 | What would a carve cost? | The planner, on the measured model |

**The graph leads, but the verdict leads it.** The graph is the orientation
device — it answers "where am I" faster than any sentence can, and an operator
arriving cold needs that before words. What it must *not* be asked to do is
carry the verdict: nobody should have to read a picture to learn whether
anything is wrong, so a one-line verdict sits above it and says so outright.

### 0.2 Findings are written to be read cold

A finding is read by someone who has just opened the page, mid-incident,
possibly having never heard the word "subregion". So each one is built in a
fixed order that answers questions in the order they occur:

| Field | Answers |
|---|---|
| `headline` | **What is the problem, in plain words?** Consequence first, no jargon, no identifier. |
| `what` | What did we actually see? The evidence, with names and numbers. |
| `why` | Why does that matter? Mechanism, in short sentences. |
| `impact` | What happens if I do nothing? |
| `steps` | What do I do? Imperative, ordered, one action each. |

Three rules that fall out of that, each of which the earlier version broke:

- **The identifier is not the headline.** `T-001` is quotable in a ticket and
  tells a cold reader nothing, so it sits at the end of the header row.
- **A branch belongs in the steps, never inside a sentence.** The first version
  of `T-001` buried "if the intent is routing… if the intent is access…" in a
  paragraph, and a reader could not tell which half applied to them. It is now
  step 1: *decide which this is for*, then a step per branch.
- **Jargon is allowed below the headline, never in it.** A test builds one of
  every finding and asserts the headline is a plain sentence, carries no
  identifier, and contains none of `subregion` / `zid` / `link-state` / `Σ`.
  Below the headline the surface has room to define its terms, and a collapsible
  glossary at the section head does the rest.

Finding text in the design preview is emitted by the engine's own test rather
than transcribed, so the preview cannot drift from what the code says.

**The change feed is what an incident actually needs.** "A router joined the
north region twenty minutes ago" is the fact that explains a symptom, and a
surface showing only instantaneous state cannot produce it. The feed is held in
memory, so a restart empties it — the UI therefore says *since this surface
started watching*, because an empty feed must read as **not seen**, never as
**did not happen**.

**The planner exists because regions are for carving.** A surface about carving
that cannot help you evaluate a carve is missing its most useful move. It is
arithmetic on the M-112 model, and it says so on its face: a projection for
planning, bounded by a measurement taken at two regions of three routers on one
host, counting routing state only.

## 1. What this is for

A goatnet's zenoh routers form a routing domain whose cost and blast radius are
set by `gateway.south` — a setting that, as M-111
found, is on its `Preset(Auto)` default fleet-wide because **no configuration
sets it anywhere**. The behaviour is now documented in
`zenoh-data-fabric.md` §"Hierarchical routing". A
document is the right home for *what the primitive does*. It is the wrong place
to answer *what is my fabric doing right now, and is any of it wrong?*

`topology` answers that. It renders every router in a goatnet, the region each
of its sessions landed in, the shape of the resulting link-state domains, and a
graded list of findings with the remediation for each.

**Why it earns its own surface rather than a panoscope tab.** Panoscope is the
data COP — topics, samples, producers, hygiene. This is the *routing* plane: it
has a different subject (routers, not data), a different audience (whoever is
carving or debugging a fabric, not whoever is watching the mission picture), and
— decisively — a different failure mode. Panoscope is the one surface whose
client demand reaches the fabric ([ADR 1123](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1123-client-demand-must-not-amplify.md));
topology must never be, and keeping it out of panoscope keeps that property
easy to verify rather than something a future change could quietly erode.

### Scope boundary — read this before extending either surface

| Surface | Owns | Does **not** own |
|---|---|---|
| `netvis.md` | Mesh / IP reachability — can peer A reach peer B, and which ACL decides | Anything about zenoh routing |
| **topology** | Zenoh routing domains — regions, link-state graphs, route tables, `gateway.south` config | Anything about IP reachability or the mesh ACL |

They sit on different layers and answer different questions. A router pair can
be perfectly reachable at the IP layer and still be in different routing
regions, and the reverse. **Neither surface grows into the other**; a question
that spans both is answered by opening both, and by the cross-surface deep-link
convention ([`zenoh-surface-deep-links.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/zenoh-surface-deep-links.md)).

## 2. Shape — one binary, two modes

```
                 router host (each one)                 mgmt-host
   ┌──────────────────────────────────┐      ┌─────────────────────────┐
   │  zenohd                          │      │  goat-topology serve    │
   │   adminspace REST                │      │   subscribes            │
   │   127.0.0.1:8000  ◄──── curl ──┐ │      │   ops/fabric/topology/**│
   │  /etc/<stack>/config.json5 ◄─┐ │ │      │                         │
   │   (gateway block only)       │ │ │      │   in-memory store       │
   │  goat-topology publish ──────┴─┘ │      │        │                │
   │        │                         │      │        │                │
   └────────┼─────────────────────────┘      │        ▼                │
            │  publish RouterTopologyV1      │   findings engine       │
            │  ops/fabric/topology/          │        │                │
            └──────────► <site>/<node> ──────┼──►     ▼                │
                        (the fabric)         │   HTTP + graph UI       │
                                             │   topology.<site>.<zone>│
                                             └─────────────────────────┘
```

**Publish mode** runs on every host that runs a router. It reads **its own
router only**, on a timer, and publishes what it saw. It knows nothing about any
other router and never asks about one.

**Serve mode** runs on the mgmt-host. It subscribes, keeps the last snapshot per
router in memory, and serves the UI from that memory. **A browser request never
touches the fabric.**

### 2.1 The publisher reads host loopback, not a network namespace

The router's adminspace REST listener is published to host loopback —
`8000/tcp -> 127.0.0.1:8000`, verified on zagros and zagros-prod on 2026-09-04. The
publisher is therefore an ordinary container with `network_mode: host` or a
loopback port mapping; it needs **no `nsenter`, no `--privileged`, no
`CAP_SYS_ADMIN`, and no zenoh client cert to read the router's own state**.

What *is* invisible from the host namespace is the router's zenoh **sessions**
as seen by `ss` — they live in the container netns. That is a debugging trap and
is recorded as such in `ops/scripts/zenoh-region-census.sh`; it is not a
constraint on this design. Getting that distinction wrong would have designed a
privileged container for no reason.

The publisher **does** need an mTLS client cert — but only to *publish* onto the
fabric, exactly like every other meter.

### 2.2 Why self-publish rather than have the surface ask

Three independent reasons, any one of which would be sufficient:

1. **[ADR 1123](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1123-client-demand-must-not-amplify.md).**
   Client demand must not amplify into fabric load. A surface that queried each
   router per page view would be the exact shape that ADR forbids.
2. **Reachability.** The adminspace is bound to each router host's *loopback*. A
   central reader cannot reach it at all without opening a new network path,
   which would be a worse design and a new attack surface.
3. **Brokerless safety.** [ADR 1036](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1036-environment-health-surface.md)'s
   pattern — each node self-publishes, a passive reader rolls up — survives
   partition. A site cut off from the mgmt-host keeps publishing; the surface
   shows its last snapshot with an honest age, and recovers on reconnect with no
   central coordination.

## 3. The envelope — `RouterTopologyV1`

Published to `ops/fabric/topology/<site>/<node>`, protobuf, canonical schema at
`ops/goat-topology/proto/router_topology_v1.proto`. One message per router per
interval. Shape:

| Field | Meaning |
|---|---|
| `zid`, `site`, `node` | Who is speaking. `node` is the segment in the key. |
| `ts_unix_ms`, `interval_secs` | When, and how often to expect the next one. |
| `zenoh_version` | From the adminspace, so a version skew is visible. |
| `gateway_mode` | `auto` \| `custom` \| `unreadable` — see §3.1. |
| `custom_subregions[]` | For `custom`: per subregion, its index and its filters, each filter carrying which of `modes` / `interfaces` / `zids` / `region_names` it uses and whether it is `negated`. |
| `regions[]` | Per region: name, link-state `V` and `E`, member ZIDs, and the session count by `whatami`. |
| `route_successor_count` | Observed, for the Σ V(V−1) cross-check. |
| `sessions[]` | Per session: peer ZID, `whatami`, assigned region, link endpoints. |

### 3.1 Two sources, because neither one is enough

Measured on zagros, 2026-09-04:

| Source | Gives | Verified |
|---|---|---|
| **Adminspace** `127.0.0.1:8000/@/local/**` | Observed regions, link-state graphs, route table, sessions | 737 keys returned, **none of them config** — it does not expose `gateway` at all |
| **Declared config** `/etc/<stack>/config.json5` on the router host | The `gateway` block as deployed | Readable, mode 0644; on zagros it contains **no `gateway` key**, independently confirming M-111's finding from a third angle |

So `gateway_mode` and `custom_subregions` come from the **file**, and everything
else from the **adminspace**. The publisher runs on the router host, so it has
both.

**The publisher extracts the `gateway` block and nothing else.** That config file
is ~900 KB and is overwhelmingly `access_control` — the goatnet's live ACL,
including slot anchors and subject CNs. None of it is read into the envelope,
logged, or published. A meter that shipped ACL content onto the bus would be an
identity leak dressed as observability. The parser targets one top-level key.

**Where a source is missing, say so.** If the config file is unreadable,
`gateway_mode` is **`unreadable`** — never inferred as `auto` from the absence
of a `south:` region. The asymmetry that makes this necessary: `Auto` and a
`Custom` list that happens to match nothing produce **the same observable
regions**, and the second is a real misconfiguration. Inferring from observation
alone would report a fault as healthy. This is [ADR 1036](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1036-environment-health-surface.md)'s
honesty rule: never a false green, and `unknown` is a first-class state.

## 4. The findings engine

Each finding is `{id, severity, subject, what, why, remediation}`. Severity is
`critical | warning | advisory | info`. Every finding names the router or pair
it is about — never "the fabric".

| ID | Fires when | Severity | Remediation offered |
|---|---|---|---|
| `T-001` region-filter-authz | A `custom_subregion` filter uses `zids` or `region_names` | **critical** | These are values the peer asserts about itself. They route; they do not authorize. Scope by certificate identity in the ACL instead. Never widen a region to grant access. |
| `T-002` empty-filter-list | A subregion has `filters: []` | **critical** | An empty list matches nothing, so the subregion is inert and every peer silently falls north — the opposite of the intent. Either populate it or remove the entry, remembering the region id is the list index. |
| `T-003` mode-drift | Routers in one goatnet disagree on `gateway_mode` | warning | Region config is per-router and a split is legitimate during a rollout. If it is not a rollout, converge them; state which routers differ. |
| `T-004` asymmetric-view | A's north graph contains B, B's does not contain A | warning | One side has not settled, or a session dropped one-way. Re-check after one interval; if it persists, inspect that pair's transport. |
| `T-005` route-table-mismatch | `route_successor_count` ≠ Σ V(V−1) over that router's **edge-bearing** regions | warning | A persistent mismatch means the router is mid-settle, or the model is wrong for this topology — capture it, it is worth a finding. See §4.1 for why the edge condition is in the rule. |
| `T-006` stale-publisher | No snapshot within 3 intervals | warning | The router's view is **unknown**, not healthy. Check the publisher, not the router. |
| `T-007` version-skew | Routers report different `zenoh_version` | warning | The version-of-record is `=1.9.0` fleet-wide (`zenoh-version-pinning.md`); a skew opens the 1.9.0-vs-1.0.4 handshake-failure class. |
| `T-008` isolated-router | `V=1` and no non-client sessions | info | A single-router goatnet is a legitimate shape, not a fault. Stated so an empty graph is not misread. |
| `T-009` domain-growth | Σ V(V−1) across the goatnet, and what one more router costs | advisory | Not a problem below the thresholds M-112 bounds. Shown so the growth curve is visible before it matters, with the honesty that M-112 measured `V ≤ 3` and everything beyond is projection. |
| `T-011` unreadable-config | The declared config could not be read | warning | The mode is reported `unreadable`, never assumed `auto` — see §3.1. Give the publisher read access, or fix the path it was pointed at. |
| `T-010` declared-vs-running | Mode is `custom` **and** the declared subregion indices are not the running ones | **critical** | The file on disk is not what the router is doing — almost always a config change without a restart. Merged is not deployed. Restart the router, or find out who edited the file out of band. Only decidable under `custom`; see §4.2. |

### 4.1 The successor model needed a correction that only live data could give

M-112 established `route/successor = Σ V(V−1)` over a router's regions, exact at
every point in the rig. Running this component's `inspect` against the **live
zagros router** on 2026-09-04 produced a route table of **2** where that formula
predicts **8** — so `T-005` would have fired a warning on a healthy production
router on every cycle.

The cause is the anomaly M-111
recorded and could not explain: zagros's `south:0:peer` graph carries **V=3 with
E=0**, nodes known through gossip that the router holds no link to. With no
links there are no paths, so that region contributes no successor entries
however many nodes it lists.

**The rule is therefore `V(V−1)` for regions with edges, and 0 for regions
without.** That fits every observation held — three live goatnets and both rig
arms:

| Source | Regions | Predicted | Observed |
|---|---|---|---|
| zagros (live) | north V2 E1; `south:0:peer` V3 E0 | 2 | 2 |
| zagros-prod (live) | north V3 E2; `south:0:peer` V1 E0 | 6 | 6 |
| nova (live) | north V1 E0; `south:0:peer` V1 E0 | 0 | 0 |
| M-112 gateway | north V3 E2; `south:0:router` V3 E2 | 12 | 12 |
| M-112 member | north V3 E2; `south:0:peer` V1 E0 | 6 | 6 |

M-112 is not wrong — every region in its rig was well-formed, so the rig could
not have surfaced this. It is the kind of refinement that only arrives when a
model meets production, and it is recorded here rather than left in the code
because the next person to reason about route-table size needs it.

### 4.2 `T-010` could not have been stated the way it first was

The first cut of `T-010` looked for a divergence by asking whether the mode was
`auto` while a `south:` region held more than one member — reading a populated
southern region as evidence someone had configured a `Custom` list the file no
longer showed.

That premise is wrong. **`Preset(Auto)` computes south regions itself**, at
runtime, from the mode pairs of the sessions it holds. Pointed at the live zagros
router on 2026-09-06 the rule fired **critical** on `south:0:peer` V=3 E=0
produced by an entirely empty `gateway` block — a healthy router, in the
fleet's only configuration. Every goat router runs `Preset(Auto)`
(M-111),
so this was a permanent false critical on all of them.

Under `Auto` there is nothing declared for the running state to diverge *from*,
so the only honest answer is silence. The check is decidable only under
`Custom`, where the file names subregions by list position and zenoh assigns
region ids from that same position (`region.rs:124-136`): the declared index set
and the running index set must match, and they part when the file is edited
without a restart — which is the fault the finding was written for.

This is the same shape as §4.1: a rule that held across every arm of the rig,
because every region in the rig was well-formed and every arm that had southern
peers was a `Custom` arm. Both corrections came from one router in production.
`auto_with_a_populated_south_region_is_not_a_divergence` in `findings.rs` pins
the live case.

**`T-001` is the reason this surface has a findings engine at all.** The
`gateway.south` filter list has the exact shape of an access control and two of
its five axes are peer-asserted. A surface that rendered the config without
saying so would make the hazard *easier* to reach. The engine therefore names it
as critical and offers the correct alternative in the same breath.

**What the engine will never do:** recommend `region_names` or `zids` as a way
to scope anything, or recommend a `Custom` layout as though it were tested at
scale. M-112 measured two regions of three routers on one host. The `T-009`
advisory says so on its face.

## 5. What a page view costs

Nothing on the fabric. The surface answers from memory. The only fabric traffic
this component generates is one small publish per router per interval — a
constant, independent of how many people have the page open.

This is the property to protect in review. Any future change that makes the
surface fetch on request converts it into the shape ADR 1123 exists to forbid.

## 6. Honesty rules

Inherited from [ADR 1036](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1036-environment-health-surface.md), restated
because they are load-bearing here:

- A router with no recent snapshot is **`unknown`**, rendered as such, never
  omitted and never green. An omitted router reads as "no problem".
- A router whose `gateway` config could not be read is **`unreadable`**, not
  `auto`.
- Every rendered number carries its snapshot age.
- A finding states what was *observed*. Where a finding rests on a projection —
  only `T-009` does — it says so in the finding text, not in a footnote.

## 6a. The surface is born in the design system

It carries no palette of its own. `src/serve.rs` `include_str!`s
[`docs/design-system/colors_and_type.css`](../design-system/colors_and_type.css)
and `components.css` and splices them ahead of the page's own rules — the same
way flashlight consumes them, so there is one source of truth for the bytes and
a design-system change reaches this surface on the next image build rather than
drifting in a vendored copy ([`design-system.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/design/design-system.md)
§"How consumers receive the CSS bytes"). The Dockerfile therefore builds from
the **repo root**, not from the crate directory.

Two properties are asserted by tests rather than left to review:

- **The tokens actually land.** A failed splice would fall back to browser
  defaults and read as a CSS mistake rather than a build one, so the test
  checks the token bytes are present *and* that they precede the page's own
  rules.
- **The page makes no external request.** No CDN, no webfont, no analytics. A
  mesh-only admin surface must not attempt egress on page load — the design
  system states that as canon, having found a Google Fonts `@import` that could
  never have worked on a mesh-only surface. Type hierarchy here comes from size,
  weight and spacing, which work in any face.

What the visualization shows, and why each choice is information rather than
decoration:

| Element | Encodes |
|---|---|
| Node ring arc | Clients terminating at that router — weight at a glance, and they cost routing nothing |
| Edge colour | The region the session was assigned: north routes, south terminates |
| Animated edge flow | **North edges only** — the routed domain is the part carrying state |
| Node tag | The declared gateway mode, or `stale` when the snapshot is old |
| Region row `V/E` | An edgeless region is labelled "carries no routes" — the §4.1 correction, on the face of the table |

The graph layout is **deterministic, not force-simulated**. A goatnet has a
handful of routers, and a layout that settles differently on each refresh makes
an operator re-find the same node every time. Same input, same picture.

## 6b. How the component is put together

One concern per module, and the dependency direction is a DAG — nothing points
back up:

```
wire      protobuf types                        (no deps)
assets    compiled-in static files              (no deps)
findings  the engine                            -> wire
store     what is known, and what changed       -> wire
view      the API projection                    -> findings, store, wire
serve     HTTP, routes, subscription            -> assets, store, view, wire
```

`store` deliberately does **not** know about `view`. An earlier cut had the
store building its own projection, which made state depend on presentation and
had the two importing each other. `serve` is the only orchestrator: it ingests,
asks whether anything changed, and installs the rebuilt payload.

The page is split the same way — `index.html` is markup, `app.css` is style,
and behaviour is ES modules (`app.js` plus one renderer per section, none of
which knows about any other). A test walks the import graph and fails if a
module is imported but not served, because that is a 404 at runtime and nothing
at build time.

### 6b.1 A request does no work

This is the property to protect in review, and it is stronger than the ADR 1123
minimum:

| | |
|---|---|
| **Write path** (a router publishes, ~every 30s) | Diff the fingerprint, run the findings engine, build the projection, serialize once, bump the ETag. |
| **Read path** (a browser polls, every 15s, × viewers) | Clone one `Arc`, or return `304`. No engine run, no serialization, no allocation of consequence. |

Three decisions make the read path that cheap, and each would be easy to undo
by accident:

- **The projection is built on write.** Building per request would redo
  identical work for every viewer, inside a lock every viewer contends for.
- **The payload carries no ticking values.** There is no `generated_ms` and no
  `age_secs` — routers carry an absolute `received_ms` and the browser
  subtracts. A value that moved every tick would change the bytes, change the
  ETag, and defeat the `304` while nothing had actually happened.
- **The staleness tick is edge-triggered.** It rebuilds only when the set of
  stale routers actually changes, not every ten seconds. A router going quiet
  still surfaces immediately; a settled fabric produces byte-identical payloads
  indefinitely.

Together: on a fabric where nothing is happening — the normal case — every poll
answers `304`, the browser parses nothing, and the page re-renders nothing.
Tests assert each leg, including that a quiet fabric keeps the same ETag across
repeated refreshes.

### 6b.2 The payload carries what the page reads, and nothing else

The wire snapshot holds every session a router has — one per
client at any scale. The page uses the count. Region member lists are likewise
resolved into edges server-side and never sent.

That is not micro-optimisation: session lists are the one part of this payload
that grows with the fabric, so shipping them would make the page heavier exactly
as a goatnet gets busier, for data nothing renders. A test asserts the payload
stays flat as client count goes from 1 to 500.

## 7. Deployment

Registered in [`ops/serve/services.yaml`](https://github.com/usareur-af/goatcore/blob/main/ops/serve/services.yaml) as a
`container`, so it inherits the Caddy/Traefik + mesh-DNS + isitup path every
hosted service uses. Mesh-internal and admin-gated like the other operator
surfaces; **never publicly exposed**.

Everything below is declared, not hand-run — the point of the wiring is that a
rebuild, a reboot or a rotation does not need anyone to remember this component
exists.

| Piece | Where |
|---|---|
| Image | `ghcr.io/<org>/goat-topology`, built by `.github/workflows/goat-topology-image.yml` **from the repo-root context** (the design-system `include_str!` needs it), digest-pinned in `ops/pins/fleet-pins.yaml` |
| Role | `roles/goat-topology`, one role with two modes; `goat_topology_modes` per group_vars |
| Playbook | `ops/ansible/topology-deploy.yml -e target=<group>` |
| Zenoh ACL | rule `infra-w-topology` + subject `infra-topology` in `roles/goat-zenoh/templates/config.json5.j2` |
| Certificate | a `goat-topology` row in the site cert manifest, minted by `ops/scripts/mint-site-certs.sh` |
| DNS + catalogue | an `isitup_services` row per goatnet; the mesh zone is **generated** from it |

### 7.1 Two prerequisites that fail silently if skipped

**The ACL grant.** The publisher writes `ops/fabric/topology/**`, which nothing
is permitted to do by default. Without the grant every publish is denied and
Zenoh denial is **silent** — the publisher runs happily, the surface
comes up healthy and empty, and the operator debugs the wrong component. The
grant is inert until a cert with the matching CN connects, so it costs nothing
on a goatnet not running the surface.

**The certificate CN.** The ACL matches `CN=goat-topology-trial-peer`. A leaf
with any other CN produces exactly the same silent nothing. The role therefore
runs `openssl x509 -subject` and **refuses to deploy** on a mismatch rather than
starting something that cannot work.

### 7.2 Registration happens in the same change as the deploy

Per F-335: a surface declared ahead of its deploy grades `unknown` and stays
invisible; one registered afterwards is a service nobody can find. So the
`isitup_services` row, the generated mesh-DNS zone and the re-signed name
snapshot all land together. Regenerating a zone invalidates that snapshot's
signature, so it is re-signed with the goatnet's own CA in the same change — an
unverifiable snapshot is worse than a stale one.

### 7.3 Rollout order

**ZAGROS sandbox → verify → zagros-prod.** Not simultaneous: this is a new
component, and the release ladder ([`release-graduation.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/release-graduation.md))
exists because "it worked on the first goatnet" is the cheapest evidence
available before production. The ACL change reaches each goatnet through an
ordinary `zenoh-deploy.yml` re-render; on a goatnet whose ACL is hybrid the
F-258 guard applies, so read it before re-rendering one.

**nova is deliberately not a target** (operator, 2026-09-05). Its
measurements below are retained because they are a record of what was observed
on 2026-09-02, and a measurement does not stop being true when its subject is
retired.

## 8. What this is not

- **Not a control plane.** It reads and renders. It changes no router config, and
  has no write path to any router. Remediation is text an operator acts on.
- **Not a mesh tool.** See §1's scope boundary.
- **Not an alerting system.** Findings are rendered, not paged. If a finding
  class earns paging it belongs in the existing Prometheus/Alertmanager path
  (`mgmt-stack-resilience-and-observability.md`),
  not in a web surface.
- **Not evidence about scale.** It shows what is running. The scaling claims it
  can make are bounded by M-111 and M-112 and nothing more.
