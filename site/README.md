# site

The landing page published to GitHub Pages by `.github/workflows/pages.yml`, at
https://usareur-af.github.io/goatstuff/. One static page: no framework, no external
requests, no web fonts, light and dark.

## Files

| File | What it is |
|---|---|
| `index.html` | the page |
| `assets/site.css` | its styles, light and dark (`prefers-color-scheme`) |
| `assets/site.js` | the copy button and the first screen's stage |
| `assets/icon.svg` | the page icon |
| `assets/from-tree/` | git-ignored; `build.py` copies into it every capture the page references |
| `figure.py` | draws the flow figure, wide and narrow, which `build.py` writes into `index.html` |
| `build.py` | copies `assets/from-tree/`, writes the figure, checks the status chips and the file behind the hero, runs `fitcheck.py` |
| `fitcheck.py` | loads the page in headless Chrome at 1440 and 390 px; fails when a command box is cut off or the page scrolls sideways |

## Preview

```sh
python3 -I site/build.py
python3 -m http.server 8000 --directory site    # then open http://127.0.0.1:8000/
```

## What build.py checks

The page states no measured numbers.

| Check | Against |
|---|---|
| each `data-status="<component>"` chip | the Status column of that component's row in `docs/RELEASE-CONTENTS.md` |
| the faint text behind the hero, `data-from="<path>"` | the start of that file, `ops/zenoh-trial-peers/synth-vendor/demo-roster.json` |
| the figure between the `figure:begin` and `figure:end` markers | the output of `figure.py`; locally it is rewritten, under CI a stale figure fails |

## The first screen

The stage shows recordings from `docs/figures/` (how each was made is in
`docs/figures/README.md`), one surface at a time: panoscope, flashlight, goat-map, the
browser playground, goat-topology and dataflows, each for the length of its recording. The
prompt line above the wordmark shows the command that runs the surface on screen. The stage
pauses under a pointer or keyboard focus and while the browser tab is hidden, and starts
playing only after the page has loaded. Gallery recordings play while they are on screen.
With reduced motion nothing advances or plays; each recording shows its poster frame.
Without JavaScript the panoscope poster shows.
