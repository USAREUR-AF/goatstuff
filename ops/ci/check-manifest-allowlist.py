#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Every build or package manifest in the tree is on the allowlist.

A new language, framework or package manager needs a written reason and a
named owner before it lands (goatdocs docs/design/engineering-baseline.md).
The manifest is where it lands, so this guard refuses any tracked manifest that
ops/ci/manifest-allowlist.txt does not list with an owner and a reason.

Rules:

  M1  every tracked manifest (package.json, an npm/pnpm/yarn/bun lockfile,
      Cargo.toml, go.mod, pyproject.toml, requirements*.txt, Gemfile,
      build.gradle(.kts), Package.swift, composer.json, deno.json(c)) is listed
      in the allowlist; paths under vendor/, testdata/ and node_modules/ are
      not manifests of this tree and are skipped;
  M2  every allowlist line names a path, an owner and a reason, once;
  M3  every allowlist line names a manifest that is still tracked (remove the
      line when the manifest goes, so the list never rots);
  M4  every package.json dependency and devDependency is an exact version
      (1.2.3, never ^1.2.3, ~1.2.3, >=1, *, latest, a tag, a URL or a path),
      unless ops/ci/manifest-version-exceptions.txt lists it;
  M5  every exception line still names a non-exact dependency (pin it, then
      remove the line: the exceptions only shrink).

Allowlist format, one manifest per line:
  <repo-relative path> | <owner> | <one-line reason>
Exceptions format, one dependency per line:
  <repo-relative package.json path> | <dependency name>

Usage:
  ops/ci/check-manifest-allowlist.py [ROOT]     # exit 1 on any violation
  ops/ci/check-manifest-allowlist.py --list [ROOT]
  ops/ci/check-manifest-allowlist.py --self-test
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ALLOWLIST = "ops/ci/manifest-allowlist.txt"
EXCEPTIONS = "ops/ci/manifest-version-exceptions.txt"

MANIFEST_NAME = re.compile(
    r"^(?:package\.json|package-lock\.json|pnpm-lock\.yaml|yarn\.lock"
    r"|bun\.lockb?|Cargo\.toml|go\.mod|pyproject\.toml|requirements[^/]*\.txt"
    r"|Gemfile|build\.gradle(?:\.kts)?|Package\.swift|composer\.json"
    r"|deno\.jsonc?)$"
)
SKIP_PARTS = {"vendor", "testdata", "node_modules"}
EXACT = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$")
DEP_KEYS = ("dependencies", "devDependencies")


def tracked_files(root: Path) -> list[str]:
    out = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z"],
        check=True,
        capture_output=True,
    ).stdout.decode()
    return [p for p in out.split("\0") if p]


def manifests(files: list[str]) -> list[str]:
    found = []
    for path in files:
        parts = path.split("/")
        if SKIP_PARTS.intersection(parts[:-1]):
            continue
        if MANIFEST_NAME.match(parts[-1]):
            found.append(path)
    return sorted(found)


def read_lines(path: Path) -> list[tuple[int, list[str]]]:
    rows = []
    if not path.exists():
        return rows
    for n, raw in enumerate(path.read_text().splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        rows.append((n, [f.strip() for f in line.split("|")]))
    return rows


def non_exact_deps(root: Path, pkg: str) -> tuple[list[tuple[str, str]], str | None]:
    try:
        doc = json.loads((root / pkg).read_text())
    except (OSError, ValueError) as exc:
        return [], f"{pkg}: cannot parse ({exc})"
    loose = []
    for key in DEP_KEYS:
        deps = doc.get(key) or {}
        if not isinstance(deps, dict):
            continue
        for name, version in sorted(deps.items()):
            if not isinstance(version, str) or not EXACT.match(version):
                loose.append((name, str(version)))
    return loose, None


def check(root: Path) -> list[str]:
    problems: list[str] = []
    found = manifests(tracked_files(root))
    found_set = set(found)

    allowed: dict[str, int] = {}
    for n, fields in read_lines(root / ALLOWLIST):
        where = f"{ALLOWLIST}:{n}"
        if len(fields) != 3 or not all(fields):
            problems.append(f"{where}: needs `<path> | <owner> | <reason>` (M2)")
            continue
        path = fields[0]
        if path in allowed:
            problems.append(f"{where}: {path} is listed twice (M2)")
            continue
        allowed[path] = n
        if path not in found_set:
            problems.append(
                f"{where}: {path} is not a tracked manifest; remove the line (M3)"
            )

    for path in found:
        if path not in allowed:
            problems.append(
                f"{path}: manifest not on the allowlist. A new language, framework "
                f"or package manager needs a written reason and a named owner: add "
                f"`{path} | <owner> | <reason>` to {ALLOWLIST} and say why in the "
                f"pull request (M1)"
            )

    excepted: dict[tuple[str, str], int] = {}
    for n, fields in read_lines(root / EXCEPTIONS):
        where = f"{EXCEPTIONS}:{n}"
        if len(fields) != 2 or not all(fields):
            problems.append(f"{where}: needs `<package.json path> | <dependency>`")
            continue
        key = (fields[0], fields[1])
        if key in excepted:
            problems.append(f"{where}: {fields[0]} {fields[1]} is listed twice")
            continue
        excepted[key] = n

    loose_seen: set[tuple[str, str]] = set()
    for pkg in found:
        if not pkg.endswith("package.json"):
            continue
        loose, err = non_exact_deps(root, pkg)
        if err:
            problems.append(err)
            continue
        for name, version in loose:
            loose_seen.add((pkg, name))
            if (pkg, name) not in excepted:
                problems.append(
                    f"{pkg}: {name} is `{version}`, not an exact version; pin it "
                    f"(M4)"
                )

    for (pkg, name), n in sorted(excepted.items(), key=lambda kv: kv[1]):
        if (pkg, name) not in loose_seen:
            problems.append(
                f"{EXCEPTIONS}:{n}: {pkg} {name} is exact or gone; remove the "
                f"line (M5)"
            )
    return problems


def list_tree(root: Path) -> None:
    for path in manifests(tracked_files(root)):
        print(path)
        if path.endswith("package.json"):
            loose, _ = non_exact_deps(root, path)
            for name, version in loose:
                print(f"  non-exact: {name} {version}")


def self_test() -> int:
    ok = True

    def expect(label: str, problems: list[str], needle: str | None) -> None:
        nonlocal ok
        hit = any(needle in p for p in problems) if needle else not problems
        if not hit:
            ok = False
            print(f"  self-test FAIL: {label}: {problems}")

    with tempfile.TemporaryDirectory(prefix="manifest-allowlist-") as tmp:
        root = Path(tmp)
        subprocess.run(["git", "init", "-q", str(root)], check=True)

        def write(rel: str, text: str) -> None:
            p = root / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(text)
            subprocess.run(["git", "-C", str(root), "add", rel], check=True)

        write("svc/go.mod", "module example.com/svc\n")
        write("svc/vendor/x/go.mod", "module example.com/x\n")
        write("web/node_modules/y/package.json", "{}\n")
        write("web/package.json", '{"devDependencies": {"@playwright/test": "1.60.0"}}\n')
        write(ALLOWLIST, "svc/go.mod | team | Go service\nweb/package.json | team | tests\n")
        write(EXCEPTIONS, "# none\n")
        expect("clean tree passes; vendor and node_modules skipped", check(root), None)

        write("app/Cargo.toml", "[package]\nname = 'app'\n")
        expect("unlisted manifest fails", check(root), "app/Cargo.toml: manifest not on")

        write(ALLOWLIST, "svc/go.mod | team | Go service\nweb/package.json | team | tests\n"
              "app/Cargo.toml | team | Rust app\ngone/go.mod | team | old\n")
        expect("stale allowlist line fails", check(root), "gone/go.mod is not a tracked")

        write(ALLOWLIST, "svc/go.mod | team | Go service\nweb/package.json | team | tests\n"
              "app/Cargo.toml | team\n")
        expect("line without a reason fails", check(root), "needs `<path> | <owner>")

        write(ALLOWLIST, "svc/go.mod | team | Go service\nweb/package.json | team | tests\n"
              "app/Cargo.toml | team | Rust app\n")
        expect("listed manifest passes", check(root), None)

        write("web/package.json", '{"devDependencies": {"@playwright/test": "^1.60.0"}}\n')
        expect("caret range fails", check(root), "@playwright/test is `^1.60.0`")
        write(EXCEPTIONS, "web/package.json | @playwright/test\n")
        expect("listed exception passes", check(root), None)

        write("web/package.json", '{"devDependencies": {"@playwright/test": "1.60.0"}}\n')
        expect("stale exception fails", check(root), "is exact or gone; remove")

        write(EXCEPTIONS, "")
        for bad in ("~1.2.3", ">=1.0.0", "*", "latest", "1.x", "github:a/b", "file:../x"):
            write("web/package.json", json.dumps({"dependencies": {"d": bad}}))
            expect(f"`{bad}` fails", check(root), "d is `")
        for good in ("1.2.3", "1.0.0-rc.1"):
            write("web/package.json", json.dumps({"dependencies": {"d": good}}))
            expect(f"`{good}` passes", check(root), None)

    print("self-test:", "PASS" if ok else "FAIL")
    return 0 if ok else 1


def main(argv: list[str]) -> int:
    if argv[:1] == ["--self-test"]:
        return self_test()
    if argv[:1] == ["--list"]:
        list_tree(Path(argv[1] if len(argv) > 1 else ".").resolve())
        return 0
    root = Path(argv[0] if argv else ".").resolve()
    problems = check(root)
    for p in problems:
        print(f"FAIL {p}")
    if problems:
        print(f"{len(problems)} problem(s); rules in {Path(__file__).name}")
        return 1
    print("manifest allowlist: every manifest listed, every dependency exact or excepted")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
