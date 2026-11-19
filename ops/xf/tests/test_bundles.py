# SPDX-License-Identifier: Apache-2.0
"""Both pack bundles match their manifests and the pinned library commit.

`bundle.py check` without --library: every file's SHA-256 matches the
manifest, no file is unlisted, every file comes from the library, and the
manifest names the commit in ops/xf/goat-transformers.ref. CI runs the same
check; the comparison with a library checkout (--library) needs the checkout.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

XF = Path(__file__).resolve().parents[1]
REPO = XF.parents[1]
BUNDLES = [REPO / "ops/serve/goat-map/xf-bundle", REPO / "ops/zenoh-shims/transform-shim/bundle"]


def check(bundle: Path) -> subprocess.CompletedProcess:
    return subprocess.run([sys.executable, str(XF / "bundle.py"), "check", str(bundle)],
                          capture_output=True, text=True)


@pytest.mark.parametrize("bundle", BUNDLES, ids=lambda p: p.parent.name)
def test_bundle_matches_its_manifest_and_the_pin(bundle):
    done = check(bundle)
    assert done.returncode == 0, done.stderr
    manifest = json.loads((bundle / "MANIFEST.json").read_text())
    assert manifest["library"]["rev"] == (XF / "goat-transformers.ref").read_text().strip()
    assert {f["origin"] for f in manifest["files"]} == {"library"}


def test_check_refuses_a_file_that_is_not_the_library_s(tmp_path):
    bundle = tmp_path / "bundle"
    shutil.copytree(BUNDLES[0], bundle)
    manifest = json.loads((bundle / "MANIFEST.json").read_text())
    manifest["files"][0]["origin"] = "local"
    (bundle / "MANIFEST.json").write_text(json.dumps(manifest))
    assert check(bundle).returncode == 1
