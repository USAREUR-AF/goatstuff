# panoscope frontend

The panoscope web pages: plain HTML, CSS and JavaScript ES modules in
[`web/`](web/), served as files. There is no bundler, transpiler or
framework and no build step. The panoscope Rust binary embeds `web/`
with [`rust-embed`](https://crates.io/crates/rust-embed)
(`../src/embed.rs`). d3 v7 is one vendored file,
[`web/vendor/d3/`](web/vendor/d3/README.md).

npm is used only to run the Playwright tests; `package.json` holds
`@playwright/test` at an exact version.

## Run locally

```bash
cd ops/panoscope
PANOSCOPE_BIND=127.0.0.1:8080 \
PANOSCOPE_ZENOH_ENDPOINT=tcp/127.0.0.1:7447 \
cargo run
```

Open http://127.0.0.1:8080. A debug build (`cargo run` without
`--release`) reads `frontend/web/` from disk on each request, so an
edited file shows up on reload. A release build embeds the files at
compile time.

## Tests

```bash
cd ops/panoscope/frontend
npm ci
npx playwright install chromium
npx playwright test
```

The specs mock `/api/*` and the WebSocket in the browser
(`tests/fixtures/backend.ts`); `tests/serve.mjs` serves `web/` with the
same path mapping as `embed.rs`. No backend runs.

## File layout

- [`web/index.html`](web/index.html) — page shell with header / graph
  SVG / info panel / legend strip. `top.html`, `who.html` and
  `access.html` are the sibling pages served at `/top`, `/who` and
  `/access`.
- [`web/src/main.js`](web/src/main.js) — bootstrap. Builds the
  renderer, starts the legend, opens the WS, wires events to the
  renderer + node-info panel.
- [`web/src/render.js`](web/src/render.js) — D3 force-directed graph
  + packet-dot animation per `pingEdge()` + schema-color edge coding
  + EMA-thickness encoding + role-color node coding.
- [`web/src/ws.js`](web/src/ws.js) — WebSocket client with
  auto-reconnect.
- [`web/src/legend.js`](web/src/legend.js) — bottom-strip legend panel;
  polls `/api/legend` every 5s.
- [`web/src/types.js`](web/src/types.js) — JSDoc typedefs for the
  backend's JSON wire shapes (Snapshot, NodeView, EdgeView, WsEvent,
  Legend, OutgoingMessage). Documentation only; no module imports it at
  run time.
- [`web/src/styles.css`](web/src/styles.css) — dark theme, CSS
  variables for role + schema colors. `pages.css` styles the sibling
  pages.

Module paths in the HTML are absolute (`/src/main.js`) because
`/access/identity` serves `access.html`, where a relative path would
resolve under `/access/`.

## Visualization conventions

| Visual element | Encoding |
|---|---|
| Node color | Role — `source` (orange) / `sink` (blue) / `intermediary` (purple) / `unknown` (gray) |
| Node size | `total_events` cumulative — log scale (very-busy peers don't dominate the canvas) |
| Node label | Vendor name when available; else last 8 chars of the ZID |
| Edge color | Schema family — one colour per family, unknown gray |
| Edge stroke width | Projected EMA EPS — log scale, 0.5–6 px |
| Edge opacity | Fades to ~0.08 when projected EPS approaches zero |
| Packet dot | One per `publish_sampled` event; travels source → sink in 800 ms; colored by the edge's schema family |

## What this does not do (v1)

Per `docs/design/panoscope.md`
§"v1 scope cuts":

- No lineage edges (gated on Block 23 schema discipline).
- No timeline scrub / replay (gated on Block 37 replay substrate).
- No historical aggregates.
- No schema-coverage gauge dashboards.
- No multi-router federation views.
