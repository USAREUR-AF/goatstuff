# zenoh-sandbox web app

goat's warm, explanatory, tacticool browser playground for Zenoh pub/sub — our own
app, hand-written HTML, CSS and ES modules built on `@eclipse-zenoh/zenoh-ts` and served
by nginx. No framework, no build step, no package manager. The upstream zenoh-ts
browser demo is the reference for *what the API does*; this is the *experience*.

Design: [`docs/design/zenoh-sandbox-ux.md`](../../../docs/design/zenoh-sandbox-ux.md) (UX) +
`docs/design/zenoh-sandbox.md` (substrate).

## What it is

A single-page app that lets anyone — engineer or not — *feel* how the fabric moves data:

- **Publish** (one-shot `put`, or a declared publisher with a `Period ms` for auto-publishing).
- **Subscribe** with wildcards (`demo/**`); samples land in a live feed that flashes on arrival.
- **Query & Reply** — declare a **queryable**, send a **get**, watch the **replies** come back.
- A 🌡 "temperature sensor" preset, a plain-English activity trace, and a dark/light toggle
  (tacticool dark by default).

Real Zenoh vocabulary throughout (key expression, put, publisher, subscriber, get, query,
queryable, reply) — introduced where you first touch it, never whitewashed.

The app connects to `/ws` on **whatever host serves it** (same-origin), so on nova-a it reaches
the sandbox router through Traefik with no configuration.

## Source layout

```
app/                    served as it is; this directory is the web root
  index.html            the sandbox page (all static text and panel markup)
  community.html        the "community of opportunity" page
  css/style.css         the tacticool design system (dark + light tokens)
  js/
    main.js             entry of index.html: wires the panels, re-renders on change
    zenoh.js            one Session + the live pub/sub/queryable/transform state
    dom.js              theme toggle, element builder, keyed list renderer
    connect-bar.js  topology-panel.js  publish-panel.js  subscribe-panel.js
    transform-panel.js  query-panel.js  activity-log.js      one module per panel
    community.js        entry of community.html
    community-viz.js    the Canvas 2D scene on that page
  vendor/zenoh-ts/      the one third-party library: zenoh-ts.js, README, licenses
vendor-tool/            regenerates and checks app/vendor/zenoh-ts/zenoh-ts.js
local/                  offline kit: router binary fetch + a stdlib static server
router/                 router image for the local dev compose
```

Text that arrives from the network (topics, payloads, replies) is always set as
text, never parsed as HTML. The Transform panel compiles the function the
visitor types with `new Function` and runs it in their own tab.

## The vendored Zenoh client

`@eclipse-zenoh/zenoh-ts` 1.9.0 (the fleet zenoh version) cannot be loaded by a
browser as published. `vendor-tool/regenerate.sh` turns it and its run-time
dependencies into the single file `app/vendor/zenoh-ts/zenoh-ts.js`; versions,
hashes, licenses and the procedure are in
[`app/vendor/zenoh-ts/README.md`](app/vendor/zenoh-ts/README.md).
`vendor-tool/regenerate.sh --check` fails if the committed file differs from
what the pinned inputs produce; the `vendored-bundle` job of
`.github/workflows/zenoh-sandbox-image.yml` runs it.

## Image

CI builds + pushes `ghcr.io/usareur-af/zenoh-sandbox` from `Dockerfile` here
(`.github/workflows/zenoh-sandbox-image.yml`), per-arch (amd64 + arm64). The
Dockerfile copies `nginx.conf` and the static files under `app/` into an nginx
image. SHA-pin the image in the role per ADR 0859 after each build.

## Local dev

There is nothing to build: edit a file under `app/` and reload the page.

```bash
# once, with internet: fetch the standalone remote-api router for this machine
ops/serve/zenoh-sandbox/local/prep.sh
# router on :10000 + the app on http://localhost:8080
ops/serve/zenoh-sandbox/local/serve.sh
```

On `localhost` the app's default server URL is `ws://localhost:10000`.

With Docker instead (web app + router, no platform binary):
`docker compose -f ops/serve/zenoh-sandbox/docker-compose.dev.yml up --build`,
then open `http://localhost:8099`.

## Why not the trial fabric

This is a throwaway learning sandbox, not the audited vendor fabric. See the comparison
table in `docs/design/zenoh-sandbox.md`.
