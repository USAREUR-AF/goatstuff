# SPDX-License-Identifier: Apache-2.0
"""The library path for CoT -> GeoTrackV1 (`GOATMAP_COT_ENGINE=library`).

`goatmap/cot_library.py` does the job of `cot.parse` with the goat-transformers
packs in `xf-bundle/`. These tests hold it to the behaviours `test_cot.py`
pins for the built-in path, validate its output against the declared schema,
and compare the two paths member by member against a recorded baseline
(`cot_parity_baseline.json`, written by `tools/cot_parity.py --write`).
"""

from __future__ import annotations

import json
import shutil
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "tools"))

from goatmap import cot, cot_library  # noqa: E402

from test_cot import ANCHOR, JAMMING, SOURCE, TRACK  # noqa: E402


def parse(payload: bytes = TRACK, kind: str = "track"):
    return cot_library.parse(payload, kind=kind, source=SOURCE, producer_anchor=ANCHOR)


def test_position_identity_and_kinematics():
    t = parse()
    assert t["lat"] == pytest.approx(0.144703641604)
    assert t["lon"] == pytest.approx(0.6869426472395)
    assert t["alt_m"] == pytest.approx(16.7187347412109)
    assert (t["id"], t["label"], t["kind"]) == ("radar-a:177", "177", "track")
    assert t["source"] == SOURCE
    assert t["course_deg"] == pytest.approx(65.0)
    assert t["speed_mps"] == pytest.approx(12.8682579750276)


def test_no_position_is_dropped_not_placed_at_null_island():
    assert parse(JAMMING, kind="alert") is None


@pytest.mark.parametrize("bad", [b"", b"not xml at all", b"<event><point/>", b"<other/>"])
def test_unparseable_returns_none_rather_than_raising(bad):
    assert parse(bad) is None


def test_non_numeric_position_is_dropped():
    assert parse(TRACK.replace(b'lat="0.144703641604"', b'lat="north-ish"')) is None


def test_type_letters_resolve_and_degrade():
    t = parse(TRACK.replace(b'type="a-u-X"', b'type="a-h-G-U-C-I"'))
    assert (t["affiliation"], t["dimension"]) == ("hostile", "ground")
    t = parse(TRACK.replace(b'type="a-u-X"', b'type="a-Z-Q"'))
    assert (t["affiliation"], t["dimension"]) == ("unknown", "unknown")


def test_absent_detail_track_yields_none_not_zero():
    t = parse(TRACK.replace(b'<track course="65" speed="12.8682579750276"/>', b""))
    assert t["course_deg"] is None and t["speed_mps"] is None


def test_error_sentinel_becomes_none_and_real_estimate_is_kept():
    t = parse()
    assert t["ce_m"] is None and t["le_m"] is None
    assert parse(TRACK.replace(b'ce="9999999"', b'ce="25.5"'))["ce_m"] == pytest.approx(25.5)


def test_the_two_clocks_are_separate():
    t = parse()
    assert t["reported_at"] == "1970-01-01T02:47:21.682Z"
    assert t["observed_at"].endswith("Z") and t["observed_at"] > "2026-01-01T00:00:00Z"


def test_every_documented_member_is_emitted_and_serialisable():
    assert set(parse()) == set(cot.parse(TRACK, kind="track", source=SOURCE,
                                         producer_anchor=ANCHOR))
    json.dumps(parse())


def test_output_validates_against_the_declared_schema():
    jsonschema = pytest.importorskip("jsonschema")
    schema = json.loads((ROOT / "schemas" / "geo" / "track.v1.schema.json").read_text())
    errors = list(jsonschema.Draft202012Validator(schema).iter_errors(parse()))
    assert not errors, "\n".join(f"{list(e.path)}: {e.message}" for e in errors)


def test_an_event_without_an_identifier_is_refused():
    # The built-in path publishes id "radar-a:", which the schema's id pattern
    # refuses. The packs give such an event no identity and no track.
    assert parse(TRACK.replace(b' uid="177"', b"")) is None


def test_parity_with_the_builtin_path_matches_the_recorded_baseline():
    import cot_parity

    saved = cot.now_iso
    try:
        report = cot_parity.measure()
    finally:
        cot.now_iso = saved
    assert report == json.loads(cot_parity.BASELINE.read_text())
    # The service's path equals the built-in path on every member, a number
    # written 65 for 65.0 aside.
    assert report["service"]["different"] == 0 and report["service"]["absent"] == 0
    # Every difference of the packs alone has a named cause, and the two paths
    # disagree on one input: the event with no identifier.
    assert "unexplained" not in report["packs"]["by_cause"]
    assert [d["input"] for d in report["messages"]["disagree"]] == ["no-uid"]


def test_the_packs_alone_give_the_descriptor_the_sender_text_and_an_empty_type():
    context = {"geotrack.kind": "track", "geotrack.sensor": "radar-a", "geotrack.site": "s",
               "geotrack.node": "n", "geotrack.observed_at": "2030-01-05T12:00:00.000Z",
               "geotrack.producer_anchor": ANCHOR, "geotrack.synthetic": 0}
    payload = (TRACK.replace(b'time="1970-01-01T02:47:21.682Z"', b'time="1970-01-01T02:47:21Z"')
               .replace(b'type="a-u-X" ', b""))
    t = cot_library.convert(payload, context)
    assert t["descriptor"] == "radar/radar-a:Tracks with Associated Plot Data:177"
    assert t["reported_at"] == "1970-01-01T02:47:21Z"  # as the sensor wrote it
    assert t["cot_type"] == ""


def test_completion_step_type_letters():
    for cot_type, want in (("a-h-G-U-C-I", ("hostile", "ground")), ("a-f-A", ("friend", "air")),
                           ("a-Z-Q", ("unknown", "unknown")), ("a", ("unknown", "unknown")),
                           ("", ("unknown", "unknown")), (None, ("unknown", "unknown"))):
        t = {"cot_type": cot_type}
        cot_library.complete_type_letters(t)
        assert (t["affiliation"], t["dimension"]) == want, cot_type


def test_completion_step_error_sentinel():
    t = {"ce_m": 9999999, "le_m": 10000000.0}
    cot_library.complete_error_sentinel(t)
    assert t == {"ce_m": None, "le_m": None}
    t = {"ce_m": 25.5}
    cot_library.complete_error_sentinel(t)
    assert t == {"ce_m": 25.5}


def test_completion_step_null_members():
    t = {"lat": 1.0}
    cot_library.complete_null_members(t)
    assert set(t) == set(cot_library.FIELDS)
    assert t["lat"] == 1.0 and t["org"] is None


def test_the_packs_leave_exactly_what_the_completion_steps_name():
    # Take a step away and the member it owns is the only thing that changes.
    # The pack leaves 9999999 absent; complete_error_sentinel owns a larger value.
    above = TRACK.replace(b'ce="9999999"', b'ce="10000000"').replace(b'le="9999999"', b'le="1e8"')
    for step, members, payload in (
            (cot_library.complete_type_letters, {"affiliation", "dimension"}, TRACK),
            (cot_library.complete_error_sentinel, {"ce_m", "le_m"}, above),
            # 9999999 in ce and le is absent from the packs, so null comes from here.
            (cot_library.complete_null_members, {"org", "ce_m", "le_m"}, TRACK)):
        full = parse(payload)
        saved = cot_library.COMPLETION
        cot_library.COMPLETION = tuple(s for s in saved if s is not step)
        try:
            partial = cot_library.convert(payload, {
                "geotrack.kind": "track", "geotrack.sensor": "radar-a",
                "geotrack.site": SOURCE["site"], "geotrack.node": SOURCE["node"],
                "geotrack.observed_at": full["observed_at"], "geotrack.producer_anchor": ANCHOR,
                "geotrack.synthetic": 0})
            for s in cot_library.COMPLETION:
                s(partial)
        finally:
            cot_library.COMPLETION = saved
        partial["source"] = SOURCE
        changed = {m for m in cot_library.FIELDS if partial.get(m, "missing") != full[m]}
        assert changed == members, step.__name__


def test_switch_selects_the_parser_and_refuses_an_unknown_name():
    assert cot_library.parser_for("builtin") is cot.parse
    assert cot_library.parser_for("library") is cot_library.parse
    with pytest.raises(ValueError):
        cot_library.parser_for("both")


def test_an_altered_bundle_does_not_load(tmp_path):
    bundle = tmp_path / "xf-bundle"
    shutil.copytree(cot_library.BUNDLE, bundle)
    pack = bundle / "packs" / "geo-track-v1" / "pack.def"
    pack.write_text(pack.read_text() + "# changed after the manifest was written\n")
    with pytest.raises(cot_library.BundleError):
        cot_library.load_engine(bundle)


def test_the_bundle_loads_only_public_packs():
    engine = cot_library.load_engine()
    assert engine.max_distribution == "public"
    assert engine.formats() == ["cot-xml", "geo-track-v1"]
