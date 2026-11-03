# SPDX-License-Identifier: Apache-2.0
"""The cache policy has to cover OUR code, not just the vendored bundle.

Without the policy, app.js / lasso.js / readiness.js falls through the
middleware with no Cache-Control at all. "No header" does not mean "do not
cache" — browsers apply heuristic freshness and serve a stale bundle without
revalidating, so after a successful deploy the operator's browser keeps running
the previous deploy's JavaScript while the host serves the new bytes.

These assert the RULE rather than the implementation, so the policy cannot be
narrowed later without a test saying so.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

SRC = (Path(__file__).resolve().parents[1] / "goatmap" / "__main__.py").read_text()
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))


def _middleware_body() -> str:
    m = re.search(r"async def cache_headers\(.*?\n(.*?)\n    app\.middlewares", SRC, re.S)
    assert m, "cache_headers middleware not found — did it move?"
    return m.group(1)


def test_our_own_assets_are_revalidated():
    """js/css/html must carry a policy that forces a freshness check."""
    body = _middleware_body()
    assert '".js"' in body or "'.js'" in body, (
        "no branch covers .js — our own code would fall through with no "
        "Cache-Control, which browsers treat as heuristically cacheable")
    assert "no-cache" in body


def test_revalidate_not_no_store():
    """`no-store` would refetch the whole bundle every load.

    The middleware exists because that cost the degraded path a page that never
    finished. `no-cache` revalidates and takes a 304, which keeps both
    properties: always fresh, still cheap.
    """
    # Inspect the ASSIGNMENTS only. Prose explaining why no-store was rejected
    # legitimately contains the string, and a test that reads comments would
    # fail on its own documentation.
    body = _middleware_body()
    assigns = [ln.strip() for ln in body.splitlines()
               if "Cache-Control" in ln and "=" in ln]
    assert assigns, "no Cache-Control assignments found"
    ours = [a for a in assigns if "no-cache" in a]
    assert ours, "nothing assigns no-cache — our assets are not revalidated"
    assert not any("no-store" in a and "no-cache" in a for a in assigns), (
        "no-store on our assets refetches ~900 KB every reload on a link that "
        "already could not finish the page")


def test_vendored_bundle_stays_immutable():
    """The long cache on vendor/basemap is the whole point; do not lose it."""
    body = _middleware_body()
    assert "immutable" in body and "/vendor/" in body


def test_live_data_is_never_cached():
    body = _middleware_body()
    assert "no-store" in body and "/api/" in body
