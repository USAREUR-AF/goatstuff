# SPDX-License-Identifier: Apache-2.0
"""zenoh-synth-vendors-slot-pool-deploy.yml applies only to a portal in the
per-uuid slot pool model.

With synth on and any other portal model, the play ends the host with one line
and succeeds, so goatcore's standup can run it on every goatnet with synth on.
With the slot-pool model it goes on to the checks it needs (here it stops at the
operator API key, which the test does not set). The play runs on localhost;
nothing is installed.
"""
import json
import os
import shutil
import subprocess

import pytest

ANSIBLE = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
PLAY = os.path.join(ANSIBLE, "zenoh-synth-vendors-slot-pool-deploy.yml")
LINE = "portal_identity_model is not per-uuid-slot-pool; a per-vendor goatnet runs its synth path by hand"

pytestmark = pytest.mark.skipif(shutil.which("ansible-playbook") is None, reason="needs ansible-playbook")


def _run(tmp_path, **vars_):
    base = {"target": "localhost", "ansible_become": False, "ansible_connection": "local",
            "synth_vendors_enabled": True, "site_id": "zagros",
            "portal_public_url": "https://portal.zagros.example.org",
            "synth_vendor_compose_dir": str(tmp_path / "synth")}
    base.update(vars_)
    extra = tmp_path / "vars.json"
    extra.write_text(json.dumps(base))
    env = {k: v for k, v in os.environ.items() if not k.endswith("_PORTAL_OPERATOR_API_KEY")}
    env.update(ANSIBLE_ROLES_PATH=os.path.join(ANSIBLE, "roles"), ANSIBLE_NOCOLOR="1",
               ANSIBLE_LOCALHOST_WARNING="false", ANSIBLE_INVENTORY_UNPARSED_WARNING="false",
               ANSIBLE_LOCAL_TEMP=str(tmp_path / "ansible-tmp"))
    r = subprocess.run(["ansible-playbook", "-i", "localhost,", PLAY, "-e", "@" + str(extra)],
                       capture_output=True, text=True, env=env, cwd=ANSIBLE, timeout=300)
    return r, r.stdout + r.stderr


@pytest.mark.parametrize("model", ["per-vendor", None])
def test_a_portal_outside_the_slot_pool_ends_the_host_with_one_line(tmp_path, model):
    extra = {} if model is None else {"portal_identity_model": model}
    r, out = _run(tmp_path, **extra)
    assert r.returncode == 0, out
    assert LINE in out, out
    assert "assert per-uuid-slot-pool mode" not in out, out
    assert "operator API key" not in out, out


def test_a_slot_pool_portal_goes_on_to_the_plays_checks(tmp_path):
    r, out = _run(tmp_path, portal_identity_model="per-uuid-slot-pool")
    assert LINE not in out, out
    assert "assert operator API key present" in out, out
    assert r.returncode != 0, "the test sets no operator API key, so the play stops there"
