#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Apply reviewed cleanup edits to verified, unpacked package inputs."""

import hashlib
import json
from pathlib import Path
import sys


def apply_patches(tree: Path, records: list) -> None:
    """Validate every input and output before changing any package file."""
    modules = (tree / "node_modules").resolve()
    pending = {}
    for record in records:
        target = (modules / record["path"]).resolve()
        if not target.is_relative_to(modules) or target in pending:
            raise ValueError("invalid or duplicate patch path")
        original = target.read_bytes()
        if hashlib.sha256(original).hexdigest() != record["input_sha256"]:
            raise ValueError(f"unexpected package input: {record['path']}")
        text = original.decode("utf-8")
        for edit in record["edits"]:
            if text.count(edit["before"]) != edit["count"]:
                raise ValueError(f"unexpected edit count: {record['path']}")
            text = text.replace(edit["before"], edit["after"])
        output = text.encode("utf-8")
        if hashlib.sha256(output).hexdigest() != record["output_sha256"]:
            raise ValueError(f"unexpected patch output: {record['path']}")
        pending[target] = output
    for target, output in pending.items():
        target.write_bytes(output)


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: patch.py <unpacked tree>")
    records = json.loads(Path(__file__).with_name("patches.json").read_text())
    apply_patches(Path(sys.argv[1]), records)


if __name__ == "__main__":
    main()
