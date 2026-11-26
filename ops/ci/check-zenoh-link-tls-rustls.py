#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Refuse a Cargo.lock that pairs the published zenoh-link-tls with rustls >= 0.23.46.

The published zenoh-link-tls 1.x drives one TLS stream from its read task and
its write task at the same time. rustls 0.23.46 and later check connection
state on every write, and with that sharing TLS links fail under load. A
workspace with rustls >= 0.23.46 must take zenoh-link-tls from the patched
copy (vendor/zenoh-link-tls, wired in by [patch.crates-io]; a path dependency
has no `source` in its lock entry). Tracked as finding F-614.

Usage: check-zenoh-link-tls-rustls.py [repo root]   (default: current directory)
Exit 1 and list the offending lockfiles when any is found.
"""
import re
import subprocess
import sys
from pathlib import Path

LIMIT = (0, 23, 46)


def packages(text):
    for block in text.split("[[package]]")[1:]:
        name = re.search(r'^name = "([^"]+)"', block, re.M)
        version = re.search(r'^version = "([^"]+)"', block, re.M)
        if name and version:
            yield name.group(1), version.group(1), "\nsource = " in block


def vtuple(v):
    return tuple(int(x) for x in re.findall(r"\d+", v)[:3])


def main():
    root = Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
    files = subprocess.run(
        ["git", "-C", str(root), "ls-files", "*Cargo.lock"],
        capture_output=True, text=True, check=True,
    ).stdout.split()
    bad = []
    for rel in files:
        pkgs = list(packages((root / rel).read_text()))
        published_zlt = [v for n, v, from_registry in pkgs if n == "zenoh-link-tls" and from_registry]
        new_rustls = [v for n, v, _ in pkgs if n == "rustls" and vtuple(v) >= LIMIT]
        if published_zlt and new_rustls:
            bad.append(f"{rel}: zenoh-link-tls {','.join(published_zlt)} (published) with rustls {','.join(new_rustls)}")
    if bad:
        print("Lockfiles pair the published zenoh-link-tls with rustls >= 0.23.46 (finding F-614):")
        for line in bad:
            print(f"  {line}")
        print(
            "Fix: add to the workspace's Cargo.toml\n"
            "  [patch.crates-io]\n"
            '  zenoh-link-tls = { path = "<relative>/vendor/zenoh-link-tls" }\n'
            "(a service whose image builds from its own directory keeps an identical\n"
            "copy at <service>/vendor/zenoh-link-tls), then `cargo update -p zenoh-link-tls`."
        )
        return 1
    print(f"ok: {len(files)} lockfiles checked")
    return 0


if __name__ == "__main__":
    sys.exit(main())
