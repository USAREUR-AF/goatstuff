# SPDX-License-Identifier: Apache-2.0
"""Cursor-on-Target XML -> GeoTrackV1 through the goat-transformers library.

The same job as `cot.parse`, done by data instead of code: the library's
Python engine decodes the event with the `cot-xml` pack and encodes it with
the `geo-track-v1` pack (docs/design/format-transformers.md in goatstuff).
The default; `GOATMAP_COT_ENGINE=builtin` selects the hand-written
`cot.parse` instead.

What the packs cannot state is completed here by three named steps
(`COMPLETION`), each of which names the library feature that would remove it.
`tools/cot_parity.py` counts what each stage produces.

Non-positional status messages stay with `cot.parse_status`.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any

from . import cot

BUNDLE = Path(__file__).resolve().parents[1] / "xf-bundle"

# Every member `cot.parse` emits, in its order. The json wire writes no member
# for an absent key; the map's contract is that every member is present and
# `null` means not reported.
FIELDS = (
    "schema", "id", "label", "kind", "source", "lat", "lon", "alt_m",
    "course_deg", "speed_mps", "ce_m", "le_m", "cot_type", "affiliation",
    "dimension", "how", "descriptor", "reported_at", "stale_at", "observed_at",
    "producer_anchor", "synthetic", "org",
)

_engine = None


class BundleError(RuntimeError):
    """The pack bundle is missing, altered or does not load."""


def load_engine(bundle: Path = BUNDLE):
    """Load the bundle after checking every file against its manifest.

    Raises BundleError: a service asked to use the library must not start on
    packs other than the ones it was built with.
    """
    try:
        from goat_transformers import Engine, GTError
    except ImportError as exc:  # pragma: no cover - exercised by deployment, not tests
        raise BundleError(f"goat-transformers is not installed: {exc}") from exc
    try:
        manifest = json.loads((bundle / "MANIFEST.json").read_text(encoding="utf-8"))
        for entry in manifest["files"]:
            digest = hashlib.sha256((bundle / entry["path"]).read_bytes()).hexdigest()
            if digest != entry["sha256"]:
                raise BundleError(f"{entry['path']} does not match the bundle manifest")
        return Engine.from_dir(
            str(bundle / "packs"), str(bundle / "model"), max_distribution="public"
        )
    except (OSError, ValueError, KeyError, GTError) as exc:
        raise BundleError(f"pack bundle {bundle} does not load: {exc}") from exc


def convert(xml_bytes: bytes, context: dict[str, Any]) -> dict[str, Any] | None:
    """What the packs alone produce: the engine's output, parsed, or None when
    the engine refuses the message. No completion."""
    global _engine
    if _engine is None:
        _engine = load_engine()
    from goat_transformers import GTError

    try:
        return json.loads(_engine.convert("cot-xml", "geo-track-v1", xml_bytes, context))
    except GTError:
        return None


def complete_type_letters(track: dict[str, Any]) -> None:
    """`affiliation` and `dimension`: the second and third token of the CoT
    type, each looked up in a table; `unknown` for a missing or unlisted token.

    Needs in the library: a derive function `token:<sep>:<n>` (the n-th piece,
    from 0, of a string split at `<sep>`; absent when there are fewer pieces),
    and a value-table line that gives the result for a value with no entry.
    """
    parts = (track.get("cot_type") or "").split("-")
    track["affiliation"] = cot._AFFILIATION.get(parts[1] if len(parts) > 1 else "", "unknown")
    track["dimension"] = cot._DIMENSION.get(parts[2] if len(parts) > 2 else "", "unknown")


def complete_error_sentinel(track: dict[str, Any]) -> None:
    """`ce_m` and `le_m`: a value above 9999999, CoT's "no usable estimate",
    the track says null, as `cot.parse` does for any value at or above it.

    The `cot-xml` pack leaves the key absent for the text 9999999 itself (its
    `absent` option), so `complete_null_members` writes that one null; this
    step is left only for a larger number, which the pack keeps as a value.
    Needs in the library: a `field` option that leaves the key absent for
    every value at or above a stated one.
    """
    for key in ("ce_m", "le_m"):
        if track.get(key) is not None and track[key] >= cot._ERROR_UNKNOWN:
            track[key] = None


def complete_null_members(track: dict[str, Any]) -> None:
    """Every member is present, and null means not reported. The `json` wire
    writes no member for an absent key.

    Needs in the library: a `json` pack directive that writes `null` for a
    mapped member whose key is absent.
    """
    for member in FIELDS:
        track.setdefault(member, None)


# What the packs do not produce, in the order it is applied.
COMPLETION = (complete_type_letters, complete_error_sentinel, complete_null_members)


def complete(track: dict[str, Any]) -> dict[str, Any]:
    out = dict(track)
    for step in COMPLETION:
        step(out)
    # Members in the order `cot.parse` writes them.
    return {m: out[m] for m in FIELDS if m in out} | out


def parse(
    xml_bytes: bytes,
    *,
    kind: str,
    source: dict[str, str],
    producer_anchor: str,
    synthetic: bool = False,
    org: str | None = None,
) -> dict[str, Any] | None:
    """`cot.parse`, with the same arguments and the same result shape."""
    context: dict[str, Any] = {
        "geotrack.kind": kind,
        "geotrack.sensor": source.get("sensor", "sensor"),
        "geotrack.observed_at": cot.now_iso(),
        "geotrack.producer_anchor": producer_anchor,
        "geotrack.synthetic": 1 if synthetic else 0,
    }
    for member in ("site", "node", "goatnet"):
        if source.get(member) is not None:
            context[f"geotrack.{member}"] = source[member]
    if org is not None:
        context["geotrack.org"] = org
    track = convert(xml_bytes, context)
    if track is None:
        return None
    out = complete(track)
    # The caller's `source` object, as `cot.parse` returns it.
    out["source"] = source
    return out


def parser_for(engine: str):
    """The CoT parser `GOATMAP_COT_ENGINE` names. `library` loads the bundle
    now, so a bundle that does not load stops the service at start."""
    if engine == "builtin":
        return cot.parse
    if engine == "library":
        global _engine
        _engine = load_engine()
        return parse
    raise ValueError(f"GOATMAP_COT_ENGINE must be builtin or library, got {engine!r}")
