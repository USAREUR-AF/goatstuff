#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Data standard records in the portal, declared in one file per goatnet.

  check  offline: every fabric key in the named files has a declared record
  plan   what the portal lacks or holds differently; changes nothing
  apply  create missing topics, correct format and description, upload the
         declared schema version where it is missing. Never deletes.

The portal registers a topic under its owner's anchor, so `volatile/` is not
part of a declared key. A `*` segment covers every topic of that shape, such as
each feed an admin approves later.
"""

from __future__ import annotations

import argparse
import json
import re
import ssl
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path

import yaml

HERE = Path(__file__).resolve().parent
KEY = re.compile(r"(?<![0-9A-Za-z/_-])(?:volatile/)?([0-9a-f]{32}(?:/[A-Za-z0-9_.*-]+)+)")
VERSION = re.compile(r"v[0-9]+")
KINDS = {".proto": "protobuf-source", ".fds": "protobuf-fds", ".xsd": "xsd"}


def covers(pattern: str, key: str) -> bool:
    p, k = pattern.split("/"), key.split("/")
    return len(p) == len(k) and all(a in ("*", b) for a, b in zip(p, k))


def load(path: Path) -> list[dict]:
    doc = yaml.safe_load(path.read_text())
    out, seen = [], set()
    for row in doc["records"]:
        std = doc["standards"][row["standard"]]
        key = row["key"]
        if not KEY.fullmatch(key) or key.startswith("volatile/") or "**" in key or len(key.split("/")) < 3:
            raise ValueError(f"{key}: <32-hex anchor>/<system>/..., no volatile/ and no **")
        if key in seen:
            raise ValueError(f"{key}: declared twice")
        seen.add(key)
        schema = std.get("schema")
        if schema:
            if not (HERE / "schemas" / schema).is_file() or Path(schema).suffix not in KINDS:
                raise ValueError(f"{schema}: no such schema file")
            if not VERSION.fullmatch(str(std.get("version", ""))):
                raise ValueError(f"{row['standard']}: version must be v<N>")
        if not row.get("description"):
            raise ValueError(f"{key}: no description")
        out.append({"key": key, "org": key.split("/")[0], "format": std["format"], "schema": schema,
                    "version": std.get("version"), "description": row["description"]})
    return out


def keys_in(text: str, prefixes: list[str]) -> set[str]:
    """Fabric keys in a configuration file. A bare prefix that keys are built
    under is named in the file's `prefixes` and is not itself a topic."""
    found = {m.group(1).rstrip(".") for m in KEY.finditer(text)}
    return {k for k in found if "**" not in k and k not in prefixes}


def undeclared(path: Path, files: list[Path]) -> list[str]:
    doc = yaml.safe_load(path.read_text())
    recs = load(path)
    skip = list(doc.get("prefixes") or [])
    out = []
    for f in files:
        for key in sorted(keys_in(f.read_text(), skip)):
            if not any(covers(r["key"], key) for r in recs):
                out.append(f"{f}: {key}")
    return out


class Portal:
    def __init__(self, url: str, token: str, ca: str | None) -> None:
        self.url, self.auth = url.rstrip("/"), {"Authorization": f"Bearer {token}"}
        self.ctx = ssl.create_default_context(cafile=ca)

    def call(self, method: str, path: str, body: bytes | None = None, ctype: str = "application/json"):
        req = urllib.request.Request(self.url + path, data=body, method=method,
                                     headers={**self.auth, **({"Content-Type": ctype} if body else {})})
        try:
            with urllib.request.urlopen(req, timeout=20, context=self.ctx) as r:
                raw = r.read()
        except urllib.error.HTTPError as exc:
            raise SystemExit(f"{method} {path}: {exc.code} {exc.read().decode()[:300]}") from None
        return json.loads(raw) if raw else {}

    def topics(self, org: str) -> dict[str, dict]:
        got = self.call("GET", f"/api/vendors/{org}/topics")
        return {t["key_pattern"]: t for t in (got if isinstance(got, list) else got.get("topics") or [])}

    def versions(self, org: str, tid: int) -> set[str]:
        got = self.call("GET", f"/api/vendors/{org}/topics/{tid}/schemas")
        return {s["version"] for s in (got if isinstance(got, list) else got.get("schemas") or [])}


def multipart(fields: dict[str, str], name: str, data: bytes) -> tuple[bytes, str]:
    b = uuid.uuid4().hex
    head = "".join(f'--{b}\r\nContent-Disposition: form-data; name="{k}"\r\n\r\n{v}\r\n' for k, v in fields.items())
    head += f'--{b}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: application/octet-stream\r\n\r\n'
    return head.encode() + data + f"\r\n--{b}--\r\n".encode(), f"multipart/form-data; boundary={b}"


def reconcile(portal, recs: list[dict], apply: bool) -> int:
    changes = 0
    for org in sorted({r["org"] for r in recs}):
        have = portal.topics(org)
        rows = [r for r in recs if r["org"] == org]
        for r in rows:
            t = have.get(r["key"])
            steps = []
            if t is None:
                steps.append("create topic")
            elif t.get("format") != r["format"] or t.get("description") != r["description"]:
                steps.append("update format or description")
            if r["schema"] and (t is None or r["version"] not in portal.versions(org, t["id"])):
                steps.append(f"upload {r['schema']} {r['version']}")
            changes += len(steps)
            print(f"{'ok    ' if not steps else 'apply ' if apply else 'plan  '} {r['key']}  {', '.join(steps)}")
            if not (apply and steps):
                continue
            body = {"key_pattern": r["key"], "format": r["format"], "description": r["description"]}
            if t is None:
                t = portal.call("POST", f"/api/vendors/{org}/topics", json.dumps({**body, "publish_to_storage": False}).encode())
            elif any(s.startswith("update") for s in steps):
                # Carry the fields this tool does not own: the portal replaces the row.
                keep = {k: t.get(k) for k in ("schema_ref", "publish_to_storage") if t.get(k) is not None}
                portal.call("PATCH", f"/api/vendors/{org}/topics/{t['id']}", json.dumps(
                    {"format": r["format"], "description": r["description"], **keep}).encode())
            if any(s.startswith("upload") for s in steps):
                f = HERE / "schemas" / r["schema"]
                data, ctype = multipart({"version": r["version"], "compatibility": "none", "format": KINDS[f.suffix]},
                                        f.name, f.read_bytes())
                portal.call("POST", f"/api/vendors/{org}/topics/{t['id']}/schemas", data, ctype)
        for key in sorted(set(have) - {r["key"] for r in rows}):
            print(f"other  {key}  (in the portal, not declared here; left alone)")
    return changes


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("verb", choices=("check", "plan", "apply"))
    ap.add_argument("records", type=Path)
    ap.add_argument("files", nargs="*", type=Path, help="check: files whose fabric keys must be declared")
    ap.add_argument("--profile", type=Path, help="plan/apply: a goat context directory (profile.toml, portal.token)")
    ap.add_argument("--ca", help="CA bundle for the portal's certificate")
    a = ap.parse_args(argv)
    if a.verb == "check":
        load(a.records)
        bad = undeclared(a.records, a.files)
        for b in bad:
            print(f"no portal record declared for {b}", file=sys.stderr)
        if bad:
            print(f"add a row to {a.records}, then run plan and apply", file=sys.stderr)
        return 1 if bad else 0
    if not a.profile:
        ap.error("--profile is required")
    url = re.search(r'portal_endpoint\s*=\s*"([^"]+)"', (a.profile / "profile.toml").read_text()).group(1)
    portal = Portal(url, (a.profile / "portal.token").read_text().strip(), a.ca)
    n = reconcile(portal, load(a.records), a.verb == "apply")
    print(f"{n} change(s) {'made' if a.verb == 'apply' else 'needed'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
