# SPDX-License-Identifier: Apache-2.0
"""The package patch step must refuse drift before writing any file."""

import hashlib
from pathlib import Path
import tempfile
import unittest

from patch import apply_patches


def record(path, before, after):
    return {
        "path": path,
        "input_sha256": hashlib.sha256(before.encode()).hexdigest(),
        "output_sha256": hashlib.sha256(after.encode()).hexdigest(),
        "edits": [{"before": before, "after": after, "count": 1}],
    }


class PatchTests(unittest.TestCase):
    def test_validated_edit(self):
        with tempfile.TemporaryDirectory() as directory:
            tree = Path(directory)
            modules = tree / "node_modules"
            modules.mkdir()
            target = modules / "input.js"
            target.write_text("old")
            apply_patches(tree, [record("input.js", "old", "new")])
            self.assertEqual(target.read_text(), "new")

    def test_drift_never_partially_applies(self):
        for failure in ("input", "output", "count", "path", "duplicate"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                tree = Path(directory)
                modules = tree / "node_modules"
                modules.mkdir()
                first = modules / "first.js"
                second = modules / "second.js"
                first.write_text("old")
                second.write_text("old")
                good = record("first.js", "old", "new")
                bad = record("second.js", "old", "new")
                if failure == "input":
                    second.write_text("changed")
                elif failure == "output":
                    bad["output_sha256"] = "0" * 64
                elif failure == "count":
                    bad["edits"][0]["count"] = 2
                elif failure == "path":
                    bad["path"] = "../outside.js"
                else:
                    bad = good
                with self.assertRaises(ValueError):
                    apply_patches(tree, [good, bad])
                self.assertEqual(first.read_text(), "old")
                self.assertEqual(second.read_text(), "changed" if failure == "input" else "old")


if __name__ == "__main__":
    unittest.main()
