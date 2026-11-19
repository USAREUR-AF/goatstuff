#!/usr/bin/env python3
"""Render production replay templates locally; never executes deployment tasks."""

import argparse
import pathlib
import subprocess
import json
import os
import shutil
import yaml

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--evidence", type=pathlib.Path, required=True)
parser.add_argument(
    "--goatcore", type=pathlib.Path, default=os.environ.get("GOATCORE_ROOT"),
    help="goatcore checkout holding the minio, audit-subscriber and goat-zenoh-portal roles "
         "and ops/compose (default: $GOATCORE_ROOT)")
args = parser.parse_args()
# This file lives in goatstuff; the roles it renders are split across the two
# repositories (openobserve here, the storage and portal roles in goatcore).
root = pathlib.Path(__file__).resolve().parents[4]
if args.goatcore is None:
    parser.error("--goatcore (or GOATCORE_ROOT) must name a goatcore checkout")
core = args.goatcore.resolve()
if not (core / "ops/ansible/roles/minio").is_dir():
    parser.error(f"{core} has no ops/ansible/roles/minio; it is not a goatcore checkout")
out = args.evidence.resolve()
out.mkdir(parents=True, exist_ok=False)


class Roles:
    """`roles / "<role>/..."`: the role from goatcore when it lives there, else from goatstuff."""

    def __truediv__(self, rel):
        name = str(rel).split("/")[0]
        base = core if (core / "ops/ansible/roles" / name).is_dir() else root
        return base / "ops/ansible/roles" / rel


roles = Roles()
ansible_playbook = shutil.which("ansible-playbook") or "ansible-playbook"
vars = {
    "operator_idp_minio_client_secret": "fixture-oidc",
    "ghcr_org": "fixture",
    "ansible_date_time": {"iso8601": "2026-10-26T12:00:00Z"},
    "site_id": "fixture",
    "minio_service_host": "storage.example.invalid",
    "minio_root_user": "fixture-admin",
    "minio_root_password": "fixture/password",
    "site_dns_zone": "example.invalid",
    "ansible_managed": "local fixture",
    "audit_subscriber_id": "site",
    "audit_subscriber_s3_endpoint": "http://127.0.0.1:19000",
    "audit_subscriber_s3_access_key_value": "fixture-audit-write",
    "audit_subscriber_s3_secret_key_value": "fixture-secret",
    "audit_subscriber_replay_secret_value": "fixture-replay-secret",
    "audit_subscriber_local_ndjson_dir": "/fixture/tail",
    "minio_replay_bucket_quota_gib": 1,
    "minio_extra_users": [
        {
            "name": "fixture-existing",
            "policy_name": "fixture-existing",
            "policy_src": "fixture.json",
            "secret_envvar": "FIXTURE_SECRET",
        }
    ],
    "portal_audit_s3_endpoint": "http://127.0.0.1:19000",
    "portal_operator_api_key": "fixture-operator",
    "minio_effective_users": "{{ minio_extra_users + (minio_replay_users if minio_replay_enabled | bool else []) }}",
}
tasks = []
for enabled in [False, True]:
    for oidc in [False, True]:
        dest = out / f"{int(enabled)}-{int(oidc)}"
        dest.mkdir(exist_ok=True)
        tasks.append(
            {
                "name": f"Select fixture flags {enabled} {oidc}",
                "ansible.builtin.set_fact": {
                    "audit_subscriber_v2_enabled": enabled,
                    "audit_subscriber_replay_enabled": enabled,
                    "minio_operator_idp_enabled": oidc,
                    "minio_tls_enabled": enabled,
                },
            }
        )
        for role, template in [
            ("minio", "mc-init.sh.j2"),
            ("minio", "docker-compose.yml.j2"),
            ("minio", "mc-extra-users.env.j2"),
            ("minio", "env.j2"),
            ("minio", "traefik-minio.yml.j2"),
            ("openobserve", "docker-compose.yml.j2"),
            ("openobserve", "env.j2"),
            ("audit-subscriber", "docker-compose.yml.j2"),
            ("audit-subscriber", "env.j2"),
            ("goat-zenoh-portal", "portal.env.j2"),
        ]:
            tasks.append(
                {
                    "name": f"Render {role} {template}",
                    "vars": {"role_path": str(roles / role)},
                    "ansible.builtin.template": {
                        "src": str(roles / role / "templates" / template),
                        "dest": str(dest / f"{role}-{template[:-3]}"),
                        "mode": "0600",
                    },
                }
            )
        tasks.append({
            "name": "Render actual standalone observability compose",
            "ansible.builtin.template": {
                "src": str(core / "ops/compose/goat-obs/docker-compose.yml"),
                "dest": str(dest / "goat-obs.yml"), "mode": "0600",
            },
        })
play = [
    {
        "name": "Local template matrix only",
        "hosts": "localhost",
        "connection": "local",
        "gather_facts": False,
        "vars_files": [
            str(roles / r / "defaults/main.yml")
            for r in ["minio", "audit-subscriber", "goat-zenoh-portal", "openobserve"]
        ],
        "vars": vars,
        "tasks": tasks,
    }
]
path = out / "play.yml"
path.write_text(yaml.safe_dump(play, sort_keys=False))
env = {
    "PATH": os.environ["PATH"],
    "HOME": str(out),
    "ANSIBLE_CONFIG": str(out / "ansible.cfg"),
    "ANSIBLE_LOCAL_TEMP": str(out / "tmp"),
    "FIXTURE_SECRET": "fixture-existing-secret",
    "FIXTURE_REPLAY_WRITE_SECRET": "fixture-replay-write",
    "FIXTURE_REPLAY_READ_SECRET": "fixture-replay-read",
}
(out / "ansible.cfg").write_text("[defaults]\nretry_files_enabled = False\n")
(out / "vars.yml").write_text(yaml.safe_dump(vars))
result = subprocess.run(
    [
        ansible_playbook,
        "-i",
        "localhost,",
        "-e",
        "@" + str(out / "vars.yml"),
        str(path),
    ],
    env=env,
    capture_output=True,
    text=True,
)
(out / "ansible.log").write_text(result.stdout + result.stderr)
if result.returncode:
    print(result.stdout[-4000:] + result.stderr[-1000:])
    raise SystemExit(result.returncode)
for enabled in [False, True]:
    for oidc in [False, True]:
        dest = out / f"{int(enabled)}-{int(oidc)}"
        config = subprocess.run([
            "docker", "compose", "-p", "audit-render-fixture", "-f", str(dest / "goat-obs.yml"),
            "config", "--format", "json", "--no-interpolate", "--no-env-resolution",
        ], capture_output=True, text=True, env=dict(os.environ, OBS_PREFIX="fixture", SITE_NAME="fixture"))
        assert config.returncode == 0, config.stderr
        model = json.loads(config.stdout)
        mounts = model["services"]["prometheus"]["volumes"]
        assert any(v["target"] == "/storage-ca/ca.crt" for v in mounts) == enabled
        script = (dest / "minio-mc-init.sh").read_text()
        assert ("replay-expiration" in script) == enabled
        assert ('mc quota set "local/fabric-replay" --size "1GiB"' in script) == enabled
        assert ("goat-replay-write" in script) == enabled
        assert "fixture-existing" in script
        subscriber = yaml.safe_load(
            (dest / "audit-subscriber-docker-compose.yml").read_text()
        )["services"]["audit-subscriber"]
        assert ("AUDIT_SUBSCRIBER_STATE_DIR" in subscriber["environment"]) == enabled
        assert (
            "AUDIT_SUBSCRIBER_REPLAY_BUCKET" in subscriber["environment"]
        ) == enabled
        if enabled:
            assert subscriber["stop_grace_period"] == "70s"
            assert subscriber["environment"]["AUDIT_SUBSCRIBER_RECEIVING_SITE"] == "fixture"
            assert subscriber["environment"]["AUDIT_SUBSCRIBER_S3_CA_BUNDLE"] == "/storage-ca/ca.crt"
        storage = yaml.safe_load((dest / "minio-docker-compose.yml").read_text())["services"]
        assert ("--certs-dir" in storage["minio"]["command"]) == enabled
        healthcheck = storage["minio"]["healthcheck"]["test"]
        assert healthcheck[1] == "wget"
        assert ("--ca-certificate=/certs/ca.crt" in healthcheck) == enabled
        assert "--no-check-certificate" not in healthcheck
        assert healthcheck[-1].startswith("https://storage.example.invalid:") == enabled
        assert ("/etc/ssl/certs/goat-storage-root.pem:ro" in (dest / "minio-docker-compose.yml").read_text()) == enabled
        assert "fixture%2Fpassword" in (dest / "minio-env").read_text()
        telemetry = yaml.safe_load((dest / "openobserve-docker-compose.yml").read_text())["services"]["openobserve"]
        assert ("SSL_CERT_FILE" in telemetry["environment"]) == enabled

        portal = (dest / "goat-zenoh-portal-portal.env").read_text()
        assert ("PORTAL_REPLAY_S3_BUCKET=fabric-replay" in portal) == enabled
        subprocess.run(["/bin/sh", "-n", str(dest / "minio-mc-init.sh")], check=True)
print(
    "PASS: production templates, replay enabled/disabled and OIDC enabled/disabled, existing user preservation, state mount and separate credentials"
)

# Exercise the actual rendered attachment branch: success, duplicate, failure.
script = (out / "1-0/minio-mc-init.sh").read_text()
start = script.index("if attach_out=$(mc admin policy attach")
end = script.index("\nfi", start) + len("\nfi")
branch = script[start:end]
for status, output, expected in [
    (0, "attached", 0),
    (1, "The specified policy change is already in effect.", 0),
    (1, "The specified policy is already attached.", 0),
    (1, "Access Denied.", 1),
    (1, "connection refused", 1),
]:
    stub = f"mc() {{ echo '{output}'; return {status}; }}\n"
    result = subprocess.run(
        ["/bin/sh", "-c", stub + branch], capture_output=True, text=True
    )
    assert result.returncode == expected, (status, output, result.stderr)
print("PASS: policy attachment failures propagate; exact idempotent responses succeed")

# Execute only the production role's two storage preflight tasks, never deployment.
preflight = [t for t in yaml.safe_load((roles / "minio/tasks/deploy.yml").read_text()) if t.get("name") in ["Select existing service users and optional replay identities", "Validate replay storage declarations"]]
assert len(preflight) == 2, "production replay preflights must be exercised"
validation_play = [
    {
        "hosts": "localhost",
        "connection": "local",
        "gather_facts": False,
        "vars_files": [str(roles / "minio/defaults/main.yml")],
        "tasks": preflight,
    }
]
validation_path = out / "validate.yml"
validation_path.write_text(yaml.safe_dump(validation_play, sort_keys=False))
for index, (override, accepted) in enumerate(
    [
        ({}, True),
        ({"minio_replay_bucket": "audit-integrity"}, False),
        ({"minio_replay_bucket": "telemetry-otlp"}, False),
        ({"minio_replay_bucket": "runtime-detection-archive"}, False),
        ({"minio_replay_bucket_quota_gib": None}, False),
        ({"minio_replay_bucket_quota_gib": 0}, False),
        ({"audit_subscriber_id": "foreign/namespace"}, False),
        (
            {
                "minio_extra_users": [
                    {"name": "goat-replay-read", "policy_name": "existing"}
                ]
            },
            False,
        ),
        (
            {"minio_extra_users": [{"name": "existing", "policy_name": "replay-read"}]},
            False,
        ),
    ]
):
    case_vars = dict(vars, minio_replay_enabled=True, **override)
    case_path = out / f"validation-{index}.yml"
    case_path.write_text(yaml.safe_dump(case_vars))
    result = subprocess.run(
        [
            ansible_playbook,
            "-i",
            "localhost,",
            "-e",
            "@" + str(case_path),
            str(validation_path),
        ],
        env=env,
        capture_output=True,
        text=True,
    )
    (out / f"validation-{index}.log").write_text(result.stdout + result.stderr)
    assert (result.returncode == 0) == accepted, (index, result.stdout, result.stderr)
print(
    "PASS: actual role preflights reject bucket aliases, missing quotas and identity/policy collisions"
)

# Deployment gates are exercised separately from the retained synthetic replay templates.
for role, flag in [("minio", "minio_replay_enabled"), ("audit-subscriber", "audit_subscriber_replay_enabled"), ("goat-zenoh-portal", "portal_replay_enabled")]:
    task_file = "tasks/deploy.yml" if role == "minio" else "tasks/main.yml"
    gate = next(t for t in yaml.safe_load((roles / role / task_file).read_text()) if t.get("name") == "Refuse replay enablement pending owner governance")
    for enabled in [False, True]:
        gate_play = [{"hosts": "localhost", "connection": "local", "gather_facts": False, "vars": {flag: enabled}, "tasks": [gate]}]
        gate_path = out / f"gate-{role}-{enabled}.yml"
        gate_path.write_text(yaml.safe_dump(gate_play))
        check = subprocess.run([ansible_playbook, "-i", "localhost,", str(gate_path)], env=env, capture_output=True, text=True)
        (out / f"gate-{role}-{enabled}.log").write_text(check.stdout + check.stderr)
        assert (check.returncode == 0) == (not enabled), role
print("PASS: all deployment roles reject replay enablement; native TLS and public CA mounts render")
