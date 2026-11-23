# SPDX-License-Identifier: Apache-2.0
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import portal_records as pr  # noqa: E402

A = "a" * 32
DOC = f"""
standards:
  cot: {{format: "xml:event", schema: cot-event.xsd, version: v1}}
  track: {{format: json, schema: processors-track.proto, version: v1}}
prefixes: [{A}/stream/in]
records:
  - {{key: {A}/radar/cot, standard: cot, description: radar}}
  - {{key: "{A}/stream/in/*", standard: track, description: feeds}}
"""


@pytest.fixture()
def rows(tmp_path):
    p = tmp_path / "r.yaml"
    p.write_text(DOC)
    return p


def test_a_wildcard_row_covers_a_feed_added_later(rows, tmp_path):
    f = tmp_path / "vars.yml"
    f.write_text(f'a: "volatile/{A}/stream/in/feed-1,{A}/radar/cot"\nprefix: volatile/{A}/stream/in\nwide: {A}/x/**\n')
    assert pr.undeclared(rows, [f]) == []
    f.write_text(f"key: {A}/radar/other\n")
    assert pr.undeclared(rows, [f]) == [f"{f}: {A}/radar/other"]
    assert pr.main(["check", str(rows), str(f)]) == 1


@pytest.mark.parametrize("key", [f"volatile/{A}/x/y", f"{A}/x/**", f"{A}/x", "short/x/y"])
def test_keys_the_portal_would_refuse(tmp_path, key):
    p = tmp_path / "r.yaml"
    p.write_text(DOC.replace(f"{A}/radar/cot,", f'"{key}",'))
    with pytest.raises(ValueError):
        pr.load(p)


class Fake:
    def __init__(self):
        self.t, self.v, self.calls = {}, {}, []

    def topics(self, org):
        return {k: v for k, v in self.t.items() if k.startswith(org)}

    def versions(self, org, tid):
        return self.v.get(tid, set())

    def call(self, method, path, body=None, ctype=""):
        self.calls.append((method, path))
        if method == "POST" and path.endswith("/topics"):
            import json
            b = json.loads(body)
            t = {"id": len(self.t) + 1, **b}
            self.t[b["key_pattern"]] = t
            return t
        if path.endswith("/schemas"):
            self.v.setdefault(int(path.split("/")[-2]), set()).add("v1")
        return {}


def test_plan_changes_nothing_apply_converges_and_nothing_is_deleted(rows, capsys):
    fake, recs = Fake(), pr.load(rows)
    fake.t[f"{A}/old/topic"] = {"id": 99, "format": "raw", "description": ""}
    assert pr.reconcile(fake, recs, False) == 4 and fake.calls == []
    assert pr.reconcile(fake, recs, True) == 4
    assert pr.reconcile(fake, recs, True) == 0
    assert all(m != "DELETE" for m, _ in fake.calls)
    assert f"other  {A}/old/topic" in capsys.readouterr().out


def test_the_shipped_schemas_have_a_known_kind():
    assert {p.suffix for p in (pr.HERE / "schemas").iterdir()} <= set(pr.KINDS)
