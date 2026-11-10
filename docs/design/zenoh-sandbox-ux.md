# zenoh-sandbox UX — from dev-tool demo to a joyful way to *feel* pub/sub

The first cut of the sandbox (`zenoh-sandbox.md`) served the upstream
zenoh-ts browser demo verbatim. It works, but it's a four-quadrant developer console:
jargon-first ("key expression", "consolidation: Auto"), a scrolling log at the bottom, no
explanation, and — most tellingly — the *machinery* (a message travelling
publisher → router → the subscribers who care) is completely invisible. It demonstrates the
API; it teaches and delights nobody.

This doc is the design for replacing that payload with **our own app** — same isolated
router, same `remote_api` WebSocket, same CI image — built so a newcomer (engineer or not)
comes away having *felt* how the fabric moves data.

**Tone:** warm and a little wondrous. Gentle motion, friendly plain-language voice, a
quiet "whoa, I can see the messages move." Not cutesy, not gamified, not a toy.

## What stays, what changes

| | v0 (shipped) | this evolution |
|---|---|---|
| Router + `remote_api` plugin | unchanged | unchanged |
| Traefik `/` (SPA) + wss `/ws` | unchanged | unchanged |
| CI image `ghcr.io/usareur-af/zenoh-sandbox` | unchanged shape | unchanged shape |
| Web payload | vendored upstream Nuxt demo | **our app**, source in `ops/serve/zenoh-sandbox/app/` |
| Client library | `@eclipse-zenoh/zenoh-ts@1.9.0` | same (we build *on* it, not *around* it) |

The upstream demo remains the reference for *what the API can do*; we own the *experience*.

## Stack

Hand-written HTML, CSS and ES modules, served by nginx as they are in the tree
(`ops/serve/zenoh-sandbox/app/`: `index.html`, `community.html`, `css/`, `js/`). There is
no framework, no build step and no package manager. `js/zenoh.js` holds the session and
the state of everything declared on it; each panel is one module that renders from that
state when it changes.

The one third-party browser library is the Zenoh client, `@eclipse-zenoh/zenoh-ts@1.9.0`.
It is a single vendored file, `app/vendor/zenoh-ts/zenoh-ts.js`, committed with the
package versions, tarball hashes and license texts beside it. The package as published
cannot be loaded by a browser directly (it imports its dependencies by bare name and its
key-expression code is a WebAssembly module in bundler format), so
`ops/serve/zenoh-sandbox/vendor-tool/regenerate.sh` produces the file: it downloads the
pinned tarballs, verifies each SHA-512 and bundles them into one ES module with the
WebAssembly embedded, using esbuild's Go API. The output is byte-for-byte reproducible,
and CI rebuilds it on every change to the sandbox and fails if the committed file
differs.

## Three pillars (built as phases)

### Phase 1 — joyful, intuitive, explanatory

The experience layer. Sets the visual + voice language the other two inherit.

- **Narrative, not a grid.** The page tells a small story: *someone publishes to a topic;
  anyone subscribing who cares hears it; a queryable answers questions on demand.* Each
  concept is introduced where you first touch it, in plain words, with a "🛈 what's a key
  expression?" reveal (topic + `**` wildcard, explained once).
- **Plain language with the real term one tap away.** "Topic" up front; "(Zenoh calls this
  a *key expression*)" on hover. Never punish a newcomer with vocabulary before meaning.
- **Preset scenarios — "Try this".** One click starts a fake temperature sensor publishing
  `demo/sensor/temp` ~1/s; a subscriber card lights up and a sparkline draws. Kills the
  blank-canvas freeze.
- **Felt feedback.** A soft pulse/flash when a message actually lands; friendly empty
  states; a guided first-run that walks pub → sub → get → queryable, one gentle step at a
  time.

### Phase 2 — subscribe → transform → republish (the imaginative feature) — **BUILT**

A **Transform card**: subscribe to topic A, apply a transformation, republish to topic B; a
downstream subscriber on B sees the *new* format live. Presets (°C→°F, JSON→CSV, UPPERCASE,
wrap-envelope, passthrough) **prefill an editable `msg => …` code box** — so the preset is a
starting point you can rewrite, not a black box (the function runs in `new Function` — fine
in a throwaway, no-real-data sandbox). This makes the goat thesis tangible — the fabric isn't
a dumb pipe, it's where *one producer's format becomes another consumer's format* without
either side knowing about the other. A hands-on, in-browser cousin of the canonical-pivot /
polygloat translator story.

Built + validated end-to-end (sensor `{"celsius": …}` → °C→°F transform → a downstream
subscriber on `demo/sensor/fahrenheit` reads `{"fahrenheit": …}`). Loop guard: refuses
input-topic == output-topic.

### Phase 3 — visualize the machinery, brokerless + topologically — **BUILT**

"The fabric, live": an SVG view that teaches Zenoh's headline property — **there is no
broker in the middle**. Producers sit on the left, consumers on the right, transforms (which
are both) in the centre, queryables below. We compute the real key-expression match graph in
the browser and draw a **direct edge for every match** — publisher → each subscriber whose
pattern matches; that *is* the routing, drawn point-to-point, not funnelled through a hub.
Every real delivery fires a flow event carrying the **concrete delivered topic**, and we
animate a packet **straight from the producer of that message to the peer that received it**
— never through a centre. A publish/reply flashes the emitting peer; declared nodes pulse.
`requestAnimationFrame`-driven and fed entirely by observed events, so it never lies.

A deliberately honest caveat sits in the panel: in *this browser sandbox* the bytes do relay
through the remote-api router (browsers can't peer directly), but Zenoh's model — and what
the viz teaches — is brokerless peer-to-peer delivery.

This is the pedagogical cousin of **panoscope** (`panoscope.md`) — same
spirit (see the topology), opposite purpose: panoscope is operational over the real fabric;
this is self-contained, animated, built to *teach*. (v1 draws from what the visitor declares
locally; pulling the wider peer set from the router admin space `@/**` is a future option.)

## The "community of opportunity" page (`/community.html`)

A **separate page** on the same subdomain — reachable from the sandbox topbar
("✦ THE BIGGER PICTURE"), not jammed into the sandbox app — that answers *why the
fabric matters*, not *how Zenoh works*. Where the sandbox is hands-on and literal,
this is the vision surface: the shape of the opportunity a shared fabric opens.

Its centerpiece is a hand-built **Canvas 2D scene** (`app/src/community-viz.ts` —
no libraries, no icon assets; procedural so it re-themes live with the design
tokens, looking right in both dark and light). It tells one story in two states,
cross-faded by an eased transition and auto-revealing on load (~5.4 s dwell on
"today" first, so the friction reads, then it settles onto the fabric):

- **Today (the legacy network).** A transit diagram in the **Harry Beck / London
  Underground** style. Each vendor system is its own colored **line**; stations are
  stages of generating, receiving, and using data (a data pipeline, with the human
  friction made literal — Detect / Collect / Ingest … Decode / Exploit / Fuse …
  Assess / Brief). To move data across systems you ride one line, **get off, and walk
  a corridor** to another — the out-of-station interchanges are labeled RE-KEY,
  SWIVEL CHAIR, EMAIL EXPORT, PDF HANDOFF, SNEAKERNET, PHONE CALL — and some data is
  **dropped in the corridor** (a ✕). Trains run and dwell; little carriers walk the
  hand-offs. The whole thing is interlocked and clunky: rails, cars, passengers,
  stops. Dark = a glowing "night map"; light = the classic cream-paper tube map.
- **On the fabric (the community of opportunity).** Leave the lines behind. Capabilities
  dock onto one shared, glowing **fabric ring** and each becomes a focused node doing
  its one thing. Value flows **peer-to-peer** across the shared medium (curved chords
  colored by their source), contribution returns as a compensation shimmer, and the
  whole community brightens in a slow pulse.

**Brokerless on purpose** (same discipline as the Phase-3 topology): there is no hub
box at the center of the fabric — the ring *is* the shared medium, and value travels
directly between peers across it.

Aligned with the closed-core / **edge-attachment** growth story
(`../project/edge-attachment-growth-strategy.md`):
products attach at the edge, do the one thing they're great at, and the community
compounds the rest. Products shown are **notional/illustrative** — no real-vendor
claims, no invented metrics.

Built as a second page (`community.html` → `js/community.js` → `js/community-viz.js`)
sharing the same `css/style.css` design system and the same image; nginx serves it
directly (`try_files $uri`).

## Non-goals

- Not the trial fabric (still no auth/audit/peering — see `zenoh-sandbox.md`).
- Not a replacement for panoscope/stethoscope/flashlight (those are operational surfaces).
- Not gamified. Wonder, not points.

## Build + deploy

`ops/serve/zenoh-sandbox/Dockerfile` copies the static files under `app/` into an nginx
image; CI publishes the image; the `zenoh-sandbox` Ansible role deploys it; SHA-pin per
ADR 0859. nginx tells the browser to revalidate `.js` and `.css` on every load and never
to cache `.html`, because the file names carry no content hash. Iterate phase by phase,
each a normal PR.

## Offline / field demo

For demoing on a laptop with **no internet and no Docker** (Docker Desktop is too heavy —
it swaps), `ops/serve/zenoh-sandbox/local/` runs the whole sandbox as a couple of native
processes: the static app, served from `app/` as committed by a stdlib-only python
server, plus the standalone `zenoh-bridge-remote-api` router on `:10000` (the app's
localhost default). One online `prep.sh` (fetch the matching router binary, gitignored)
ahead of time; then `serve.sh` runs fully offline. Same app and same isolated/disposable model as
`sandbox.nova` — only the delivery differs. See `ops/serve/zenoh-sandbox/local/README.md`.

## References

- `zenoh-sandbox.md` — the substrate (router, plugin, routes, isolation)
- [`zenoh-trial-fabric.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/zenoh-trial-fabric.md) — what the sandbox is *not*
- `panoscope.md` — the operational topology viz this Phase-3 viz is a teaching cousin of
- upstream [`eclipse-zenoh/zenoh-ts`](https://github.com/eclipse-zenoh/zenoh-ts) — reference for the API surface
