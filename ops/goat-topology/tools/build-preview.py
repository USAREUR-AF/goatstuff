#!/usr/bin/env python3
"""Bundle the surface into one self-contained page for design review.

The service serves markup, styles and ES modules as separate assets. An artifact
host cannot fetch them, so this inlines the module graph in dependency order and
swaps the poll loop for a fixture.

It is a REVIEW tool, not part of the build. The bundling here is the one place
the preview and the product differ, so it does the inlining mechanically and
asserts the result rather than hand-maintaining a copy — a preview that has
drifted from the code is worse than no preview.

usage: build-preview.py <findings.json> <out.html>
  <findings.json> is the engine's own output:
    cargo test -- --ignored dump_for_preview --nocapture
"""
import json, pathlib, re, sys

HERE = pathlib.Path(__file__).resolve().parent.parent      # ops/goat-topology
ROOT = HERE.parent.parent                                   # repo root
STATIC = HERE / "static"


def inline_modules() -> str:
    """Concatenate the ES modules in dependency order, stripping import/export.

    Order is derived from the import graph rather than hardcoded, so adding a
    renderer does not silently leave it out of the preview.
    """
    files = {p.relative_to(STATIC).as_posix(): p.read_text()
             for p in STATIC.rglob("*.js")}
    deps = {}
    for name, body in files.items():
        base = pathlib.PurePosixPath(name).parent
        deps[name] = {
            (base / m).as_posix().replace("/./", "/")
            for m in re.findall(r'from\s+"([^"]+)"', body)
        }
    # Normalise ".." segments the same way a browser would.
    deps = {k: {pathlib.PurePosixPath(v).as_posix() for v in vs} for k, vs in deps.items()}
    deps = {k: {re.sub(r"^\.\./", "", v).lstrip("./") for v in vs} for k, vs in deps.items()}

    order, seen = [], set()

    def visit(n):
        if n in seen or n not in files:
            return
        seen.add(n)
        for d in sorted(deps.get(n, ())):
            visit(d)
        order.append(n)

    for n in sorted(files):
        visit(n)

    out = []
    for n in order:
        src = files[n]
        src = re.sub(r"^import .*?;\s*$", "", src, flags=re.M)   # drop imports
        src = re.sub(r"^export (const|function|let) ", r"\1 ", src, flags=re.M)
        # `import * as graph from ...` became a namespace; flatten the calls.
        out.append(f"/* ---- {n} ---- */\n{src}")
    return "\n".join(out)


def main() -> int:
    findings_json, out_path = sys.argv[1], sys.argv[2]
    html = (STATIC / "index.html").read_text()
    css = (STATIC / "app.css").read_text()
    ds = "\n".join(
        (ROOT / "docs/design-system" / f).read_text()
        for f in ("colors_and_type.css", "components.css")
    )
    body = html[html.index("<body>") + 6 : html.rindex("</body>")]
    body = body[: body.index("<script")].rstrip()

    with open(findings_json) as source_file:
        payload = json.load(source_file)

    page = (
        "<title>Topology Surface</title>\n"
        f"<style>\n{ds}\n{css}\n</style>\n"
        + BANNER
        + body
        + "\n<script>\n"
        + '"use strict";\nconst PREVIEW_DATA = '
        + json.dumps(payload)
        + ";\n"
        + inline_modules()
        + PREVIEW_DRIVER
        + "\n</script>\n"
    )

    # The artifact host supplies the document skeleton; a second one leaves the
    # browser recovering from nested documents, which is how a structural break
    # once rendered as scrambled output rather than a blank page.
    assert "<!doctype" not in page.lower(), "preview must carry no doctype"
    assert "<body>" not in page, "preview must carry no body tag"
    assert page.count("<script>") == 1 == page.count("</script>")
    js = page[page.index("<script>") : page.index("</script>")]
    assert js.count("`") % 2 == 0, "unbalanced template literals"
    for fn in ("function renderGraph", "function projectCarve"):
        assert fn in js, f"{fn} missing from the bundle"

    pathlib.Path(out_path).write_text(page)
    print(f"wrote {out_path} ({len(page)} bytes)")
    return 0


BANNER = (
    '<div style="background:var(--status-warn-bg);border-bottom:1px solid var(--c-amber);'
    'padding:9px 32px;font-family:var(--font-mono);font-size:12px;letter-spacing:.06em;'
    'color:var(--c-amber-hi)">PREVIEW — EXAMPLE DATA, bundled from the real modules. '
    "Router shapes come from the M-111 census; the carved gateway and the quiet publisher "
    "are invented so every finding has something to say. Finding text is the engine's own "
    "output. Click a router to plan a carve.</div>\n"
)

PREVIEW_DRIVER = """
/* ---- preview driver: the fixture stands in for the poll loop ---- */
const picks = new Set();
function paintAll() {
  renderVerdict(PREVIEW_DATA);
  renderGraph(PREVIEW_DATA, picks);
  renderFindingFilters(PREVIEW_DATA);
  renderFindings(PREVIEW_DATA);
  renderFeed(PREVIEW_DATA);
  renderRouters(PREVIEW_DATA);
  renderPlanner(PREVIEW_DATA, picks);
  const sites = [...new Set(PREVIEW_DATA.routers.map(r => r.site).filter(Boolean))];
  $("#scope").textContent = sites.join(" · ");
  $("#clock").textContent = "preview";
}
function togglePick(n) { picks.has(n) ? picks.delete(n) : picks.add(n); paintAll(); }
onNodeClick(togglePick); onSelectionChange(togglePick);
onFilterChange(() => { renderFindingFilters(PREVIEW_DATA); renderFindings(PREVIEW_DATA); });
paintAll();
"""

if __name__ == "__main__":
    raise SystemExit(main())
