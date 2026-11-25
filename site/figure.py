#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Draw the page's one figure: who puts data on the fabric and which surfaces read it.

Fixed layout, no auto-layout. Two drawings of the same content: a wide one
(three columns, read left to right) and a narrow one for phones (one column,
read top to bottom). Colors come from the page's CSS variables through classes,
so light and dark follow prefers-color-scheme. Standard library only.

Every row is taken from the tree; the source of each is named in ROWS_SOURCES
below and in site/README.md.

    python3 -I site/figure.py          # prints both SVGs
"""
from html import escape

# (name, what it puts or reads, both_ways)
PUBLISHERS = [
    ("synthetic vendors", "notional/**", False),          # ops/zenoh-trial-peers/synth-vendor/demo-roster.json
    ("health publishers", "ops/<domain>/health/…", False),  # ops/zenoh-trial-peers/*-health
    ("trashgoat", "trashgoat/<slug>/…", False),          # ops/goat-trashgoat/README.md
    ("goat-topology publish", "ops/fabric/topology/<site>/<node>", False),  # ops/goat-topology/README.md
    ("transform-shim", "one format in, another out", True),  # docs/design/format-transformers.md part 1
]
SURFACES = [
    ("panoscope", "subscribes; queries when asked", True),   # README.md table, ops/panoscope/README.md
    ("goat-topology serve", "ops/fabric/topology/**", False),  # ops/goat-topology/README.md
    ("flashlight", "only the slice its URL names", False),     # docs/design/flashlight.md
    ("stethoscope", "one target its URL names", False),        # docs/design/stethoscope.md
    ("goat-map", "CoT in, GeoTrackV1 out and back", True),     # ops/serve/goat-map/README.md
]

ARROW_DEFS = (
    '<defs><marker id="{p}h" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" '
    'orient="auto-start-reverse"><path d="M0 0L10 5L0 10z" class="fhead"/></marker></defs>'
)


def box(x, y, w, h, name, detail, cls="fbox"):
    return (
        f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="6" class="{cls}"/>'
        f'<text x="{x + 14}" y="{y + 26}" class="fname">{escape(name)}</text>'
        f'<text x="{x + 14}" y="{y + 48}" class="fdet">{escape(detail)}</text>'
    )


def arrow(x1, y1, x2, y2, p, both=False):
    """A flow from (x1,y1) to (x2,y2). Both ways: two lines, one each way."""
    if not both:
        return f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" class="fflow" marker-end="url(#{p}h)"/>'
    dx, dy = (0, 6) if y1 == y2 else (6, 0)
    return (
        f'<line x1="{x1 + dx}" y1="{y1 - dy}" x2="{x2 + dx}" y2="{y2 - dy}" class="fflow" marker-end="url(#{p}h)"/>'
        f'<line x1="{x2 - dx}" y1="{y2 + dy}" x2="{x1 - dx}" y2="{y1 + dy}" class="fflow back" marker-end="url(#{p}h)"/>'
    )


def wide():
    W, H = 1160, 560
    p = "fw"
    out = [f'<svg class="fig wide" viewBox="0 0 {W} {H}" width="{W}" height="{H}" role="img" '
           'aria-labelledby="fw-t" xmlns="http://www.w3.org/2000/svg"><title id="fw-t">'
           'Publishers on the left put data on the fabric of one goatnet in the middle; surfaces on the right read it. '
           'transform-shim, panoscope and goat-map move data both ways.</title>', ARROW_DEFS.format(p=p)]
    out.append(f'<text x="20" y="28" class="fcol">PUT DATA ON THE FABRIC</text>')
    out.append(f'<text x="780" y="28" class="fcol">READ IT</text>')
    fx, fw, fy, fh = 470, 220, 50, 490
    out.append(f'<rect x="{fx}" y="{fy}" width="{fw}" height="{fh}" rx="10" class="ffabric"/>')
    out.append(f'<text x="{fx + fw / 2}" y="{fy + fh / 2 - 8}" text-anchor="middle" class="fname">Zenoh routers</text>')
    out.append(f'<text x="{fx + fw / 2}" y="{fy + fh / 2 + 16}" text-anchor="middle" class="fdet">one goatnet\'s fabric</text>')
    bw, bh, step, top = 360, 66, 96, 52
    for i, (name, det, both) in enumerate(PUBLISHERS):
        y = top + i * step
        out.append(box(20, y, bw, bh, name, det))
        out.append(arrow(20 + bw + 4, y + bh / 2, fx - 4, y + bh / 2, p, both))
    for i, (name, det, both) in enumerate(SURFACES):
        y = top + i * step
        out.append(box(780, y, bw, bh, name, det, "fbox fsurf"))
        out.append(arrow(fx + fw + 4, y + bh / 2, 780 - 4, y + bh / 2, p, both))
    out.append("</svg>")
    return "".join(out)


def narrow():
    W = 360
    p = "fn"
    bw, bh, gap = 340, 62, 10
    y = 30
    parts = []
    parts.append(f'<text x="10" y="{y - 10}" class="fcol">PUT DATA ON THE FABRIC</text>')
    for name, det, both in PUBLISHERS:
        parts.append(box(10, y, bw, bh, name + ("  ⇅" if both else ""), det))
        y += bh + gap
    y += 6
    parts.append(arrow(130, y, 130, y + 34, p, False))
    parts.append(arrow(230, y, 230, y + 34, p, True))
    parts.append(f'<text x="250" y="{y + 22}" class="fdet">both ways</text>')
    y += 42
    parts.append(f'<rect x="10" y="{y}" width="{bw}" height="66" rx="10" class="ffabric"/>')
    parts.append(f'<text x="{W / 2}" y="{y + 28}" text-anchor="middle" class="fname">Zenoh routers</text>')
    parts.append(f'<text x="{W / 2}" y="{y + 50}" text-anchor="middle" class="fdet">one goatnet\'s fabric</text>')
    y += 66 + 8
    parts.append(arrow(130, y, 130, y + 34, p, False))
    parts.append(arrow(230, y, 230, y + 34, p, True))
    parts.append(f'<text x="250" y="{y + 22}" class="fdet">both ways</text>')
    y += 42
    parts.append(f'<text x="10" y="{y + 12}" class="fcol">READ IT</text>')
    y += 24
    for name, det, both in SURFACES:
        parts.append(box(10, y, bw, bh, name + ("  ⇅" if both else ""), det, "fbox fsurf"))
        y += bh + gap
    H = y + 4
    return (f'<svg class="fig narrow" viewBox="0 0 {W} {H}" width="{W}" height="{H}" role="img" aria-labelledby="fn-t" '
            'xmlns="http://www.w3.org/2000/svg"><title id="fn-t">Publishers put data on the fabric of one goatnet; '
            'surfaces read it. transform-shim, panoscope and goat-map, marked with a double arrow, move data both ways.</title>'
            + ARROW_DEFS.format(p=p) + "".join(parts)
            + "</svg>")


def render():
    return wide() + "\n" + narrow()


if __name__ == "__main__":
    print(render())
