# SPDX-License-Identifier: Apache-2.0
"""goat-feeds-deploy.yml checks a goatnet's settings before release-file puts anything in place.

Runs the play on localhost with goatcore's release-file and rollback-ledger roles replaced by
stand-ins that record that they ran; nothing is installed.
"""
import json
import os
import shutil
import subprocess

import pytest

ANSIBLE = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))

pytestmark = pytest.mark.skipif(shutil.which("ansible-playbook") is None, reason="needs ansible-playbook")


def _run(tmp_path, **vars_):
    roles = tmp_path / "roles"
    shutil.copytree(os.path.join(ANSIBLE, "roles"), roles)
    marker = tmp_path / "release-file-ran.json"
    for stub in ("release-file", "rollback-ledger"):
        shutil.rmtree(roles / stub, ignore_errors=True)
        (roles / stub / "tasks").mkdir(parents=True)
    (roles / "release-file" / "tasks" / "main.yml").write_text(
        "---\n- name: release-file stand-in\n  ansible.builtin.copy:\n"
        "    content: \"{{ {'home': goat_feeds_release_home, 'rollback': release_file_rollback} | to_json }}\"\n"
        f"    dest: {marker}\n"
        "- name: stop here\n  ansible.builtin.fail:\n    msg: stand-in stops the play\n"
    )
    (roles / "rollback-ledger" / "tasks" / "main.yml").write_text("---\n- ansible.builtin.debug:\n    msg: ledger\n")
    base = {"target": "localhost", "goat_feeds_host": "localhost", "ansible_become": False,
            "ansible_connection": "local", "release_file_root": str(tmp_path / "releases"),
            "goat_feeds_bind_ip": "100.127.0.10", "goat_feeds_oidc_issuer_url": "https://example.org/"}
    base.update(vars_)
    extra = tmp_path / "vars.json"
    extra.write_text(json.dumps(base))
    env = dict(os.environ, ANSIBLE_ROLES_PATH=str(roles), ANSIBLE_NOCOLOR="1",
               ANSIBLE_LOCALHOST_WARNING="false", ANSIBLE_INVENTORY_UNPARSED_WARNING="false")
    r = subprocess.run(["ansible-playbook", "-i", "localhost,", os.path.join(ANSIBLE, "goat-feeds-deploy.yml"),
                        "-e", "@" + str(extra)], capture_output=True, text=True, env=env)
    return r, (json.loads(marker.read_text()) if marker.exists() else None)


def test_a_release_reaches_release_file_with_its_home_as_an_install(tmp_path):
    r, ran = _run(tmp_path, release_file_component="goat-feeds")
    assert ran == {"home": str(tmp_path / "releases" / "goat-feeds"), "rollback": False}, r.stdout


def test_a_trials_file_left_beside_a_release_is_refused_before_release_file(tmp_path):
    r, ran = _run(tmp_path, release_file_component="goat-feeds", goat_feeds_binary_src="/nowhere/goat-feeds-{arch}")
    assert r.returncode != 0 and "Nothing was installed" in r.stdout
    assert ran is None


def test_missing_fence_settings_are_refused_before_release_file(tmp_path):
    r, ran = _run(tmp_path, release_file_component="goat-feeds", goat_feeds_oidc_issuer_url="")
    assert r.returncode != 0 and "Nothing was installed" in r.stdout
    assert ran is None
