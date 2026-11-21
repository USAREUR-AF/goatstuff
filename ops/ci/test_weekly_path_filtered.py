"""The weekly path-filtered run (ops/ci/weekly-path-filtered.py), with a stub gh.

Run: python3 -m pytest ops/ci/test_weekly_path_filtered.py
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
TOOL = os.path.join(REPO, "ops/ci/weekly-path-filtered.py")

STUB = r'''#!/usr/bin/env python3
import json, os, sys
a = sys.argv[1:]
st = json.load(open(os.environ["GH_STATE"]))
open(os.environ["GH_LOG"], "a").write(" ".join(a) + "\n")
if a[:2] == ["workflow", "run"]:
    st["runs"].append({"wf": a[2], "databaseId": 100 + len(st["runs"]), "createdAt": "2999-01-01T00:00:00Z"})
    json.dump(st, open(os.environ["GH_STATE"], "w")); sys.exit(0)
if a[:2] == ["run", "list"]:
    wf = a[a.index("--workflow") + 1]
    print(json.dumps([{k: r[k] for k in ("databaseId", "createdAt")} for r in st["runs"] if r["wf"] == wf])); sys.exit(0)
if a[:2] == ["run", "view"]:
    rid = int(a[2]); c = st["conclusions"].get(str(rid), "success")
    print(json.dumps({"status": "completed", "conclusion": c, "url": f"run/{rid}"})); sys.exit(0)
if a[:2] == ["issue", "list"]:
    print(json.dumps(st.get("issues", []))); sys.exit(0)
if a[0] == "issue":
    sys.exit(0)
sys.exit(3)
'''


class Weekly(unittest.TestCase):
    def setUp(self):
        self.t = tempfile.mkdtemp(prefix="weekly-")
        self.wf = os.path.join(self.t, "workflows")
        os.makedirs(self.wf)
        self.write("a.yml", """\
            on:
              pull_request:
                paths: ['ops/a/**']
              workflow_dispatch:
            jobs: {}
            """)
        self.write("b.yml", """\
            on:
              push:
                paths-ignore: ['docs/**']
              workflow_dispatch: {}
            jobs: {}
            """)
        self.write("c.yml", "on:\n  pull_request:\n    paths: ['c/**']\njobs: {}\n")
        self.write("d.yml", "on: [pull_request, workflow_dispatch]\njobs: {}\n")
        self.write("_reusable.yml", "on:\n  workflow_call:\n  pull_request:\n    paths: ['x']\njobs: {}\n")
        self.bin = os.path.join(self.t, "bin")
        os.makedirs(self.bin)
        with open(os.path.join(self.bin, "gh"), "w") as fh:
            fh.write(STUB)
        os.chmod(os.path.join(self.bin, "gh"), 0o700)
        self.state = os.path.join(self.t, "gh.json")
        self.log = os.path.join(self.t, "gh.log")
        self.gh({"runs": [], "conclusions": {}})

    def tearDown(self):
        shutil.rmtree(self.t)

    def write(self, name, text):
        with open(os.path.join(self.wf, name), "w") as fh:
            fh.write(textwrap.dedent(text))

    def read_log(self):
        with open(self.log, encoding="utf-8") as fh:
            return fh.read()

    def gh(self, st):
        with open(self.state, "w") as fh:
            json.dump(st, fh)

    def run_tool(self, *args):
        env = dict(os.environ, PATH=self.bin + os.pathsep + os.environ["PATH"], GH_STATE=self.state, GH_LOG=self.log)
        r = subprocess.run([sys.executable, TOOL, *args], capture_output=True, text=True, env=env, cwd=self.t)
        return r.returncode, r.stdout + r.stderr

    def test_list_names_filtered_workflows_and_which_are_not_covered(self):
        rc, out = self.run_tool("list", "--workflows", self.wf)
        self.assertIn("dispatch  a.yml", out)
        self.assertIn("dispatch  b.yml", out)
        self.assertIn("NOT COVERED (no workflow_dispatch)  c.yml", out)
        self.assertNotIn("d.yml", out)          # no path filter: runs on every change already
        self.assertNotIn("_reusable.yml", out)  # a reusable workflow runs through its callers
        self.assertIn("2 of 3 path-filtered workflows can be run on main", out)

    def test_dispatch_then_report_files_one_issue_per_red_workflow(self):
        rc, out = self.run_tool("dispatch", "--workflows", self.wf, "--out", "runs.json", "--poll-seconds", "0")
        self.assertEqual(rc, 0, out)
        with open(os.path.join(self.t, "runs.json"), encoding="utf-8") as fh:
            runs = json.load(fh)
        self.assertEqual(set(runs), {"a.yml", "b.yml"})
        with open(self.state, encoding="utf-8") as fh:
            st = json.load(fh)
        st["conclusions"] = {str(runs["b.yml"]): "failure"}
        self.gh(st)
        rc, out = self.run_tool("report", "--runs", "runs.json", "--open-issues", "--poll-seconds", "0")
        self.assertEqual(rc, 1, out)
        self.assertIn("RED  b.yml: failure", out)
        self.assertIn("ok   a.yml: success", out)
        log = self.read_log()
        self.assertEqual(log.count("issue create"), 1)
        self.assertIn("Weekly path-filtered run: b.yml is red on main", log)

    def test_an_open_issue_is_commented_and_a_green_run_closes_its_issue(self):
        with open(os.path.join(self.t, "runs.json"), "w") as fh:
            json.dump({"a.yml": 1, "b.yml": 2}, fh)
        self.gh({"runs": [], "conclusions": {"2": "failure"}, "issues": [
            {"number": 7, "title": "Weekly path-filtered run: a.yml is red on main"},
            {"number": 8, "title": "Weekly path-filtered run: b.yml is red on main"}]})
        self.run_tool("report", "--runs", "runs.json", "--open-issues", "--poll-seconds", "0")
        log = self.read_log()
        self.assertIn("issue close 7", log)
        self.assertIn("issue comment 8", log)
        self.assertNotIn("issue create", log)

    def test_this_repository_has_its_path_filtered_workflows_dispatchable(self):
        rc, out = self.run_tool("list", "--workflows", os.path.join(REPO, ".github/workflows"))
        self.assertEqual(rc, 0, out)
        not_covered = [l.split()[-1] for l in out.splitlines() if l.startswith("NOT COVERED")]
        with open(os.path.join(REPO, ".github/weekly-path-filtered-exceptions.txt"), encoding="utf-8") as fh:
            allowed = set(fh.read().split())
        self.assertEqual(sorted(set(not_covered) - allowed), [], "add workflow_dispatch, or list the workflow with its reason")


if __name__ == "__main__":
    unittest.main()
