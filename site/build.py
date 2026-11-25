#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Prepare site/ for publishing and check it against the tree. Standard library only.

    python3 -I site/build.py

1. Copies the captures and figures the page shows from the tree into
   site/assets/from-tree/ (git-ignored). The page never carries its own copy.
2. Writes the flow figure from site/figure.py between the figure markers in
   index.html. Under CI (the CI variable is set) a stale figure fails instead.
3. Checks every status chip (data-status="<component>") against the Status
   column of that component's row in docs/RELEASE-CONTENTS.md.
4. Checks that the faint file behind the hero (data-from="<path>") is the
   start of that file, as it is in the tree.
5. Fails when the page or a file under assets/ still holds a placeholder.
6. Runs site/fitcheck.py: in headless Chrome at 1440 and 390 px, no command box
   is cut off, no output box at 1440 px, and the page never scrolls sideways.

The page states no measured numbers, so there is no data-measure check.
"""
import glob
import html
import importlib.util
import os
import re
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SITE = os.path.join(ROOT, "site")
OUT = os.path.join(SITE, "assets", "from-tree")

PAGES = ["index.html"]

# Files copied from the tree into assets/from-tree/, by path relative to the repo root.
# docs/figures/README.md says how each capture was made.
FROM_TREE = [
    "docs/figures/panoscope.gif",
    "docs/figures/panoscope-still.webp",
    "docs/figures/zenoh-sandbox-dark.mp4",
    "docs/figures/zenoh-sandbox-light.mp4",
    "docs/figures/zenoh-sandbox-dark.webp",
    "docs/figures/zenoh-sandbox-light.webp",
    "docs/figures/trust-sandbox-dark.webp",
    "docs/figures/trust-sandbox-light.webp",
    "docs/figures/dataflows-pubsub-dark.webp",
    "docs/figures/gym-dark.webp",
    "docs/figures/gym-light.webp",
    "ops/goat-topology/docs/graph-dark.png",
]

RELEASE_CONTENTS = os.path.join(ROOT, "docs", "RELEASE-CONTENTS.md")
FIG_BEGIN = "<!-- figure:begin (written by site/figure.py; site/build.py fails when it is stale) -->"
FIG_END = "<!-- figure:end -->"


def release_status():
    """Component cell -> Status cell, from the Components table."""
    rows = {}
    with open(RELEASE_CONTENTS) as f:
        for line in f:
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            if len(cells) == 4 and cells[2] in ("Tested in CI", "Built", "Not run"):
                rows[cells[0]] = cells[2]
    return rows


def figure_svg():
    spec = importlib.util.spec_from_file_location("figure", os.path.join(SITE, "figure.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod.render()


def main():
    bad = 0
    os.makedirs(OUT, exist_ok=True)
    for rel in FROM_TREE:
        shutil.copyfile(os.path.join(ROOT, rel), os.path.join(OUT, os.path.basename(rel)))

    index = os.path.join(SITE, "index.html")
    with open(index) as f:
        page = f.read()
    a, b = page.index(FIG_BEGIN) + len(FIG_BEGIN), page.index(FIG_END)
    want = "\n" + figure_svg() + "\n"
    if page[a:b] != want:
        if os.environ.get("CI"):
            print("site/index.html: the figure is stale; run python3 -I site/build.py and commit")
            bad += 1
        else:
            page = page[:a] + want + page[b:]
            with open(index, "w") as f:
                f.write(page)
            print("site/index.html: figure written from site/figure.py")

    rows = release_status()
    for comp, said in re.findall(r'data-status="([^"]+)">([^<]*)<', page):
        comp = html.unescape(comp)
        if comp not in rows:
            print(f"site/index.html: data-status=\"{comp}\" is not a row of docs/RELEASE-CONTENTS.md")
            bad += 1
        elif rows[comp] != said:
            print(f"site/index.html: {comp} says {said}, docs/RELEASE-CONTENTS.md says {rows[comp]}")
            bad += 1

    for src, text in re.findall(r'<pre class="bgcode"[^>]*data-from="([^"]+)">([^<]*)</pre>', page):
        with open(os.path.join(ROOT, src)) as f:
            if not f.read().startswith(html.unescape(text)):
                print(f"site/index.html: the text behind the hero is no longer the start of {src}")
                bad += 1

    served = PAGES + [os.path.relpath(p, SITE) for p in glob.glob(os.path.join(SITE, "assets", "*.*"))]
    for name in served:
        with open(os.path.join(SITE, name), errors="replace") as f:
            text = f.read()
        lines = sorted({text.count("\n", 0, m.start()) + 1 for m in re.finditer(r"REPLACE|\{\{[A-Z_]+\}\}", text)})
        if lines:
            print(f"site/{name}: placeholders on lines {', '.join(map(str, lines[:5]))}")
            bad += 1
    if bad:
        return 1
    print(f"site: {len(FROM_TREE)} files copied from the tree; status chips match docs/RELEASE-CONTENTS.md")
    return subprocess.run([sys.executable, "-I", os.path.join(SITE, "fitcheck.py"), SITE] + PAGES).returncode


if __name__ == "__main__":
    sys.exit(main())
