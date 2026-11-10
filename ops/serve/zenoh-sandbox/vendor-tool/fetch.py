#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Download the packages pinned in packages.lock.json and unpack them.

Each tarball is fetched from the URL the lock file gives and accepted only
when its SHA-512 equals the lock file's integrity value. Nothing is resolved
at run time: a version or a hash changes only by editing the lock file.

Output:
  <out>/node_modules/<package name>/...   the unpacked packages
  <out>/licenses/<slug>-LICENSE.txt       each package's license file

Python standard library only.

Usage: fetch.py <packages.lock.json> <out dir>
"""

from __future__ import annotations

import base64
import hashlib
import io
import json
import sys
import tarfile
import time
import urllib.request
from pathlib import Path, PurePosixPath

MAX_TARBALL = 32 * 1024 * 1024
ATTEMPTS = 3


def slug(name: str) -> str:
    return name.lstrip("@").replace("/", "-")


def download(url: str) -> bytes:
    if not url.startswith("https://"):
        raise SystemExit(f"fetch: refusing a non-https URL: {url}")
    req = urllib.request.Request(url, headers={"User-Agent": "zenoh-sandbox-vendor-tool"})
    for attempt in range(1, ATTEMPTS + 1):
        try:
            with urllib.request.urlopen(req, timeout=60) as resp:
                data = resp.read(MAX_TARBALL + 1)
            break
        except OSError as err:  # a dropped connection; the hash check below guards the content
            if attempt == ATTEMPTS:
                raise SystemExit(f"fetch: {url}: {err}")
            time.sleep(2 * attempt)
    if len(data) > MAX_TARBALL:
        raise SystemExit(f"fetch: {url} is larger than {MAX_TARBALL} bytes")
    return data


def verify(name: str, data: bytes, integrity: str) -> None:
    alg, _, want = integrity.partition("-")
    if alg != "sha512":
        raise SystemExit(f"fetch: {name}: integrity must be sha512, got {alg!r}")
    got = base64.b64encode(hashlib.sha512(data).digest()).decode()
    if got != want:
        raise SystemExit(
            f"fetch: {name}: integrity mismatch\n  lock file: sha512-{want}\n  download:  sha512-{got}"
        )


def unpack(name: str, data: bytes, dest: Path) -> None:
    """Write the regular files of an npm tarball under dest.

    npm tarballs hold everything under one top-level directory (usually
    "package/"); that directory is dropped. Links, devices and any path that
    would leave dest are refused.
    """
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tf:
        for member in tf.getmembers():
            if member.isdir():
                continue
            if not member.isfile():
                raise SystemExit(f"fetch: {name}: refusing non-regular entry {member.name!r}")
            parts = PurePosixPath(member.name).parts[1:]
            if not parts or any(p in ("", ".", "..") for p in parts) or member.name.startswith("/"):
                raise SystemExit(f"fetch: {name}: refusing path {member.name!r}")
            target = dest.joinpath(*parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            src = tf.extractfile(member)
            assert src is not None
            target.write_bytes(src.read())


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    lock = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    out = Path(sys.argv[2])
    licenses = out / "licenses"
    licenses.mkdir(parents=True, exist_ok=True)
    for pkg in lock["packages"]:
        name, version = pkg["name"], pkg["version"]
        data = download(pkg["tarball"])
        verify(name, data, pkg["integrity"])
        dest = out / "node_modules" / name
        unpack(name, data, dest)
        manifest = json.loads((dest / "package.json").read_text(encoding="utf-8"))
        if manifest.get("name") != name or manifest.get("version") != version:
            raise SystemExit(
                f"fetch: {name}@{version}: tarball holds "
                f"{manifest.get('name')}@{manifest.get('version')}"
            )
        if pkg.get("license_file"):
            text = (dest / pkg["license_file"]).read_bytes()
            (licenses / f"{slug(name)}-LICENSE.txt").write_bytes(text)
        print(f"fetch: {name}@{version} verified ({pkg['integrity'][:20]}...)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
