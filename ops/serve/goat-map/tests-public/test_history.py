# SPDX-License-Identifier: Apache-2.0
"""Track history for the overlay: sampling, retention, window and scoping fields."""

from goatmap.store import TrackStore


def _t(tid, lon, lat, **kw):
    t = {"id": tid, "lon": lon, "lat": lat, "observed_at": "2026-01-01T00:00:00Z",
         "org": "org-a", "producer_anchor": "a1", "affiliation": "unknown", "label": tid}
    t.update(kw)
    return t


def test_history_samples_one_point_per_step():
    s = TrackStore(history_step_s=5)
    for i in range(10):  # ten reports one second apart
        s._record_history(_t("x", 10 + i * 0.01, 45), now=1000 + i)
    pts = s.history(3600, now=1010)[0]["points"]
    # 1000..1004 collapse into one point, 1005..1009 into a second
    assert len(pts) == 2
    assert pts[-1][0] == 10 + 9 * 0.01  # the newest position survives in its step


def test_history_window_and_retention():
    s = TrackStore(history_s=3600, history_step_s=1)
    s._record_history(_t("old", 1, 1), now=0)
    s._record_history(_t("new", 2, 2), now=3000)
    assert {e["id"] for e in s.history(600, now=3100)} == {"new"}
    assert {e["id"] for e in s.history(3600, now=3100)} == {"old", "new"}
    s.prune_history(now=3700)
    assert {e["id"] for e in s.history(3600, now=3700)} == {"new"}


def test_history_outlives_ttl_and_keeps_scoping_fields():
    s = TrackStore(ttl_s=0.0)
    s.upsert(_t("x", 1, 1))
    s.expire()
    assert len(s) == 0
    (e,) = s.history(3600)
    assert e["org"] == "org-a" and e["producer_anchor"] == "a1" and e["affiliation"] == "unknown"


def test_history_forgets_least_recent_track_at_ceiling():
    s = TrackStore(history_max_tracks=2, history_step_s=1)
    s._record_history(_t("a", 1, 1), now=1)
    s._record_history(_t("b", 1, 1), now=2)
    s._record_history(_t("c", 1, 1), now=3)
    assert {e["id"] for e in s.history(3600, now=4)} == {"b", "c"}
