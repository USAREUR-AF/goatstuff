# SPDX-License-Identifier: Apache-2.0
"""The goat-feeds role runs a verified release file, or a trial's file, and nothing else.

Runs the role's tasks/binary.yml with ansible-playbook on localhost, against a release
directory laid out as goatcore's release-file role leaves it:
<home>/<sha256>/goat-feeds-<version>-linux-<arch>, with `current` a link to the file.
The role's handler (a systemd restart) is replaced by a no-op in a copy of the role, and
goatcore's rollback-ledger role, its dependency, by a stand-in.
"""
import getpass
import hashlib
import json
import os
import shutil
import subprocess
import textwrap

import pytest

ROLE = os.path.normpath(os.path.join(os.path.dirname(__file__), ".."))
NAME = "goat-feeds-0.5.0-linux-amd64"

pytestmark = pytest.mark.skipif(shutil.which("ansible-playbook") is None, reason="needs ansible-playbook")


def _release(tmp_path, content=b"goat-feeds binary\n", name=NAME, sha=None):
    home = tmp_path / "releases" / "goat-feeds"
    real = hashlib.sha256(content).hexdigest()
    d = home / (sha or real)
    d.mkdir(parents=True)
    f = d / name
    f.write_bytes(content)
    f.chmod(0o755)
    for p in (d, home, home.parent):
        p.chmod(0o755)
    (home / "current").symlink_to(f)
    return home, real, f


def _run(tmp_path, **vars_):
    roles = tmp_path / "roles"
    shutil.copytree(ROLE, roles / "goat-feeds", ignore=shutil.ignore_patterns("tests", "__pycache__"))
    (roles / "goat-feeds" / "handlers" / "main.yml").write_text(
        "---\n- name: restart goat-feeds\n  ansible.builtin.debug:\n    msg: restart\n"
    )
    # goatcore's ledger role, which a tree that layers goatstuff has; here a no-op.
    (roles / "rollback-ledger" / "tasks").mkdir(parents=True)
    (roles / "rollback-ledger" / "tasks" / "main.yml").write_text(
        "---\n- name: rollback-ledger stand-in\n  ansible.builtin.debug:\n    msg: ledger\n"
    )
    opt = tmp_path / "opt"
    opt.mkdir(exist_ok=True)
    base = {
        "gf_arch": "amd64",
        "goat_feeds_dir": str(opt),
        "goat_feeds_version": "0.5.0",
        "goat_feeds_binary_src": "",
        "goat_feeds_release_home": "",
        "goat_feeds_release_owner": getpass.getuser(),
    }
    base.update(vars_)
    play = tmp_path / "play.yml"
    play.write_text(textwrap.dedent("""\
        - hosts: localhost
          connection: local
          gather_facts: false
          tasks:
            - ansible.builtin.include_role:
                name: goat-feeds
                tasks_from: binary
            - ansible.builtin.copy:
                content: "{{ {'binary': gf_binary, 'version': gf_version} | to_json }}"
                dest: "{{ goat_feeds_dir }}/result.json"
        """))
    extra = tmp_path / "vars.json"
    extra.write_text(json.dumps(base))
    env = dict(os.environ, ANSIBLE_ROLES_PATH=str(roles), ANSIBLE_NOCOLOR="1",
               ANSIBLE_LOCALHOST_WARNING="false", ANSIBLE_INVENTORY_UNPARSED_WARNING="false")
    r = subprocess.run(["ansible-playbook", "-i", "localhost,", str(play), "-e", "@" + str(extra)],
                       capture_output=True, text=True, env=env)
    result = opt / "result.json"
    return r, (json.loads(result.read_text()) if r.returncode == 0 else None), opt


def _release_vars(home, sha, name=NAME, **more):
    return dict(goat_feeds_release_home=str(home), _rf_sha256=sha, _rf_name=name, **more)


def test_runs_the_verified_release_file(tmp_path):
    home, sha, f = _release(tmp_path)
    r, out, opt = _run(tmp_path, **_release_vars(home, sha))
    assert r.returncode == 0, r.stdout + r.stderr
    assert out == {"binary": str(f), "version": "0.5.0"}
    assert os.readlink(opt / "goat-feeds") == str(f)


def test_refuses_both_sources(tmp_path):
    home, sha, _ = _release(tmp_path)
    r, _, _ = _run(tmp_path, goat_feeds_binary_src="/nowhere/goat-feeds-{arch}", **_release_vars(home, sha))
    assert r.returncode != 0 and "not both and not" in r.stdout


def test_refuses_no_source(tmp_path):
    r, _, _ = _run(tmp_path)
    assert r.returncode != 0 and "not both and not" in r.stdout


def test_refuses_a_file_not_its_directorys_sha256(tmp_path):
    home, _, _ = _release(tmp_path, sha="0" * 64)
    r, _, opt = _run(tmp_path, **_release_vars(home, "0" * 64))
    assert r.returncode != 0 and "is not a verified goat-feeds release file" in r.stdout
    assert not os.path.lexists(opt / "goat-feeds")


def test_refuses_a_file_others_can_change(tmp_path):
    home, sha, f = _release(tmp_path)
    f.chmod(0o775)
    r, _, opt = _run(tmp_path, **_release_vars(home, sha))
    assert r.returncode != 0 and "is not a verified goat-feeds release file" in r.stdout
    assert not os.path.lexists(opt / "goat-feeds")


def test_refuses_a_directory_others_can_change(tmp_path):
    home, sha, _ = _release(tmp_path)
    home.chmod(0o777)
    r, _, _ = _run(tmp_path, **_release_vars(home, sha))
    assert r.returncode != 0 and "is not a verified goat-feeds release file" in r.stdout


def test_refuses_another_architectures_file(tmp_path):
    name = "goat-feeds-0.5.0-linux-arm64"
    home, sha, _ = _release(tmp_path, name=name)
    r, _, _ = _run(tmp_path, **_release_vars(home, sha, name=name))
    assert r.returncode != 0 and "is not a verified goat-feeds release file" in r.stdout


def test_refuses_a_current_that_is_not_a_link(tmp_path):
    home, sha, f = _release(tmp_path)
    (home / "current").unlink()
    shutil.copy(f, home / "current")
    r, _, _ = _run(tmp_path, **_release_vars(home, sha))
    assert r.returncode != 0 and "is not a verified goat-feeds release file" in r.stdout


def test_install_refuses_a_current_that_is_not_the_file_just_verified(tmp_path):
    home, sha, _ = _release(tmp_path)
    r, _, _ = _run(tmp_path, **_release_vars(home, "1" * 64))
    assert r.returncode != 0 and "is not a verified goat-feeds release file" in r.stdout


def test_rollback_runs_the_file_current_names(tmp_path):
    home, sha, f = _release(tmp_path)
    r, out, _ = _run(tmp_path, **_release_vars(home, "1" * 64, release_file_rollback=True))
    assert r.returncode == 0, r.stdout + r.stderr
    assert out["binary"] == str(f)


def test_refuses_a_current_that_resolves_outside_the_home(tmp_path):
    content = b"goat-feeds binary\n"
    sha = hashlib.sha256(content).hexdigest()
    home, _, _ = _release(tmp_path)
    elsewhere = tmp_path / "elsewhere" / sha
    elsewhere.mkdir(parents=True)
    f = elsewhere / NAME
    f.write_bytes(content)
    f.chmod(0o755)
    (home / "current").unlink()
    (home / "current").symlink_to(f)
    r, _, opt = _run(tmp_path, **_release_vars(home, sha))
    assert r.returncode != 0 and "is not a verified goat-feeds release file" in r.stdout
    assert not os.path.lexists(opt / "goat-feeds")


def test_refuses_a_sha256_directory_others_can_change(tmp_path):
    home, sha, f = _release(tmp_path)
    f.parent.chmod(0o775)
    r, _, _ = _run(tmp_path, **_release_vars(home, sha))
    assert r.returncode != 0 and "is not a verified goat-feeds release file" in r.stdout


def test_refuses_a_release_root_others_can_change(tmp_path):
    home, sha, _ = _release(tmp_path)
    home.parent.chmod(0o777)
    r, _, _ = _run(tmp_path, **_release_vars(home, sha))
    assert r.returncode != 0 and "is not a verified goat-feeds release file" in r.stdout


def test_refuses_a_file_another_user_owns(tmp_path):
    if getpass.getuser() == "root":
        pytest.skip("the files are root's")
    home, sha, _ = _release(tmp_path)
    r, _, _ = _run(tmp_path, goat_feeds_release_owner="root", **_release_vars(home, sha))
    assert r.returncode != 0 and "is not a verified goat-feeds release file" in r.stdout


def test_refuses_a_name_with_a_trailing_newline(tmp_path):
    name = NAME + "\n"
    home, sha, _ = _release(tmp_path, name=name)
    r, _, _ = _run(tmp_path, **_release_vars(home, sha, name=name))
    assert r.returncode != 0 and "is not a verified goat-feeds release file" in r.stdout
