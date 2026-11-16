# SPDX-License-Identifier: Apache-2.0
"""The packs proposed for goat-transformers load and reproduce their vectors.

ops/xf/upstream-proposed/ holds a model module and a pack written here and
offered to the library. Until the library carries them, this test is what
runs their conformance vectors: each `{from, to, input, expect}` file under
`upstream-proposed/packs/*/vectors/`, on the library's Python engine, with the
formats assembled the way a consumer's bundle is.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

XF = Path(__file__).resolve().parents[1]
REPO = XF.parents[1]
PROPOSED = XF / "upstream-proposed"
VECTORS = sorted(PROPOSED.glob("packs/*/vectors/*.json"))
BUNDLES = [REPO / "ops/serve/goat-map/xf-bundle", REPO / "ops/zenoh-shims/transform-shim/bundle"]


@pytest.fixture(scope="module")
def engine():
    gt = pytest.importorskip("goat_transformers")
    # The transform-shim bundle holds every format the vectors name.
    bundle = BUNDLES[1]
    return gt.Engine.from_dir(str(bundle / "packs"), str(bundle / "model"),
                              max_distribution="public")


def test_there_are_vectors():
    assert len(VECTORS) >= 7


@pytest.mark.parametrize("path", VECTORS, ids=lambda p: f"{p.parts[-3]}/{p.stem}")
def test_vector(engine, path):
    from goat_transformers import GTError

    vector = json.loads(path.read_text())
    assert path.parts[-3] == vector["from"], "a vector lives with the pack that owns its input"
    call = lambda: engine.convert(  # noqa: E731
        vector["from"], vector["to"], vector["input"], vector.get("context"))
    if "expect_error" in vector:
        with pytest.raises(GTError) as caught:
            call()
        assert str(caught.value).startswith(vector["expect_error"] + ":")
    else:
        assert call().decode() == vector["expect"]


@pytest.mark.parametrize("bundle", BUNDLES, ids=lambda p: p.parent.name)
def test_bundle_matches_its_manifest_and_the_proposed_sources(bundle):
    done = subprocess.run([sys.executable, str(XF / "bundle.py"), "check", str(bundle)],
                          capture_output=True, text=True)
    assert done.returncode == 0, done.stderr
