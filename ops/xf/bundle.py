#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Build and check a pinned bundle of goat-transformers packs.

A consumer of the goat-transformers library (docs/design/format-transformers.md)
loads formats from data files: model modules and packs. A bundle is the exact
set of those files one consumer loads, copied into the consumer's directory so
its image build needs nothing outside that directory, with a MANIFEST.json that
names the library commit and the SHA-256 of every file.

  bundle.py build --library <checkout> --out <dir> --formats cot-xml,geo-track-v1
  bundle.py check <dir> [--library <checkout>]

`build` refuses a library checkout that is not at the commit in
ops/xf/goat-transformers.ref. A file under ops/xf/upstream-proposed/ is taken
from there and marked `upstream-proposed` in the manifest: a pack or module
the library does not hold, or a library file with lines added. The model
modules are `core` plus every module a chosen pack or module names in a `uses`
line.

`check` recomputes every hash and refuses a file the manifest does not list.
With --library it also compares every library file with the checkout and
every proposed file with ops/xf/upstream-proposed/, and refuses a proposed
file that drops or changes a line of the library file it replaces.

Standard library only. Exit 0 on success, 1 on any finding.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REF_FILE = HERE / "goat-transformers.ref"
PROPOSED = HERE / "upstream-proposed"
REPOSITORY = "https://github.com/USAREUR-AF/goat-transformers"
MANIFEST = "MANIFEST.json"


def pinned_rev() -> str:
    return REF_FILE.read_text(encoding="utf-8").strip()


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def uses_of(def_file: Path) -> list[str]:
    out = []
    for line in def_file.read_text(encoding="utf-8").splitlines():
        tokens = line.split("#", 1)[0].split()
        if len(tokens) == 2 and tokens[0] == "uses":
            out.append(tokens[1])
    return out


def find(rel: str, library: Path) -> tuple[Path, str]:
    """The source of one bundle file: the proposed set first, then the library."""
    if (PROPOSED / rel).is_file():
        return PROPOSED / rel, "upstream-proposed"
    if (library / rel).is_file():
        return library / rel, "library"
    raise SystemExit(f"bundle: {rel} is in neither {library} nor {PROPOSED}")


def adds_lines_only(proposed: Path, original: Path) -> bool:
    """Every line of `original` is in `proposed`, in the same order."""
    lines = iter(proposed.read_text(encoding="utf-8").splitlines())
    return all(any(line == have for have in lines)
               for line in original.read_text(encoding="utf-8").splitlines())


def library_rev(library: Path) -> str:
    try:
        return subprocess.run(
            ["git", "-C", str(library), "rev-parse", "HEAD"],
            check=True, capture_output=True, text=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError) as exc:
        raise SystemExit(f"bundle: cannot read the commit of {library}: {exc}")


def build(library: Path, out: Path, formats: list[str]) -> int:
    rev = pinned_rev()
    have = library_rev(library)
    if have != rev:
        print(f"bundle: {library} is at {have}; {REF_FILE.name} pins {rev}", file=sys.stderr)
        return 1
    rels = [f"packs/{f}/pack.def" for f in formats]
    modules, queue = [], ["core"]
    for rel in rels:
        queue += uses_of(find(rel, library)[0])
    while queue:
        name = queue.pop(0)
        if name in modules:
            continue
        modules.append(name)
        queue += uses_of(find(f"model/{name}.def", library)[0])
    rels += [f"model/{m}.def" for m in modules]
    for stale in ("model", "packs"):  # a rebuild leaves no file of the previous set
        shutil.rmtree(out / stale, ignore_errors=True)
    files = []
    for rel in sorted(rels):
        src, origin = find(rel, library)
        dst = out / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.write_bytes(src.read_bytes())
        files.append({"path": rel, "sha256": sha256(dst), "origin": origin})
    manifest = {
        "library": {"repository": REPOSITORY, "rev": rev},
        "formats": sorted(formats),
        "files": files,
    }
    (out / MANIFEST).write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"bundle: wrote {len(files)} files to {out}")
    return check(out, library)


def check(bundle: Path, library: Path | None) -> int:
    findings = []
    try:
        manifest = json.loads((bundle / MANIFEST).read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        print(f"bundle: cannot read {bundle / MANIFEST}: {exc}", file=sys.stderr)
        return 1
    rev = manifest.get("library", {}).get("rev")
    if rev != pinned_rev():
        findings.append(f"manifest names library commit {rev}; {REF_FILE.name} pins {pinned_rev()}")
    listed = {f["path"]: f for f in manifest.get("files", [])}
    on_disk = {
        p.relative_to(bundle).as_posix()
        for p in bundle.rglob("*") if p.is_file() and p.name != MANIFEST
    }
    for rel in sorted(on_disk - set(listed)):
        findings.append(f"{rel}: in the bundle, not in the manifest")
    for rel, entry in sorted(listed.items()):
        path = bundle / rel
        if not path.is_file():
            findings.append(f"{rel}: in the manifest, not in the bundle")
            continue
        if sha256(path) != entry["sha256"]:
            findings.append(f"{rel}: content does not match the manifest")
        source = None
        if entry["origin"] == "upstream-proposed":
            source = PROPOSED / rel
        elif library is not None:
            source = library / rel
        if source is not None and (not source.is_file() or sha256(source) != entry["sha256"]):
            findings.append(f"{rel}: differs from its source {source}")
        replaced = library / rel if library is not None else None
        if (entry["origin"] == "upstream-proposed" and replaced is not None
                and replaced.is_file() and not adds_lines_only(PROPOSED / rel, replaced)):
            findings.append(f"{rel}: the proposed file drops or changes a line of {replaced}")
    if library is not None and library_rev(library) != pinned_rev():
        findings.append(f"{library} is not at the pinned commit {pinned_rev()}")
    for finding in findings:
        print(f"bundle: {bundle}: {finding}", file=sys.stderr)
    if not findings:
        print(f"bundle: {bundle}: {len(listed)} files match the manifest")
    return 1 if findings else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("build")
    b.add_argument("--library", required=True, type=Path)
    b.add_argument("--out", required=True, type=Path)
    b.add_argument("--formats", required=True)
    c = sub.add_parser("check")
    c.add_argument("bundle", type=Path)
    c.add_argument("--library", type=Path)
    args = parser.parse_args()
    if args.cmd == "build":
        return build(args.library, args.out, [f for f in args.formats.split(",") if f])
    return check(args.bundle, args.library)


if __name__ == "__main__":
    sys.exit(main())
