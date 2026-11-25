# Figures

Captures of the surfaces, all with synthetic or example data. The landing page in `site/` shows them; `site/build.py` copies them from here.

| File | Shows | Source |
|---|---|---|
| `panoscope.gif` | panoscope (`ops/panoscope/`) on a laptop: synthetic peers on a local Zenoh fabric, then the live stream of a newly published topic | a screen capture, the same file as `docs/assets/panoscope.gif` in [goatcore](https://github.com/usareur-af/goatcore) |
| `panoscope-still.webp` | the frame one second into `panoscope.gif`, shown instead of it under reduced motion | `ffmpeg -ss 1 -i panoscope.gif -frames:v 1`, then `cwebp` |
| `zenoh-sandbox-dark.mp4`, `zenoh-sandbox-light.mp4` | the browser playground (`ops/serve/zenoh-sandbox/app/`) connected to a local router: Listen on `demo/**`, the pretend sensor, the Fahrenheit transform, two publishes on `demo/hello`; 13 seconds | the router from `ops/serve/zenoh-sandbox/local/prep.sh` and the page as committed, driven and recorded in headless Chrome at 1440 by 880 px, ten frames a second, in the page's dark and light themes |
| `zenoh-sandbox-dark.webp`, `zenoh-sandbox-light.webp` | the last frame of each recording, used as its poster | the same recording |
| `trust-sandbox-dark.webp`, `trust-sandbox-light.webp` | the trust sandbox's network admission page (`ops/serve/trust-sandbox/app/concept.html`) | the page served as static files without the sandbox's containers, `__GOATNET__` replaced by `GOATNET` as `goatnet-name.sh` does by default, headless Chrome at 1440 px |
| `dataflows-pubsub-dark.webp` | the dataflows deck's publish and subscribe simulation (`ops/serve/dataflows/iframes/pattern-pubsub.html`) set to twelve subscribers | headless Chrome at 1000 px, the width the deck frames it at |
| `gym-dark.webp`, `gym-light.webp` | the gym's front page | `python3 ops/gym/generate.py --out <dir>`, served, headless Chrome at 1440 px |
