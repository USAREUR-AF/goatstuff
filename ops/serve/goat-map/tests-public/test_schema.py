# SPDX-License-Identifier: Apache-2.0
"""The declared schema is the contract, and these tests are what keep it one.

fabric-schema-evolution.md principle 1 is "every topic has a declared schema —
no undeclared-shape publishes ever reach the fabric". A schema file that
nothing validates against is a comment, so the transform's real output is
checked against the real file here.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from goatmap import cot  # noqa: E402

from test_cot import ANCHOR, JAMMING, SOURCE, TRACK  # noqa: E402

SCHEMA_PATH = ROOT / "schemas" / "geo" / "track.v1.schema.json"


@pytest.fixture(scope="module")
def schema() -> dict:
    return json.loads(SCHEMA_PATH.read_text())


@pytest.fixture(scope="module")
def validator(schema):
    jsonschema = pytest.importorskip("jsonschema")
    return jsonschema.Draft202012Validator(schema)


def test_schema_file_is_itself_valid(schema):
    jsonschema = pytest.importorskip("jsonschema")
    jsonschema.Draft202012Validator.check_schema(schema)


def test_transform_output_validates(validator):
    track = cot.parse(TRACK, kind="track", source=SOURCE, producer_anchor=ANCHOR)
    errors = sorted(validator.iter_errors(track), key=lambda e: e.path)
    assert not errors, "\n".join(f"{list(e.path)}: {e.message}" for e in errors)


def test_store_enriched_output_validates(validator):
    # The store adds trail/updates/first_seen_at after the parser runs, and
    # those are what actually go on the wire — so validate the enriched shape,
    # not just the parser's.
    from goatmap.store import TrackStore

    store = TrackStore()
    track = store.upsert(cot.parse(TRACK, kind="track", source=SOURCE, producer_anchor=ANCHOR))
    errors = sorted(validator.iter_errors(track), key=lambda e: e.path)
    assert not errors, "\n".join(f"{list(e.path)}: {e.message}" for e in errors)


def test_schema_rejects_a_position_free_record(validator):
    # Belt and braces on the behaviour test in test_cot: even if the parser
    # ever let a position-free record through, the schema would refuse it.
    assert cot.parse(JAMMING, kind="alert", source=SOURCE, producer_anchor=ANCHOR) is None
    bad = {
        "schema": "goat.geo.track.v1",
        "id": "radar-a:1",
        "label": "1",
        "kind": "alert",
        "source": SOURCE,
        "observed_at": "2030-01-05T12:00:00.000Z",
        "producer_anchor": ANCHOR,
        "synthetic": False,
    }
    assert list(validator.iter_errors(bad)), "schema must require lat/lon"


def test_schema_rejects_an_unknown_affiliation(validator):
    track = cot.parse(TRACK, kind="track", source=SOURCE, producer_anchor=ANCHOR)
    track["affiliation"] = "definitely-not-a-cot-affiliation"
    assert list(validator.iter_errors(track))


def test_registry_copy_matches_producer_copy():
    """While the service still lives inside the monorepo, the fabric-wide
    registry copy and the producer's copy must be byte-identical.

    After extraction to its own repo this test cannot see the registry and
    skips; keeping the two in sync then needs a check on the monorepo side.
    """
    registry = ROOT.parents[2] / "docs" / "design" / "schemas" / "geo" / "track.v1.schema.json"
    if not registry.exists():
        pytest.skip("registry copy not visible — service extracted to its own repo")
    assert registry.read_text() == SCHEMA_PATH.read_text(), (
        "schema drift between the producer copy and the fabric registry copy"
    )
