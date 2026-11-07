#!/usr/bin/env python3
"""Refuse any goat-map READ of the forgeable groups header.

oauth2-proxy runs as a REVERSE PROXY: it strips client-supplied `X-Forwarded-*`
and injects its own from the validated session, but it does NOT strip a
client-supplied `X-Auth-Request-*` from the upstream request — it only sets that
family on the `/oauth2/auth` subrequest RESPONSE.

So reading `x-auth-request-groups` AT ALL is a privilege-escalation hole: a
logged-in floor viewer sends `X-Auth-Request-Groups: ADMINS` and becomes full
scope. Found live in panoscope and fixed there; this guard stops
goat-map growing the same bug independently.

Parses the AST rather than grepping, because the code that MUST mention the
header is the code explaining why it is dangerous — a text search cannot tell
that prose from a lookup, and a guard that cries wolf on its own documentation
gets disabled. Docstrings and comments are invisible to this check; string
literals in executable positions are not.
"""

from __future__ import annotations

import ast
import sys
from pathlib import Path

FORBIDDEN = {"x-auth-request-groups", "x-auth-request-email"}
# The one binding allowed to hold the name, so the reason travels with the code.
ALLOWED_ASSIGN_TARGETS = {"FORGEABLE_HEADER_DO_NOT_READ"}


def offenders(path: Path) -> list[tuple[int, str]]:
    try:
        tree = ast.parse(path.read_text(encoding="utf-8"))
    except SyntaxError as exc:
        return [(exc.lineno or 0, f"unparseable: {exc}")]

    # Names bound to the forbidden constant deliberately (the do-not-read marker).
    allowed_nodes: set[int] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Assign):
            names = {t.id for t in node.targets if isinstance(t, ast.Name)}
            if names & ALLOWED_ASSIGN_TARGETS and isinstance(node.value, ast.Constant):
                allowed_nodes.add(id(node.value))

    # Docstrings are not reads.
    docstrings: set[int] = set()
    for node in ast.walk(tree):
        if isinstance(node, (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            body = getattr(node, "body", [])
            if body and isinstance(body[0], ast.Expr) and isinstance(body[0].value, ast.Constant) \
               and isinstance(body[0].value.value, str):
                docstrings.add(id(body[0].value))

    hits: list[tuple[int, str]] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Constant) or not isinstance(node.value, str):
            continue
        if id(node) in allowed_nodes or id(node) in docstrings:
            continue
        if node.value.strip().lower() in FORBIDDEN:
            hits.append((node.lineno, node.value))
    return hits


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    target = root / "ops" / "serve" / "goat-map"
    if not target.is_dir():
        print("check-goatmap-groups-header: no goat-map tree, skipping")
        return 0

    failed = False
    for py in sorted(target.rglob("*.py")):
        for lineno, val in offenders(py):
            failed = True
            print(f"{py.relative_to(root)}:{lineno}: reads forbidden header {val!r}",
                  file=sys.stderr)

    scope = target / "goatmap" / "scope.py"
    if scope.exists() and "x-forwarded-groups" not in scope.read_text(encoding="utf-8"):
        print("check-goatmap-groups-header: scope.py no longer reads x-forwarded-groups",
              file=sys.stderr)
        failed = True

    if failed:
        print("\ncheck-goatmap-groups-header: FAIL — use x-forwarded-groups "
              "(sidecar-injected, non-forgeable). See goatmap/scope.py, F-244 / M-079.",
              file=sys.stderr)
        return 1

    print("check-goatmap-groups-header: OK — trusted header only.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
