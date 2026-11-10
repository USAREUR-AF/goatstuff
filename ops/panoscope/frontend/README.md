# panoscope frontend

Single-page Vite + TypeScript + D3 v7 SPA. Built into `dist/` and
embedded into the panoscope Rust binary via [`rust-embed`](https://crates.io/crates/rust-embed)
in 82Q.6.

## Quick start

```bash
cd ops/panoscope/frontend
npm install
npm run dev   # http://localhost:5173 — proxies /api + /ws to :8080
```

Run the backend in parallel:

```bash
cd ops/panoscope
PANOSCOPE_BIND=127.0.0.1:8080 \
PANOSCOPE_ZENOH_ENDPOINT=tcp/127.0.0.1:7447 \
cargo run --release
```

## Build

```bash
npm run build       # → dist/
npm run typecheck   # tsc --noEmit
```

## File layout

- [`index.html`](index.html) — page shell with header / graph SVG /
  info panel / legend strip.
- [`src/main.ts`](src/main.ts) — bootstrap. Builds the renderer, starts
  the legend, opens the WS, wires events to the renderer + node-info
  panel.
- [`src/render.ts`](src/render.ts) — D3 force-directed graph
  + packet-dot animation per `pingEdge()` + schema-color edge coding
  + EMA-thickness encoding + role-color node coding.
- [`src/ws.ts`](src/ws.ts) — WebSocket client with auto-reconnect.
- [`src/legend.ts`](src/legend.ts) — bottom-strip legend panel; polls
  `/api/legend` every 5s.
- [`src/types.ts`](src/types.ts) — TypeScript types matching the
  backend's JSON wire shapes (Snapshot, NodeView, EdgeView, WsEvent,
  Legend, OutgoingMessage). The compile-time check that frontend +
  backend agree on the wire.
- [`src/styles.css`](src/styles.css) — dark theme, CSS variables for
  role + schema colors.

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

## Initial frontend scope

The initial frontend scope excluded:

- No lineage edges (gated on Block 23 schema discipline).
- No timeline scrub / replay (gated on Block 37 replay substrate).
- No historical aggregates.
- No schema-coverage gauge dashboards.
- No multi-router federation views.
