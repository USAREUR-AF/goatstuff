# SPDX-License-Identifier: Apache-2.0
"""The version a release tag names is the one the binary, the role and the changelog name.

The release workflow refuses a tag otherwise; this holds the tree to it on every pull
request, so a version bump that misses one of them fails here first.
"""
import os
import re

ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "..", "..", "..", ".."))
FEEDS = os.path.join(ROOT, "ops", "serve", "goat-feeds")


def _version():
    v = open(os.path.join(FEEDS, "VERSION")).read().strip()
    assert re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", v), v
    return v


def test_the_roles_default_is_the_version():
    defaults = open(os.path.join(ROOT, "ops", "ansible", "roles", "goat-feeds", "defaults", "main.yml")).read()
    m = re.search(r'^goat_feeds_version: "([^"]*)"$', defaults, re.M)
    assert m and m.group(1) == _version()


def test_the_changelog_has_the_versions_section():
    lines = open(os.path.join(FEEDS, "CHANGELOG.md")).read().splitlines()
    assert "## " + _version() in lines


def test_the_workflow_signs_for_the_installers_signer():
    wf = open(os.path.join(ROOT, ".github", "workflows", "goat-feeds-release.yml")).read()
    assert 'tags: ["goat-feeds-v*"]' in wf
    assert "--bundle dist/SHA256SUMS.sigstore.json dist/SHA256SUMS" in wf
    assert "IDENTITY: https://github.com/${{ github.repository }}/.github/workflows/goat-feeds-release.yml@${{ github.ref }}" in wf
    assert "if: github.event_name == 'push' && github.ref_type == 'tag'" in wf


def test_the_pinned_signing_config_names_a_rekor_v1_log_only():
    import json
    cfg = json.load(open(os.path.join(FEEDS, "release", "signing-config.json")))
    assert cfg["mediaType"] == "application/vnd.dev.sigstore.signingconfig.v0.2+json"
    assert [(u["url"], u["majorApiVersion"]) for u in cfg["rekorTlogUrls"]] == [("https://rekor.sigstore.dev", 1)]


def test_every_job_checks_out_before_it_downloads_the_files():
    # actions/checkout cleans the workspace: files downloaded before it are lost.
    import yaml
    wf = yaml.safe_load(open(os.path.join(ROOT, ".github", "workflows", "goat-feeds-release.yml")))
    for name, job in wf["jobs"].items():
        uses = [s.get("uses", "").split("@")[0] for s in job["steps"]]
        if "actions/checkout" in uses and "actions/download-artifact" in uses:
            assert uses.index("actions/checkout") < uses.index("actions/download-artifact"), name
