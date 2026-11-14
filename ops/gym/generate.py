#!/usr/bin/env python3
"""goat gym — render the registry and its documents into a static site.

    gym.yaml ─┐
    content/ ─┼─→ generate.py ─→ site/   (nginx, mesh-only)
    docs/ ────┘

THREE THINGS THIS FILE EXISTS TO GET RIGHT, each learned by getting it wrong:

1. NO REPO PATHS ON A SERVED PAGE. The first version printed
   `docs/design/zenoh-data-fabric.md` at the reader and called it a reference.
   Most readers have no repository, so that is a dead end wearing the clothes
   of a citation. Referenced documents are now HOSTED beside the gym and shown
   by their own title; anything not hosted is named, never pathed.

2. NOTHING SHIPS UNSCRUBBED. Every rendered page goes through scrub.py before
   it is written. The gym is unfenced on the mesh, so a page carrying an
   operator's name, an address or a place of operations must not exist.

3. A PAGE THAT OPENS MUST TEACH. Modules marked `planned` get NO page — they
   appear in their lane as a named gap. A thin page is worse than an honest
   absence, because the reader spends the click before finding out.

Usage:
    generate.py                          # render into ops/gym/site/
    generate.py --out DIR                # the deploy role stages this
    generate.py --services services.json # per-goatnet live surfaces
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import render as R          # noqa: E402
try:
    import scrub            # noqa: E402  the deployment's own name and place check
except ImportError:         # a public checkout carries no deployment roster to check against
    class scrub:            # noqa: N801
        @staticmethod
        def find_hits(text, roster):
            return []

        @staticmethod
        def roster_names():
            return set()

try:
    import yaml
except ImportError:  # pragma: no cover
    sys.exit("generate.py needs PyYAML (pip install pyyaml)")

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
with open(os.path.join(HERE, "theme.css")) as source_file:
    CSS = source_file.read()

# Same shape the site gate looks for. A quoted repository path is still a path
# to a reader who has no repository, so previews are held to the page's rule.
REPO_PATH_IN_TEXT = re.compile(
    r"\b(?:docs|ops|terraform[\w-]*|\.github)/[\w./-]+\.(?:md|ya?ml|py|sh|go|j2|rs|json)\b")


# ── page chrome ──────────────────────────────────────────────────────────────

MERMAID_INIT = """
<script src="{up}mermaid.min.js"></script>
<script>
if (window.mermaid) {{
  var dark = matchMedia('(prefers-color-scheme: dark)').matches;
  mermaid.initialize({{startOnLoad: true, securityLevel: 'strict',
                      theme: dark ? 'dark' : 'neutral',
                      fontFamily: 'ui-sans-serif, -apple-system, sans-serif'}});
}}
</script>
"""


def page(title: str, body: str, depth: int, footer: str, narrow: bool = True) -> str:
    up = "../" * depth
    # Only pages carrying a diagram pay for the renderer.
    diagrams = MERMAID_INIT.format(up=up) if 'class="mermaid"' in body else ""
    nav = [("Lanes", f"{up}index.html"),
           ("Surfaces", f"{up}surfaces.html"),
           ("Foundations", f"{up}foundations/index.html"),
           ("Library", f"{up}library/index.html")]
    links = "".join(f'<a href="{h}">{R.esc(t)}</a>' for t, h in nav)
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{R.esc(title)}</title>
<style>{CSS}</style>
</head>
<body>
<div class="topbar"><div class="in">
  <a class="brand" href="{up}index.html">goat <span>gym</span></a>
  <nav class="topnav">{links}</nav>
</div></div>
<main class="wrap{' narrow' if narrow else ''}">
{body}
</main>
<div class="wrap{' narrow' if narrow else ''}"><footer>{footer}</footer></div>
{diagrams}
</body>
</html>
"""


# ── the document library ─────────────────────────────────────────────────────

def slug_for(path: str) -> str:
    return os.path.splitext(os.path.basename(path))[0]


class Library:
    """Project documents hosted beside the gym, so a pointer is a link.

    A document is hosted only if it exists AND passes the publication gate. One
    that does not is still NAMED on the module that cites it — the reader learns
    the document exists and can ask for it — but it is never linked or pathed.
    """

    def __init__(self, roster: set[str]):
        self.roster = roster
        self.hosted: dict[str, dict] = {}     # repo path -> {slug,title}
        self.refused: dict[str, list] = {}    # repo path -> hits
        self.titles: dict[str, str] = {}

    def consider(self, repo_path: str) -> None:
        if repo_path in self.hosted or repo_path in self.refused:
            return
        full = os.path.join(REPO, repo_path)
        if not os.path.exists(full) or not repo_path.endswith(".md"):
            self.refused.setdefault(repo_path, [])
            self.titles[repo_path] = R.doc_title(full) if os.path.exists(full) else \
                os.path.basename(repo_path)
            return
        with open(full, encoding="utf-8", errors="replace") as source_file:
            text = source_file.read()
        title = R.doc_title(full)
        self.titles[repo_path] = title
        hits = scrub.find_hits(text, self.roster)
        if hits:
            self.refused[repo_path] = hits
            return
        self.hosted[repo_path] = {"slug": slug_for(repo_path), "title": title}

    def title(self, repo_path: str) -> str:
        return self.titles.get(repo_path) or os.path.basename(repo_path)

    def preview(self, repo_path: str, words: int = 45) -> str | None:
        """A few opening sentences of a document we cannot host.

        A reader who cannot open the document should still learn what is in it.
        The preview goes through the SAME rules as any other gym-authored text:
        the publication gate, and the no-repository-paths rule — a quoted path
        is still a dead end to a reader with no repository.

        So it walks paragraphs and returns the first CLEAN one rather than
        editing a quotation to fit. If none of the opening paragraphs qualify it
        returns nothing, and the page says the preview was withheld instead of
        showing a gap.
        """
        full = os.path.join(REPO, repo_path)
        if not os.path.exists(full) or not repo_path.endswith(".md"):
            return None
        with open(full, encoding="utf-8", errors="replace") as source_file:
            text = source_file.read()

        paragraphs: list[str] = []
        current: list[str] = []
        for line in text.splitlines():
            st = line.strip()
            structural = (not st or st.startswith(("#", "|", ">", "-", "*", "`", "!", "<"))
                          or st.startswith("**Status") or st.startswith("Status:")
                          or st.startswith("---"))
            if structural:
                if current:
                    paragraphs.append(" ".join(current))
                    current = []
                if len(paragraphs) >= 6:
                    break
            else:
                current.append(st)
        if current:
            paragraphs.append(" ".join(current))

        for para in paragraphs:
            words_out = para.split()
            if len(words_out) < 12:
                continue
            snippet = " ".join(words_out[:words]).rstrip(" ,.;:—-") + "…"
            snippet = re.sub(r"[*_`\[\]]", "", snippet)
            if scrub.find_hits(snippet, self.roster):
                continue
            if REPO_PATH_IN_TEXT.search(snippet):
                continue
            return snippet
        return None

    def href(self, repo_path: str, depth: int) -> str | None:
        entry = self.hosted.get(repo_path)
        if not entry:
            return None
        return f'{"../" * depth}library/{entry["slug"]}.html'

    def resolver(self, from_doc: str):
        """Rewrite intra-document links to hosted siblings; drop the rest."""
        base = os.path.dirname(from_doc)

        def resolve(target: str) -> str | None:
            # Absolute paths are site-root references from the document's own
            # repository, not ours — joining them produces a link that resolves
            # nowhere, and a hosted document must not hand the reader a 404.
            if target.startswith(("http://", "https://", "#", "mailto:", "/")):
                return None
            path = os.path.normpath(os.path.join(base, target.split("#")[0]))
            entry = self.hosted.get(path)
            return f'{entry["slug"]}.html' if entry else None
        return resolve

    def write(self, out_dir: str, footer: str) -> None:
        lib = os.path.join(out_dir, "library")
        os.makedirs(lib, exist_ok=True)
        for repo_path, entry in sorted(self.hosted.items(), key=lambda kv: kv[1]["title"].lower()):
            with open(os.path.join(REPO, repo_path), encoding="utf-8", errors="replace") as source_file:
                text = source_file.read()
            # Drop the document's own H1: the page header already carries it, and
            # printing the title twice reads as a rendering fault.
            lines = text.splitlines()
            for n, ln in enumerate(lines[:8]):
                if ln.startswith("# "):
                    del lines[n]
                    break
            body = R.md_to_html("\n".join(lines), self.resolver(repo_path))
            head = (f'<p class="crumb"><a href="../index.html">gym</a> / '
                    f'<a href="index.html">library</a></p>'
                    f'<p class="eyebrow">Project document</p>'
                    f'<h1>{R.esc(entry["title"])}</h1>')
            write_page(os.path.join(lib, entry["slug"] + ".html"),
                       page(f'{entry["title"]} — goat gym', head + body, 1, footer))

        rows = "".join(
            f'<li><a href="{e["slug"]}.html">{R.esc(e["title"])}</a></li>'
            for _, e in sorted(self.hosted.items(), key=lambda kv: kv[1]["title"].lower()))
        body = (f'<p class="crumb"><a href="../index.html">gym</a> / library</p>'
                f"<h1>Library</h1>"
                f'<p class="lead">The project documents the modules point at, hosted here so a '
                f'pointer is something you can open. {len(self.hosted)} documents.</p>'
                f'<ul class="rowlist">{rows}</ul>')
        write_page(os.path.join(lib, "index.html"), page("Library — goat gym", body, 1, footer))


# ── writing, with the gate in the way ────────────────────────────────────────

REFUSALS: list[str] = []
ROSTER: set[str] = set()


def write_page(path: str, html_text: str) -> None:
    hits = scrub.find_hits(html_text, ROSTER)
    if hits:
        classes = sorted({f"{c}:{m}" for c, _, m in hits})
        REFUSALS.append(f"{os.path.basename(path)} — {', '.join(classes[:4])}")
        return
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as fh:
        fh.write(html_text)


# ── module rendering ─────────────────────────────────────────────────────────


def teaser(text: str, limit: int = 150) -> str:
    """Cut on a word boundary. A summary that stops mid-word reads as breakage."""
    text = (text or "").strip()
    if len(text) <= limit:
        return text
    return text[:limit].rsplit(" ", 1)[0].rstrip(" ,.;:—-") + "…"

def chips(*items) -> str:
    out = []
    for cls, text in items:
        if text:
            out.append(f'<span class="chip {cls}">{R.esc(text)}</span>')
    return f'<div class="chips">{"".join(out)}</div>' if out else ""


def render_steps(steps: list[dict]) -> str:
    out = []
    for step in steps:
        do = step.get("do", "")
        cmd = step.get("run")
        note = step.get("note")
        block = f'<p class="do">{R.inline(do)}</p>'
        if cmd:
            block += f"<pre><code>{R.esc(cmd)}</code></pre>"
        if note:
            block += f'<p class="note">{R.inline(note)}</p>'
        out.append(f"<li>{block}</li>")
    return f'<ol class="steps">{"".join(out)}</ol>'


def render_module(mod: dict, lane_of: dict, lib: Library, services: dict,
                  out_dir: str, footer: str, neighbours: dict, repo: dict | None = None) -> None:
    mid = mod["id"]
    lanes = lane_of.get(mid, [])
    parts = [
        '<p class="crumb"><a href="../index.html">gym</a> / '
        + " · ".join(f'<a href="../lanes/{l["id"]}.html">{R.esc(l["title"])}</a>' for l in lanes)
        + "</p>",
        f'<h1>{R.esc(mod["title"])}</h1>',
        chips(("level-" + mod["level"], mod["level"]),
              ("", f'{mod["minutes"]} min')),
    ]

    if mod.get("summary"):
        parts.append(f'<p class="lead">{R.inline(mod["summary"])}</p>')

    if mod.get("body"):
        parts.append(R.md_to_html(mod["body"], demote_h1=False))

    if mod.get("walkthrough"):
        parts.append("<h2>Do this</h2>")
        parts.append(render_steps(mod["walkthrough"]))

    quirks = mod.get("quirks") or []
    if quirks:
        parts.append("<h2>Quirks — true, and not visible on first look</h2>")
        for q in quirks:
            src = q.get("evidence", "")
            label = evidence_label(src, lib)
            parts.append(
                f'<div class="quirk"><p>{R.inline(q["claim"])}</p>'
                + (f'<p class="so"><b>So:</b> {R.inline(q["so"])}</p>' if q.get("so") else "")
                + (f'<span class="src">{label}</span>' if label else "")
                + "</div>")

    drill = mod.get("drill")
    if drill:
        parts.append("<h2>Drill</h2>")
        run = f'<pre><code>{R.esc(drill["run"])}</code></pre>' if drill.get("run") else ""
        parts.append(
            '<div class="drill">'
            f'<p><b>Do:</b> {R.inline(drill["do"])}</p>{run}'
            f'<p class="pass"><b>You pass when:</b> {R.inline(drill["pass"])}</p>'
            + chips(("safety-" + drill["safety"], drill["safety"]),
                    ("", drill.get("needs", "")))
            + "</div>")

    deeper = mod.get("deeper") or []
    if deeper:
        parts.append("<h2>Go deeper</h2><ul class=\"linklist\">")
        for item in deeper:
            path = item.get("doc")
            why = item.get("why", "")
            href = lib.href(path, 1) if path else item.get("url")
            title = item.get("title") or (lib.title(path) if path else "")
            if href:
                parts.append(f'<li><a href="{R.esc(href)}">{R.esc(title)}</a>'
                             f'<span class="why">{R.inline(why)}</span></li>')
            else:
                # Not hosted: name it, say what is in it, show what we may, and
                # tell the reader exactly how to get the rest. A dead end with an
                # apology was the previous behaviour and it read as broken.
                preview = lib.preview(path) if path else None
                block = [f'<li class="offsite"><span class="t">{R.esc(title)}</span>'
                         f'<span class="why">{R.inline(why)}</span>']
                if preview:
                    block.append(f'<p class="preview">{R.esc(preview)}</p>')
                elif path:
                    block.append('<p class="preview withheld">Preview withheld — the opening of '
                                 'this document contains material the publication gate refuses.</p>')
                block.append('<p class="access">' + repo_access_html(repo, 1) + "</p>")
                block.append("</li>")
                parts.append("".join(block))
        parts.append("</ul>")
        if any(not (lib.href(i.get("doc"), 1) if i.get("doc") else i.get("url"))
               for i in deeper):
            parts.append(repo_footnote_html(repo))

    surfaces = [s for s in (mod.get("surface") or []) if s in services]
    if surfaces:
        parts.append("<h2>Open it on this goatnet</h2><ul class=\"linklist\">")
        for sid in surfaces:
            svc = services[sid]
            gate = "open to every peer" if svc.get("access") == "public" else "admins only"
            cls = "open" if svc.get("access") == "public" else "gated"
            parts.append(
                f'<li>{surface_name_html(svc)} '
                f'<span class="chip {cls}">{gate}</span>'
                f'<span class="why">{R.inline(svc.get("desc", ""))}</span></li>')
        parts.append("</ul>")

    prev_m, next_m = neighbours.get(mid, (None, None))
    if prev_m or next_m:
        left = f'<a class="l" href="{prev_m["id"]}.html">{R.esc(prev_m["title"])}</a>' if prev_m else "<span></span>"
        right = f'<a class="r" href="{next_m["id"]}.html">{R.esc(next_m["title"])}</a>' if next_m else "<span></span>"
        parts.append(f'<div class="pager">{left}{right}</div>')

    write_page(os.path.join(out_dir, "modules", f"{mid}.html"),
               page(f'{mod["title"]} — goat gym', "\n".join(parts), 1, footer))


def repo_access_html(repo: dict | None, depth: int) -> str:
    """One honest sentence about a document we do not host.

    Always three parts, never fewer: WHERE it is, WHAT it costs to get in, and
    WHEN that stops being true. A link with no caveat implies the reader can
    open it; a caveat with no link makes them go and ask what the repository is
    even called.
    """
    if not repo:
        return ('<span class="chip gated">not published here</span> '
                "Ask a platform admin for it.")
    bits = ['<span class="chip gated">not published here</span> ']
    if repo.get("url"):
        bits.append(f'It lives in <a href="{R.esc(repo["url"])}" rel="noreferrer">'
                    f'{R.esc(repo.get("name", "the project repository"))}</a>. ')
    else:
        bits.append(f'It lives in {R.esc(repo.get("name", "the project repository"))}. ')
    if repo.get("note"):
        bits.append(R.esc(repo["note"]) + " ")
    return "".join(bits)


def repo_footnote_html(repo: dict | None) -> str:
    """The open-sourcing note, once per page.

    It was repeated under every unhosted entry, which turned a genuinely useful
    fact into visual noise and buried the per-item detail beside it. Said once,
    at the end of the section it applies to.
    """
    if not repo or not repo.get("open_sourcing"):
        return ""
    return f'<p class="soon-note">{R.esc(repo["open_sourcing"])}</p>' 


def surface_href(svc: dict) -> str | None:
    """Where a reader's click should land, or None if there is nowhere to land.

    Two fields on the service row decide this, and BOTH already exist because
    the status page reads them — the gym was ignoring them and inventing
    `https://<fqdn>/` for every row.

      kind=api   A machine door, not a web page. Through the ingress these
                 names route nothing and answer 404. The status page has never linked
                 them; it renders them as plain text. So does this now.

      link_path  The row's landing page when it is not `/`. Four ZAGROS rows
                 (netview, netview-response, peerview-peers, peerview-demand)
                 share the status page's hostname and are told apart ONLY by
                 this path — without it all four linked to the same index.
    """
    if (svc.get("kind") or "ui") != "ui":
        return None
    return f'https://{svc["fqdn"]}{svc.get("link_path") or "/"}'


def surface_name_html(svc: dict) -> str:
    """The service's name: a link where one works, plain text where none does."""
    href = surface_href(svc)
    if not href:
        return (f'<span class="t">{R.esc(svc["name"])}</span> '
                f'<span class="chip machine">not a web page</span>')
    return f'<a href="{R.esc(href)}">{R.esc(svc["name"])}</a>'


def evidence_label(src: str, lib: Library) -> str:
    """Evidence, named rather than pathed. A reader cannot open a repo path."""
    if not src:
        return ""
    token = src.split(" §")[0].split(" (")[0].strip()
    if token.startswith("F-"):
        return f"Recorded as finding {token} in the project's findings log."
    if token.endswith(".md"):
        title = lib.title(token)
        return f"From “{title}”."
    return "Verified against the deployed system."


# ── site ─────────────────────────────────────────────────────────────────────

def build(reg: dict, out_dir: str, services: dict, rev: str) -> dict:
    modules = {m["id"]: m for m in reg["modules"]}
    lanes = reg["lanes"]
    written = {mid: m for mid, m in modules.items() if m.get("status") == "written"}

    footer = (
        f'goat gym · version <code>{R.esc(rev)}</code> · '
        f"{len(written)} modules written of {len(modules)} mapped. "
        "Every page here is checked for names, addresses and places before it ships. "
        "Something wrong, missing, or thin: tell a platform admin — that is how it gets fixed."
    )

    lane_of: dict[str, list] = {}
    for lane in lanes:
        for mid in lane["modules"]:
            lane_of.setdefault(mid, []).append(lane)

    neighbours: dict[str, tuple] = {}
    for lane in lanes:
        seq = [modules[m] for m in lane["modules"] if m in written]
        for i, mod in enumerate(seq):
            if mod["id"] in neighbours:
                continue
            neighbours[mod["id"]] = (seq[i - 1] if i else None,
                                     seq[i + 1] if i + 1 < len(seq) else None)

    # library
    lib = Library(ROSTER)
    for mod in modules.values():
        for item in (mod.get("deeper") or []):
            if item.get("doc"):
                lib.consider(item["doc"])
        for q in (mod.get("quirks") or []):
            ev = str(q.get("evidence", ""))
            token = ev.split(" §")[0].split(" (")[0].strip()
            if token.endswith(".md") and not token.startswith("F-"):
                lib.consider(token)
    lib.write(out_dir, footer)

    # modules
    repo = ((reg.get('meta') or {}).get('repos') or {}).get('core')
    for mod in written.values():
        render_module(mod, lane_of, lib, services, out_dir, footer, neighbours, repo)

    # lanes
    for lane in lanes:
        rows = []
        n = 0
        headings = lane.get("headings") or {}
        for mid in lane["modules"]:
            mod = modules[mid]
            if mid in headings:
                rows.append(f'<li class="heading">{R.esc(headings[mid])}</li>')
            if mod.get("status") == "written":
                n += 1
                rows.append(
                    f'<li><span class="n">{n:02d}</span>'
                    f'<span><a href="../modules/{mid}.html">{R.esc(mod["title"])}</a>'
                    f'<span class="why">{R.inline(teaser(mod.get("summary", "")))}</span></span>'
                    f'<span class="chip">{mod["minutes"]} min</span></li>')
            elif mod.get("frontier"):
                rows.append(
                    f'<li class="planned"><span class="n">··</span>'
                    f'<span>{R.esc(mod["title"])}'
                    f'<span class="why">Frontier territory: the tooling for this is not built yet.</span></span>'
                    f'<span class="chip frontier">frontier</span></li>')
            else:
                rows.append(
                    f'<li class="planned"><span class="n">··</span>'
                    f'<span>{R.esc(mod["title"])}'
                    f'<span class="why">Mapped, not yet written.</span></span>'
                    f'<span class="chip">planned</span></li>')
        mins = sum(modules[m]["minutes"] for m in lane["modules"]
                   if modules[m].get("status") == "written")
        body = (
            f'<p class="crumb"><a href="../index.html">gym</a> / lane</p>'
            f'<p class="eyebrow">Lane</p><h1>{R.esc(lane["title"])}</h1>'
            f'<p class="lead">{R.inline(lane["outcome"])}</p>'
            f"<p><b>Who it is for:</b> {R.inline(lane['audience'])}</p>"
            f'<p class="meta chips"><span class="chip">{n} modules ready</span>'
            f'<span class="chip">about {mins} min</span></p>'
            f'<ul class="rowlist">{"".join(rows)}</ul>')
        nxt = next((x for x in lanes if x["id"] == lane.get("next")), None)
        if nxt:
            body += (f'<p class="meta">Finished this lane? Next: '
                     f'<a href="{nxt["id"]}.html">{R.esc(nxt["title"])}</a>.</p>')
        write_page(os.path.join(out_dir, "lanes", f'{lane["id"]}.html'),
                   page(f'{lane["title"]} — goat gym', body, 1, footer))

    # surfaces + foundations
    write_surfaces(out_dir, services, modules, footer)
    techs = write_foundations(out_dir, footer)

    # index
    cards = []
    for lane in lanes:
        ready = sum(1 for m in lane["modules"] if modules[m].get("status") == "written")
        cards.append(
            f'<a class="card" href="lanes/{lane["id"]}.html">'
            f'<h3>{R.esc(lane["title"])}</h3>'
            f'<p>{R.esc(lane["audience"])}</p>'
            f'<div class="meta">{ready} of {len(lane["modules"])} modules ready</div></a>')

    rows = []
    for mid, mod in sorted(written.items(), key=lambda kv: kv[1]["title"].lower()):
        rows.append(
            f'<li data-s="{R.esc((mod["title"] + " " + mod.get("summary", "")).lower())}">'
            f'<span class="n"></span><span><a href="modules/{mid}.html">{R.esc(mod["title"])}</a>'
            f'<span class="why">{R.inline(teaser(mod.get("summary", ""), 130))}</span></span>'
            f'<span class="chip">{mod["minutes"]} min</span></li>')

    body = f"""
<p class="eyebrow">Training</p>
<h1>Learn this system by using it</h1>
<p class="lead">Nine lanes, one per role. Each module explains the thing here, gives you a
drill you can actually run, and names the quirks that cost people days — the behaviour
that is real, load-bearing, and invisible until it bites.</p>
<div class="grid">{''.join(cards)}</div>

<h2>Foundations — the technologies we do not own</h2>
<p class="lead">WireGuard, Zenoh, Ory, Postgres, Terraform and the rest: curated outside
reading, each with the one thing upstream documentation cannot contain — where our use
differs from what it assumes. <a href="foundations/index.html">Open Foundations →</a></p>

<h2>Every module</h2>
<input class="filter" id="f" type="search" placeholder="Filter modules…" autocomplete="off">
<ul class="rowlist" id="list">{''.join(rows)}</ul>
<script>
document.getElementById('f').addEventListener('input', function (e) {{
  var q = e.target.value.toLowerCase();
  document.querySelectorAll('#list li').forEach(function (li) {{
    li.style.display = li.dataset.s.indexOf(q) === -1 ? 'none' : '';
  }});
}});
</script>
"""
    write_page(os.path.join(out_dir, "index.html"),
               page("goat gym", body, 0, footer, narrow=False))

    payload = {"version": reg.get("version"), "rev": rev,
               "written": len(written), "mapped": len(modules),
               "lanes": [{"id": l["id"], "title": l["title"], "modules": l["modules"]}
                         for l in lanes],
               "modules": [{k: v for k, v in m.items() if k != "body"}
                           for m in reg["modules"]]}
    with open(os.path.join(out_dir, "gym.json"), "w") as fh:
        json.dump(payload, fh, indent=2)

    return {"lanes": len(lanes), "written": len(written), "mapped": len(modules),
            "hosted": len(lib.hosted), "unhosted": len(lib.refused), "techs": techs}




# ── foundations: the technologies we use and do not own ──────────────────────

def write_foundations(out_dir: str, footer: str) -> int:
    """Curated outside reading, with our deltas attached.

    The value is not the links — anyone can find upstream documentation. It is
    `differences`: where our use diverges from what the upstream document
    assumes, which is the part no upstream document will ever contain and the
    part that costs days to discover by accident.
    """
    cat_path = os.path.join(HERE, "upstream.yaml")
    if not os.path.exists(cat_path):
        return 0
    with open(cat_path) as fh:
        cat = yaml.safe_load(fh)

    verified = {}
    vpath = os.path.join(HERE, "upstream-verified.json")
    if os.path.exists(vpath):
        with open(vpath) as source_file:
            verified = json.load(source_file)

    techs = cat.get("technologies", [])
    cats = cat.get("categories", [])
    by_cat: dict[str, list] = {}
    for t in techs:
        by_cat.setdefault(t.get("category", "other"), []).append(t)

    os.makedirs(os.path.join(out_dir, "foundations"), exist_ok=True)

    for tech in techs:
        parts = [
            ('<p class="crumb"><a href="../index.html">gym</a> / '
             '<a href="index.html">foundations</a></p>'),
            '<p class="eyebrow">We use this and do not own it</p>',
            f'<h1>{R.esc(tech["name"])}</h1>',
            chips((("open" if tech.get("status") == "in-use" else "gated"),
                   {"in-use": "we run this", "partial": "partly deployed",
                    "not-deployed": "NOT deployed here"}.get(tech.get("status"), "")),),
            f'<p class="lead">{R.inline(tech["one_line"])}</p>',
            "<h2>What it is</h2>", f"<p>{R.inline(tech['what_it_is'])}</p>",
            "<h2>How we use it</h2>", f"<p>{R.inline(tech['how_we_use_it'])}</p>",
            "<h2>Where our use differs</h2>",
            f'<div class="quirk"><p>{R.inline(tech["differences"])}</p>'
            '<span class="src">This part is ours. No upstream document will tell you it.</span></div>',
        ]

        parts.append("<h2>Extended reading</h2>")
        parts.append('<p class="note">Every link here is off-mesh — it needs internet, '
                     'which the surfaces on this goatnet do not.</p>')
        parts.append('<ul class="linklist">')
        for r in tech.get("reading") or []:
            v = verified.get(r["url"], {})
            stamp = (f' · reachable {v["checked"]}' if v.get("ok")
                     else (' · <b>did not answer at last check</b>' if v else ""))
            mins = f'{r["minutes"]} min' if r.get("minutes") else "reference"
            parts.append(
                f'<li><a href="{R.esc(r["url"])}" rel="noreferrer">{R.esc(r["title"])}</a> '
                f'<span class="chip">{R.esc(r["kind"])}</span>'
                f'<span class="chip">{mins}</span>'
                f'<span class="why">{R.inline(r["why"])}<br>'
                f'<span class="meta">{R.esc(r["url"])}{stamp}</span></span></li>')
        parts.append("</ul>")

        parts.append("<h2>Do these</h2>")
        parts.append('<ol class="steps">')
        for a in tech.get("activities") or []:
            parts.append(f'<li><p class="do">{R.inline(a["do"])}</p>'
                         f'<p class="note">{R.inline(a["gets"])}</p></li>')
        parts.append("</ol>")

        write_page(os.path.join(out_dir, "foundations", f'{tech["id"]}.html'),
                   page(f'{tech["name"]} — goat gym', "\n".join(parts), 1, footer))

    sections = []
    for c in cats:
        rows = []
        for t in by_cat.get(c["id"], []):
            n = len(t.get("reading") or [])
            st = t.get("status")
            badge = ("" if st == "in-use" else
                     f'<span class="chip gated">'
                     f'{"partly deployed" if st == "partial" else "not deployed"}</span>')
            rows.append(
                f'<li><span class="n"></span>'
                f'<span><a href="{t["id"]}.html">{R.esc(t["name"])}</a>'
                f'<span class="why">{R.inline(t["one_line"])}</span></span>'
                f'{badge}<span class="chip">{n} readings</span></li>')
        if rows:
            sections.append(f'<h2>{R.esc(c["title"])}</h2>'
                            f'<p class="note">{R.inline(c["blurb"])}</p>'
                            f'<ul class="rowlist">{"".join(rows)}</ul>')

    links = sum(len(t.get("reading") or []) for t in techs)
    body = ('<p class="crumb"><a href="../index.html">gym</a> / foundations</p>'
            '<p class="eyebrow">Outside this project</p>'
            "<h1>Foundations</h1>"
            '<p class="lead">The technologies goat is built from and does not own. Their '
            'documentation is better than anything we would write, so this section links it '
            'rather than copying it — and adds the one thing it cannot contain: where our use '
            f'differs from what it assumes. {len(techs)} technologies, {links} curated readings.</p>'
            + "".join(sections))
    write_page(os.path.join(out_dir, "foundations", "index.html"),
               page("Foundations — goat gym", body, 1, footer))
    return len(techs)

def write_surfaces(out_dir: str, services: dict, modules: dict, footer: str) -> None:
    if not services:
        body = ("<h1>Surfaces</h1><p class=\"lead\">This copy of the gym was built without a "
                "service list, so it cannot show you what this goatnet runs. Your status page "
                "can.</p>")
        write_page(os.path.join(out_dir, "surfaces.html"),
                   page("Surfaces — goat gym", body, 0, footer))
        return

    by_module: dict[str, list] = {}
    for mid, mod in modules.items():
        if mod.get("status") != "written":
            continue
        for sid in (mod.get("surface") or []):
            by_module.setdefault(sid, []).append((mid, mod["title"]))

    groups: dict[str, list] = {}
    for sid, svc in services.items():
        groups.setdefault(svc.get("category", "other"), []).append((sid, svc))

    order = ["learn", "account", "sandbox", "platform", "live", "admin", "api", "tools", "other"]
    sections = []
    for cat in sorted(groups, key=lambda c: order.index(c) if c in order else 99):
        rows = []
        for sid, svc in sorted(groups[cat], key=lambda kv: kv[1]["name"].lower()):
            gate = "open to every peer" if svc.get("access") == "public" else \
                ("admins only" if svc.get("access") == "admins" else "machine door")
            cls = "open" if svc.get("access") == "public" else "gated"
            teach = by_module.get(sid, [])
            learn = " ".join(
                f'<a href="modules/{mid}.html">{R.esc(title)}</a>' for mid, title in teach[:2]
            ) or '<span class="unhosted">no module yet</span>'
            rows.append(
                f'<tr><td>{surface_name_html(svc)}<br>'
                f'<span class="why">{R.inline(svc.get("desc", ""))}</span></td>'
                f'<td><span class="chip {cls}">{gate}</span></td>'
                f"<td>{learn}</td></tr>")
        sections.append(
            f"<h2>{R.esc(cat.title())}</h2>"
            f'<div class="scroll"><table><thead><tr><th>Service</th><th>Who can open it</th>'
            f"<th>Learn it</th></tr></thead><tbody>{''.join(rows)}</tbody></table></div>")

    body = (f'<p class="eyebrow">This goatnet</p><h1>Surfaces</h1>'
            f'<p class="lead">Everything this goatnet runs, what it is for, and who can open it. '
            f'“Open to every peer” means any enrolled device; “admins only” means a login and a '
            f'group. {len(services)} services.</p>' + "".join(sections))
    write_page(os.path.join(out_dir, "surfaces.html"),
               page("Surfaces — goat gym", body, 0, footer, narrow=False))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(HERE, "site"))
    ap.add_argument("--rev", default=os.environ.get("GYM_REV", "working tree"))
    ap.add_argument("--services", help="JSON list of this goatnet's isitup services")
    ap.add_argument("--mermaid", help="path to a vendored mermaid bundle; without it, "
                                      "diagrams render as their own source")
    args = ap.parse_args()

    global ROSTER
    ROSTER = scrub.roster_names()
    if not ROSTER:
        print("gym: NO OPERATOR ROSTER READABLE — pages will not be checked for names",
              file=sys.stderr)

    with open(os.path.join(HERE, "gym.yaml")) as fh:
        reg = yaml.safe_load(fh)

    for mod in reg["modules"]:
        if mod.get("content"):
            path = os.path.join(REPO, mod["content"])
            if os.path.exists(path):
                with open(path, encoding="utf-8", errors="replace") as source_file:
                    mod["body"] = source_file.read()

    services = {}
    if args.services and os.path.exists(args.services):
        with open(args.services) as fh:
            rows = json.load(fh)
        services = {r["id"]: r for r in rows if r.get("id") and r.get("fqdn")}

    if os.path.isdir(args.out):
        shutil.rmtree(args.out)
    os.makedirs(args.out, exist_ok=True)

    if args.mermaid and os.path.exists(args.mermaid):
        shutil.copyfile(args.mermaid, os.path.join(args.out, "mermaid.min.js"))

    stats = build(reg, args.out, services, args.rev)

    print(f"gym: {stats['written']} modules written of {stats['mapped']} mapped, "
          f"{stats['lanes']} lanes, {stats['hosted']} documents hosted "
          f"({stats['unhosted']} not publishable), {len(services)} live surfaces, "
          f"{stats['techs']} foundations")
    if REFUSALS:
        print("gym: PAGES REFUSED BY THE PUBLICATION GATE:", file=sys.stderr)
        for r in REFUSALS:
            print(f"    {r}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
