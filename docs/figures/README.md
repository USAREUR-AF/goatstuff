# Figures

Captures of the surfaces, all with synthetic or example data. The landing page in `site/` shows them; `site/build.py` copies the ones it references from here.

## How the recordings were made

On one laptop, with no container runtime: a stock `zenohd` 1.9.0 router on loopback, and every surface built from this tree with `cargo build` and run against it. The synthetic fleet is `synth-vendor --roster` with `ops/zenoh-trial-peers/synth-vendor/demo-roster.json`, its endpoint pointed at the local router: 22 peers of one notional org. Each page was driven and recorded in headless Chrome at 1440 by 900 px, about ten frames a second, and encoded with `ffmpeg` (H.264) and `cwebp`. A `.webp` beside an `.mp4` is its poster frame. Where a surface has a light and a dark theme, both were recorded.

| File | Shows | How |
|---|---|---|
| `panoscope.gif` | panoscope (`ops/panoscope/`) on a laptop: synthetic peers on a local Zenoh fabric, then the live stream of a newly published topic | a screen capture, the same file as `docs/assets/panoscope.gif` in [goatcore](https://github.com/usareur-af/goatcore); the README shows it |
| `panoscope-dark.mp4` | panoscope with the synthetic fleet, trashgoat and goat-map's synthetic tracks on the fabric; one peer selected and its topic streaming | the local router and fleet; panoscope's own frontend has one dark theme |
| `flashlight-dark.mp4` | flashlight's directory of the fleet, then the live slice `?vendor=notional` | the local router and fleet |
| `trashgoat-flashlight-dark.mp4` | flashlight on `?key=trashgoat/**` while `goat-trashgoat` runs three control events: a burst of chunked publishes, a stream of UUID-keyed JSON publishes, then a cleanse; the rows with no size are the deletes | control events put on `ops/trashgoat/control/<id>/v1` through the router's REST plugin, in the wire shape of `ops/goat-trashgoat/README.md` |
| `goat-map-dark.mp4` | goat-map (`ops/serve/goat-map/`) drawing ten synthetic tracks with an hour of trail | `tools/synth_tracks.py` with its session opened without TLS (the one change, for a laptop router), `--center 0,0`, no basemap; 40 seconds at two frames a second, played at five times speed |
| `goat-topology-dark.mp4`, `goat-topology-light.mp4` | goat-topology on its shipped example: the graph, router-a's detail, the carve planner | `ops/goat-topology/example/` run natively: three `zenohd` routers with the REST plugin, a `goat-topology publish` beside each, one `goat-topology serve` |
| `stethoscope-dark.mp4`, `stethoscope-light.mp4` | a topic typed into stethoscope's picker, then its checks; every check reports Skipped with the reason, because no portal or audit store is configured | the local binary, no portal; cropped to the page's content column |
| `netvis-dark.mp4`, `netvis-light.mp4` | netvis's site matrix, the SITE-A to SITE-B crossing, a crossing with no rule | the local binary, with `/api/reach` answered by its synthetic test fixture `crates/netvis/tests/fixtures/reach-doc.json`; cropped to the matrix |
| `microscope-dark.webp`, `microscope-light.webp` | microscope's page for one typed selector, with no sources configured: each source says why it was skipped | the local binary |
| `zenoh-sandbox-dark.mp4`, `zenoh-sandbox-light.mp4` | the browser playground (`ops/serve/zenoh-sandbox/app/`) connected to a local router: Listen on `demo/**`, the pretend sensor, the Fahrenheit transform, two publishes on `demo/hello` | the router from `ops/serve/zenoh-sandbox/local/prep.sh` and the page as committed |
| `dataflows-dark.mp4` | the dataflows deck's point-to-point simulation from 2 to 12 systems, then its publish and subscribe simulation from 1 to 25 subscribers | `ops/serve/dataflows/iframes/pattern-icd.html` and `pattern-pubsub.html`, recorded at 1000 px, the width the deck frames them at |
| `trust-sandbox-dark.webp`, `trust-sandbox-light.webp` | the trust sandbox's network admission page (`ops/serve/trust-sandbox/app/concept.html`) | the page served as static files without the sandbox's containers, `__GOATNET__` replaced by `GOATNET` as `goatnet-name.sh` does by default |
| `gym-dark.webp`, `gym-light.webp` | the gym's front page | `python3 ops/gym/generate.py --out <dir>`, served |

Not captured: peerview and netview read JSON that a poller writes from a goatnet's NetBird management API, Prometheus and Traefik log, and the tree has no fixture of it.
