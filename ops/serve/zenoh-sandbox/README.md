# zenoh-sandbox web app

goat's warm, explanatory, tacticool browser playground for Zenoh pub/sub — our own
Vite + Vue 3 app built on `@eclipse-zenoh/zenoh-ts`, served by nginx. Replaces the earlier
"vendor the upstream Nuxt demo" approach: that demo is the reference for *what the API
does*; this is the *experience*.

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

## Source layout (`app/`)

```
app/
  index.html            entry
  vite.config.ts        wasm + top-level-await plugins, esnext target
  package.json          vue + @eclipse-zenoh/zenoh-ts@1.9.0
  src/
    main.ts             mount + restore theme
    zenoh.ts            the reactive store: one Session + live pub/sub/queryable state
    App.vue             layout, hero, theme toggle
    style.css           the tacticool design system (dark + light tokens)
    components/         ConnectBar, PublishPanel, SubscribePanel, AskAnswerPanel, ActivityLog, Explainer
```

## Build

CI builds + pushes `ghcr.io/usareur-af/zenoh-sandbox` from `Dockerfile` here
(`.github/workflows/zenoh-sandbox-image.yml`), per-arch (amd64 + arm64). The
Dockerfile does a Node build of `app/` (Vite → static) → nginx. The published
`@eclipse-zenoh/zenoh-ts@1.9.0` ships the prebuilt keyexpr WASM, so no Rust toolchain is
needed. SHA-pin the image in the role per ADR 0859 after each build.

## Local dev

```bash
# 1) a local remote-api router on :10000 (macOS arm64 shown):
#    download zenoh-ts-1.9.0-<arch>-<os>-standalone.zip from eclipse-zenoh/zenoh-ts releases
./zenoh-bridge-remote-api --ws-port 10000

# 2) the app:
cd ops/serve/zenoh-sandbox/app
npm install
npm run dev            # http://localhost:5173 ; default server URL ws://localhost:10000
```

Or build the container: `docker compose -f ops/serve/zenoh-sandbox/docker-compose.yml up --build`.

## Why not the trial fabric

This is a throwaway learning sandbox, not the audited vendor fabric. See the comparison
table in `docs/design/zenoh-sandbox.md`.
