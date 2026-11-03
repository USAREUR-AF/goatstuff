# panoscope view model — one rule for what the operator sees

Panoscope shows a fabric with more topics than a screen can hold, so something
must decide what to draw. That decision now lives in one place on the server
and is described here. The browser draws what it is given.

Canonical for panoscope's complexity management. The surface itself —
interaction, side panel, stream pane, identity chain, deployment — is
panoscope.md. The load-protection rule this operates under is
surface-overload-protection.md
([ADR 1123](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1123-client-demand-must-not-amplify.md)): a viewer's demand
never becomes fabric load.

**Status:** built on branch `track/panoscope-view-model` (PR #1161) and proven
on zagros-dev 2026-09-15 (`sha-a74ab97`, measurements below). Not yet on the
higher rungs: those need the operator.

## What changed

Before, five mechanisms each folded one shape of fan-out, three on the server
and two in the browser, and they disagreed: a topic could be inside a server
fold, a client fold, or neither, depending on which threshold it crossed
first. The counts in the HUD, the topics in the side panel and the elements on
the canvas came from different reductions of the same data, so they did not
add up. Search read the drawn node set, so a topic inside a fold could not be
found by a middle substring unless its group happened to carry the keys
client-side. Every request rebuilt the picture from the ingest state.

Now:

- **One model.** A background task rebuilds `FabricModel` from the ingest
  state once a second and publishes it as an `Arc`. Requests read that model
  and never take an ingest lock.
- **One grouping rule.** A publisher's topics form a segment trie; same-shape
  siblings become a family, and whole subtrees collapse only to fit a budget.
  There are no per-shape special cases.
- **One assembly per viewer.** Scope, window, filters, organisation collapse,
  grouping and the element cap are applied in that order, on the server, to
  produce exactly the elements the browser draws.
- **One place gestures land.** Opening a group, folding a peer, collapsing an
  organisation and every toolbar control are edits to a small parameter object
  the browser sends with each request.

Everything a viewer can see is decided from their scope on every request, so
the model being shared is not a way around
RBAC ([ADR 1073](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1073-rbac-adherence-principle.md)).

## The pieces

Code lives in `ops/panoscope/src/view/`.

| File | What it owns |
|---|---|
| `model.rs` | `FabricModel`: peers, topics, per-peer facts, build statistics, a content fingerprint. `build_model` runs off-lock in `spawn_blocking`. |
| `subs.rs` | `SubscriptionIndex` — which peers subscribe to which topic, updated incrementally as topics and subscriptions change rather than rematched per build. |
| `group.rs` | The grouping rule. `group_topics(publisher, topics, budget, pinned)` → units. Memoised per (publisher, topic set, budget, pins). |
| `assemble.rs` | Per-viewer assembly: visibility, organisation collapse, grouping, edges, totals, the element cap, the entity tag. |
| `service.rs` | The builder task, and the group / search / reveal / topics answers served from the model. |

### The model

`build_model` takes copies of topology, flow, aliases and the latest-format map
under their locks — milliseconds — and then computes with no lock held. This
matters because the data tail takes those locks for write on every sample:
holding them for the length of a build stalls ingest, which is how panoscope
stopped draining its Zenoh session and the router closed its transport
(the 2026-09-14 sequence recorded in panoscope.md).

The model carries a **generation** that advances only when a content
fingerprint changes, so an idle fabric answers `304 Not Modified` instead of
re-sending a picture nobody changed.

**The cadence follows the build's own cost.** A build happens whether or not
anyone is looking, so on a large fabric a fixed one-second tick spends most of
a core on a picture nobody asked for: on zagros, 58,096 topics take 580-607 ms
to build (`canonical` 170-181 ms, `topics` 169-176 ms, `index` 169-175 ms,
`hygiene` 53 ms — measured 2026-09-16 on the mgmt host), and a later build of
the same fabric measured 1.07-1.17 s. `next_model_tick` waits
`max(1 s, 2 x the last build)` after a build finishes, so the builder takes
about a third of the wall clock however large the fabric grows, and a slow
build rests proportionally rather than running back to back.
Every response carries `built_at_ms`, so how old the picture is remains
visible rather than implied.

### The grouping rule

A publisher's topics form a trie of key segments. Two steps decide what is
drawn:

1. **Families.** At every node, children with the same *shape* are one class —
   shape being the set of a child's own children's shapes, so a chunk
   directory with 5 parts and one with 30 are the same shape and families
   compose bottom-up. A class of more than `FAMILY_MIN` (7) members folds into
   one group. A directory class whose names are words (`sensor`, `tak`,
   `imagery`) needs more than `WORD_DIR_FAMILY_MIN` (16), so eight distinct
   subsystems under one anchor stay visible while `cam0…cam7` folds.
2. **Budget.** If the publisher still draws more than `PUBLISHER_BUDGET` (40)
   units, whole subtrees collapse — deepest level first, largest first within
   a level — until it fits. The top of the tree stays readable longest. If the
   top level alone exceeds the budget, the publisher becomes one group.

Group ids are stable strings the browser sends back: `grp:<publisher>:pub`,
`grp:<publisher>:sub:<path>`, `grp:<publisher>:fam:<shape>:<path>`.

**Opening** a group means adding its id to `expand`; the rule then must not
form it, and its contents are grouped again one level down. Opening is exempt
from the budget — it is an explicit request to see more — but bounded:
`MAX_OPEN_UNITS` (2,000) per publisher, above which the pins are ignored and
the group stays closed, with the inspector listing it instead.

Grouping depends on the publisher, its topic keys, the budget and its own pins
— not on rates — so it is memoised. Without the memo, 49,437 topics cost 372 ms
per call in a release build, on every request.

### One assembly per generation

Assembly is per viewer because scope and parameters are — but most viewers ask
the same question, and on zagros one assembly costs 160-210 ms, so ten viewers
polling every 1.5 s spent more than a core re-deriving identical bytes. The
serialised body is therefore cached under everything the answer depends on:
generation, scope, parameters, and the minute for a windowed view. The key is
those values themselves, not the entity tag: the tag is a 64-bit unkeyed hash
of client-supplied strings, and a collision here would serve one viewer a body
cut to another viewer's scope (ADR 1073). A body can only be served to a
request that would have produced it, and the whole map is dropped when the
generation moves, so nothing outlives the model it came from. A body that
fails to serialise answers 500 and is never cached. Two viewers arriving together may
both build — the build runs outside the lock, because holding it across a
200 ms assembly would stall every other request in the process.

### Per-viewer assembly

`assemble(model, scope, params, now)` returns the response:

1. **Visibility** — the viewer's scope (peer facts and key), the history
   window, and the SOURCE / INFRA / TEST filters.
2. **Organisation collapse** — a collapsed organisation is one node with
   outbound edges only.
3. **Grouping** — per visible publisher, at its budget (1 when the peer is
   folded or SIMPLIFY is on).
4. **Edges** — publish edges from a peer to each pin or group, subscribe edges
   from those to each *visible* subscriber, so a group keeps its subscribers
   connected rather than hiding the relationship.
5. **Element cap** — if peers + pins + groups + organisations + edges exceed
   `MAX_ELEMENTS` (5,000), every publisher is regrouped at budget 8, then 1.
   If opened groups are what breaks the cap, they are dropped. Whatever was
   done is reported in `truncated`, and the browser says so on the canvas.

Totals count what the viewer *can see* (9,448 topics), not what is drawn (a
group of 1,204 counts 1,204), so the HUD and the canvas no longer disagree.

The response's entity tag is the generation, the viewer's scope key, the
parameters, and — for a windowed view — the current minute, because a topic
ages out of a window with no change to the model.

## The wire

| Endpoint | Answers |
|---|---|
| `POST /api/view` | The whole view for these parameters. Honours `If-None-Match`. |
| `POST /api/view/group` | One group's members, filtered and paged server-side, from the same grouping the canvas drew. `404` when the id is not a group in that viewer's view. |
| `POST /api/view/search` | Visible topics matching a substring, each with the element that draws it. |
| `POST /api/view/reveal` | The groups to open so one topic is its own pin. |
| `GET /api/view/build` | Build statistics. Names no topic. |
| `GET /api/topics` | Every visible topic, one row per key, never grouped — `goat flow`'s read. Filterable by key expression. |

Parameters (`ViewParams`), all optional: `window_hours`, `detail`
(`normal` / `simplified`), `expand`, `fold_peers`, `collapse_orgs` (`"*"` = all),
`source`, `infra`, `test`.

## The browser

`view-client.ts` holds the parameters, persists them in `localStorage`
(`panoscope.viewParams.v1`, seeded once from the pre-view-model window and
SIMPLIFY settings) and makes the requests. `render.ts` maps a response onto
nodes and edges and folds nothing: `setView` replaces `setSnapshot`, and the
last-good-frame guard is unchanged — an apply that throws restores the previous
frame and the page shows the error rather than blanking.

Gestures:

| Gesture | Parameter edit |
|---|---|
| Double-click a group | `expand` += its id (or the inspector, when it is too large to draw) |
| Double-click a peer | `fold_peers` toggles that peer |
| Double-click a hull or organisation node | `collapse_orgs` toggles that organisation |
| COLLAPSE ALL / EXPAND ALL | `collapse_orgs = ["*"]` / everything cleared |
| SOURCE / INFRA / TEST / SIMPLIFY / window | the matching field |
| Search commit on a grouped topic | `/api/view/reveal`, then its groups are opened |

One fetch is in flight at a time; a parameter change while one is running
refetches when it lands, and the superseded answer is dropped. The WebSocket is
opened with `snapshot=0` — the page draws from `/api/view`, so the server does
not build the opening snapshot it would discard. Packet dots still animate from
`publish_sampled` events: a live publish attributes to its own pin, or to the
group whose selector covers the key.

## What it costs

Measured on the operator laptop, release build, against a copy of zagros's shape
on 2026-09-15 (9,448 topics, 206,954 publisher–subscriber links);
best of several runs on a loaded machine, so read them as an order of
magnitude, not a benchmark.

| | Before | After |
|---|---|---|
| Build, warm | 305 ms | 34 ms |
| Per-request work | a full build | 23 ms, including JSON |
| Response body | 177 KB (folded snapshot) | 270 KB (elements + totals) |

The build is once a second whatever the viewer count; the per-request work is
what each viewer adds. The response is larger because it carries the elements
and their aggregates rather than raw edges the browser then reduces.

### On zagros, 2026-09-16

The first deploy to the preprod rung, and the first measurement at real scale.
Same load both sides — three browser tabs still running the previous bundle,
so both builds were serving the same requests:

| | `sha-8eb895d` | `sha-74b6fbc` |
|---|---|---|
| CPU | 97.6% | ~78% |
| Memory | 2.8 GiB | 1.0-1.3 GiB |
| Response | 832 KB | 188 KB |
| One request | 3.33 s cold, 20 ms from cache | 160-210 ms |
| `/api/topics` | — | 58,096 rows in 50 ms |

What this measurement changed: at 58,096 topics a build costs ~600 ms, not the
37 ms the 6,600-topic dev fabric predicted, so a one-second cadence would have
left ~60% of a core spent at rest and each viewer adding ~13%. The pacing rule
and the per-generation response cache above are that finding's answer. Read
the dev numbers below as the shape of the change, and these as its scale.

### On zagros-dev, 2026-09-15

Measured on the zagros-dev management host against the same publisher shape both
times: 6,600 topics from one publisher as `<anchor>/viewproof/stream/<id>/chunk/<n>/<part>`,
26 other peers, no other traffic. The candidate is `sha-a74ab97`; the build it
replaced is the fleet pin `sha-bf6f304`.

| | `/api/snapshot`, fleet pin | `/api/view`, candidate |
|---|---|---|
| One request | 330 ms, 24.8 KB | 6–10 ms, 13.7 KB |
| Unchanged model | — | 304 in 0.4 ms, no body |
| CPU, ten viewers polling every 1.5 s | 140–300% | 6–9% |
| CPU, no viewer | ~0% | ~4% (the 30 ms build each second) |

The idle cost is new and deliberate: the model is built whether or not anyone
is looking, which is what makes the per-viewer cost small and flat.

That fleet pin predates #1126, so the comparison above is against a build that
rebuilt the whole snapshot on every request. Against **#1126** (`sha-8eb895d`,
what zagros runs), which shares one build across viewers, the same zagros-dev host
with 20 subscribers holding `<anchor>/**` — 6,600 topics, 132,000
publisher-subscriber links, the shape that made zagros's snapshot expensive:

| | #1126 | view model |
|---|---|---|
| One build | 0.61–0.65 s | 37 ms |
| One request | the build, when the cache expired; ~0.5 ms otherwise | 13–20 ms, always on a model at most 1 s old |
| CPU, ten viewers at 1.5 s | 3.6–62%, in spikes as the cache expired | 3.6–5.4%, flat |
| What it drew | one edge: the outlier rollup collapsed the publisher whole | one family group, 20 subscribe edges, 70 elements |

#1126 made the per-viewer cost bounded by build rate; it did not make a build
cheap, and it kept the v1 folds, so at this shape the picture it serves is a
single edge with the subscriber relationships gone. The view model makes the
build cheap and keeps the relationships.

What the view drew, and what the old build drew, for those 6,600 topics: one
family group (28 elements) against 25 folded edges keyed per chunk *part*
(`chunk/*/000000`, `chunk/*/000001`, …) — the old template fold wildcarded the
wrong axis for this shape. Opening the group one level drew its 240 chunk
directories (602 elements). The rest of the acceptance on the live surface:
group members paged and filtered server-side (25 of 6,600 for `chunk/0007`),
a middle-substring search found a topic inside the group and named the element
drawing it, reveal opened two levels and drew the pin, `/api/topics` returned
all 6,600 rows ungrouped in 11 ms, and the browser drew each of those states
with no console error from the page.

## Mechanisms this replaces

Each of these is removed, not layered over. The history of why each existed is
in panoscope.md under the dates given.

| Mechanism | Was | Now |
|---|---|---|
| Server template fold (`fold_group_templates`) | Per-publisher varying-segment wildcards, skipped sink-resolved edges | Families, which do not care whether a topic has subscribers |
| Server outlier rollup (`rollup_outlier_publishers`) | Whole publisher rolled up past 40 topics and 1,000 edges | Budget collapse, deepest-first |
| Aggressive rollup (`aggressive=true`) | Server bundle per publisher | `detail: "simplified"` (budget 1) |
| Client singleton fold (threshold 7) | Per (peer, namespace) pile | Families |
| Client per-publisher fanout fold (threshold 40) | `(auto)` bundle per publisher | The budget |
| Client manual fold | `(manual)` sentinel group | `fold_peers` |
| Client vendor collapse | Renderer-local map | `collapse_orgs` |
| `publishes` fold on a peer | Display strings on the wire | The drawn units' labels |

`/api/snapshot`, `/api/snapshot/expand` and the WebSocket opening snapshot stay
served while pages and `goat ui` still read them; removing them is a separate
change once nothing does.

## Lenses the browser keeps

Two things stay in the browser because they are views over what the server
already served, not decisions about what to serve:

- **Component isolation** (`f`): draw only what the selection is connected to.
  Reachability over the drawn edges, recomputed on every view, session-local.
  It costs no request and composes with whatever grouping the server chose.
- **What the page knows yet**: until a response lands, the HUD reads `—` and
  the welcome card says it is waiting. An empty canvas is a claim about the
  fabric, and the page may only make it after the server has answered.

## Limits

- **`goat ui` still reads `/api/snapshot`** for its peer pivot, so that build
  still runs when `goat ui` is used. `goat flow` reads `/api/topics` and falls
  back to `/api/snapshot` only against a panoscope that predates it.
- **The element cap is a cap, not a layout.** At 5,000 elements a canvas is
  still dense; the cap bounds cost and the banner is honest about it, but the
  readable-at-scale question (organisation-level grouping as the primary
  object) is unchanged and remains in backlog.md.
- **Selectors are display attribution.** A group's `selector` decides which
  element a live publish animates on. It is never an authorization input.
- **Numbers above are laptop measurements**, not a live goatnet. The proving
  run on zagros-dev is what turns them into a claim about panoscope in service.
