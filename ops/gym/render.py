#!/usr/bin/env python3
"""goat gym — markdown → HTML, and the shared page chrome.

Split out of generate.py because the site now renders two kinds of page from
markdown: the modules' own prose, and the project documents hosted alongside
them. Both go through the same converter so a hosted document reads like part
of the site rather than a pasted file.

Deliberately a small markdown subset (headings, paragraphs, lists, tables,
fenced code, blockquotes, inline code/bold/links). No dependency, and nothing
here executes what it renders.
"""

from __future__ import annotations

import html
import os
import re

INLINE_CODE = re.compile(r"`([^`]+)`")
BOLD = re.compile(r"\*\*([^*]+)\*\*")
ITALIC = re.compile(r"(?<![\*\w])\*([^*\n]+)\*(?!\*)")
LINK = re.compile(r"\[([^\]]+)\]\(([^)]+)\)")


def esc(text) -> str:
    return html.escape(str(text), quote=False)


def inline(text: str, link_resolver=None) -> str:
    """Escape, then re-introduce the inline markdown we support.

    `link_resolver(target) -> href|None` decides whether a markdown link
    survives. A link to a document we do not host would 404 for the reader, so
    an unresolved link renders as plain text rather than a dead end.
    """
    out = esc(text)

    def _link(m):
        label, target = m.group(1), m.group(2)
        href = link_resolver(target) if link_resolver else None
        return f'<a href="{esc(href)}">{label}</a>' if href else label

    out = LINK.sub(_link, out)
    out = INLINE_CODE.sub(lambda m: f"<code>{m.group(1)}</code>", out)
    out = BOLD.sub(lambda m: f"<strong>{m.group(1)}</strong>", out)
    out = ITALIC.sub(lambda m: f"<em>{m.group(1)}</em>", out)
    return out


def md_to_html(md: str, link_resolver=None, demote_h1: bool = True) -> str:
    lines = md.splitlines()
    out: list[str] = []
    i = 0
    while i < len(lines):
        line = lines[i]

        if line.startswith("```"):
            lang = line[3:].strip().lower()
            i += 1
            buf = []
            while i < len(lines) and not lines[i].startswith("```"):
                buf.append(esc(lines[i]))
                i += 1
            i += 1
            body = "\n".join(buf)
            if lang == "mermaid":
                # A diagram, not a code sample. The renderer is vendored into the
                # site rather than fetched from a CDN, because this is served
                # where there may be no internet — and if it is absent the block
                # still reads as its own source, which is what it did before.
                out.append(f'<pre class="mermaid">{body}</pre>')
            else:
                out.append("<pre><code>" + body + "</code></pre>")
            continue

        if line.startswith("#"):
            level = len(line) - len(line.lstrip("#"))
            if demote_h1:
                level += 1
            out.append(f"<h{min(level, 6)}>{inline(line.lstrip('#').strip(), link_resolver)}"
                       f"</h{min(level, 6)}>")
            i += 1
            continue

        if line.startswith("|") and i + 1 < len(lines) and \
                set(lines[i + 1].replace("|", "").strip()) <= set("-: "):
            head = [c.strip() for c in line.strip("|").split("|")]
            i += 2
            rows = []
            while i < len(lines) and lines[i].startswith("|"):
                rows.append([c.strip() for c in lines[i].strip("|").split("|")])
                i += 1
            th = "".join(f"<th>{inline(c, link_resolver)}</th>" for c in head)
            body = "".join(
                "<tr>" + "".join(f"<td>{inline(c, link_resolver)}</td>" for c in r) + "</tr>"
                for r in rows)
            out.append(f'<div class="scroll"><table><thead><tr>{th}</tr></thead>'
                       f"<tbody>{body}</tbody></table></div>")
            continue

        if re.match(r"^\s*[-*] ", line):
            items = []
            while i < len(lines) and re.match(r"^\s*[-*] ", lines[i]):
                items.append(f"<li>{inline(re.sub(r'^\s*[-*] ', '', lines[i]), link_resolver)}</li>")
                i += 1
            out.append("<ul>" + "".join(items) + "</ul>")
            continue

        if re.match(r"^\d+\. ", line):
            items = []
            while i < len(lines) and re.match(r"^\d+\. ", lines[i]):
                items.append(f"<li>{inline(re.sub(r'^\d+\. ', '', lines[i]), link_resolver)}</li>")
                i += 1
            out.append("<ol>" + "".join(items) + "</ol>")
            continue

        if line.startswith(">"):
            buf = []
            while i < len(lines) and lines[i].startswith(">"):
                buf.append(inline(lines[i].lstrip("> "), link_resolver))
                i += 1
            out.append("<blockquote>" + " ".join(buf) + "</blockquote>")
            continue

        if not line.strip() or set(line.strip()) <= set("-—_*") and len(line.strip()) > 2:
            if line.strip() and set(line.strip()) <= set("-—_*"):
                out.append("<hr>")
            i += 1
            continue

        buf = []
        while i < len(lines) and lines[i].strip() and \
                not lines[i].startswith(("#", "|", ">", "```")) and \
                not re.match(r"^\s*[-*] |^\d+\. ", lines[i]):
            buf.append(lines[i])
            i += 1
        out.append("<p>" + inline(" ".join(buf), link_resolver) + "</p>")

    return "\n".join(out)


def doc_title(path: str) -> str:
    """The document's own H1, which is what a reader should see instead of a path.

    ONLY for markdown. A script or a config file has no H1 — its first `#` line
    is a comment, and those comments habitually start with the file's own path,
    which is exactly what must never reach a reader. Derive a name from the
    filename instead.
    """
    if not path.endswith(".md"):
        base = os.path.splitext(os.path.basename(path))[0]
        return base.replace("-", " ").replace("_", " ")
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                if line.startswith("# "):
                    return line[2:].strip()
                if line.startswith("---"):
                    continue
    except OSError:
        pass  # cannot read an H1: derive a title from the filename below
    base = os.path.basename(path)
    return os.path.splitext(base)[0].replace("-", " ")
