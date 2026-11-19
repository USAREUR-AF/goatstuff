#!/usr/bin/env python3
"""Run rendered replay lifecycle/IAM policies against a disposable loopback MinIO.

Requires local minio/mc binaries, Jinja2, and an empty evidence directory. All
credentials are synthetic. No deployed endpoints or operator credentials are used.
"""

import argparse
import json
import os
from pathlib import Path
import socket
import ssl
import shutil
import subprocess
import time
import urllib.request

from jinja2 import Environment, FileSystemLoader, StrictUndefined, select_autoescape


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def certificates(directory, names="DNS:localhost,IP:127.0.0.1"):
    directory.mkdir(mode=0o700)
    def openssl(*args):
        subprocess.run(["openssl", *args], cwd=directory, check=True, capture_output=True)
    openssl("req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-subj", "/CN=Fixture Organization", "-keyout", "ca.key", "-out", "ca.crt")
    openssl("req", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=localhost", "-keyout", "private.key", "-out", "server.csr")
    (directory / "extensions").write_text(f"subjectAltName={names}\nextendedKeyUsage=serverAuth\n")
    openssl("x509", "-req", "-in", "server.csr", "-CA", "ca.crt", "-CAkey", "ca.key", "-CAcreateserial", "-days", "2", "-extfile", "extensions", "-out", "public.crt")
    for key in directory.glob("*.key"):
        key.chmod(0o600)


def run(minio, mc, evidence, core, tls=False):
    # This file lives in goatstuff; the minio role and policies live in goatcore.
    root = Path(__file__).resolve().parents[4]
    templates = core / "ops/ansible/roles/minio/templates"
    env_jinja = Environment(
        loader=FileSystemLoader(templates), undefined=StrictUndefined,
        # These IAM/lifecycle templates emit JSON; escape HTML/XML templates only.
        autoescape=select_autoescape(),
    )
    variables = {
        "minio_replay_bucket": "fabric-replay",
        "minio_replay_retention_days": 30,
        "minio_replay_bucket_quota_gib": 1,
        "audit_subscriber_id": "site",
    }
    evidence.mkdir(parents=True, exist_ok=False)
    api, console = free_port(), free_port()
    scheme = "https" if tls else "http"
    endpoint = f"{scheme}://127.0.0.1:{api}"
    context = None
    if tls:
        certificates(evidence / "certs")
        certificates(evidence / "wrong-certs", "DNS:wrong.invalid")
        ca_dir = evidence / "mc-config/certs/CAs"
        ca_dir.mkdir(parents=True)
        shutil.copyfile(evidence / "certs/ca.crt", ca_dir / "organization.crt")
        context = ssl.create_default_context(cafile=str(evidence / "certs/ca.crt"))
    env = dict(
        os.environ,
        MINIO_ROOT_USER="fixture-admin",
        MINIO_ROOT_PASSWORD="fixture-admin-password",
        MINIO_BROWSER="off",
        MC_CONFIG_DIR=str(evidence / "mc-config"),
    )
    env["MC_HOST_local"] = (
        f"{scheme}://fixture-admin:fixture-admin-password@127.0.0.1:{api}"
    )
    env["PATH"] = str(mc.parent) + os.pathsep + env["PATH"]
    with (evidence / "minio.log").open("w") as log:
        server = subprocess.Popen(
            [
                str(minio),
                "server",
                str(evidence / "data"),
                "--address",
                f"127.0.0.1:{api}",
                "--console-address",
                f"127.0.0.1:{console}",
            ] + (["--certs-dir", str(evidence / "certs")] if tls else []),
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
        )
        try:
            for _ in range(500):
                if server.poll() is not None:
                    raise RuntimeError("fixture MinIO exited; inspect minio.log")
                try:
                    with urllib.request.urlopen(
                        endpoint + "/minio/health/live", timeout=1, context=context
                    ) as response:
                        if response.status == 200:
                            break
                except OSError:
                    time.sleep(0.1)
            else:
                raise RuntimeError("fixture MinIO readiness timed out")

            if tls:
                for ctx, name in [(ssl.create_default_context(), "localhost"), (context, "wrong.invalid")]:
                    try:
                        with socket.create_connection(("127.0.0.1", api), timeout=3) as sock:
                            with ctx.wrap_socket(sock, server_hostname=name):
                                raise AssertionError("untrusted or wrong-host TLS succeeded")
                    except ssl.SSLCertVerificationError:
                        pass
                try:
                    urllib.request.urlopen(f"http://127.0.0.1:{api}/minio/health/live", timeout=3)
                except (OSError, urllib.error.HTTPError):
                    pass
                else:
                    raise AssertionError("TLS listener accepted plaintext health request")

            def command(*args, success=True, stdin=None):
                result = subprocess.run(
                    [str(mc), "--no-color", *args],
                    input=stdin,
                    env=env,
                    capture_output=True,
                    text=True,
                    timeout=30,
                )
                if (result.returncode == 0) != success:
                    # Arguments may contain fixture credentials; never print them.
                    raise AssertionError(
                        f"mc {args[0]} unexpected exit {result.returncode}: {result.stderr[:500]}"
                    )
                return result.stdout

            def provision(bucket, success=True):
                content = env_jinja.get_template("replay-bucket.sh.j2").render(
                    **dict(variables, minio_replay_bucket=bucket)
                )
                script = evidence / (bucket + ".sh")
                script.write_text(content)
                # The template uses /tmp for transient policy JSON. Scope it to
                # the fixture directory, preserving every command and policy.
                script.write_text(
                    content.replace(
                        "/tmp/replay-lifecycle.json",
                        str(evidence / "replay-lifecycle.json"),
                    )
                )
                result = subprocess.run(
                    ["sh", str(script)],
                    env=env,
                    capture_output=True,
                    text=True,
                    timeout=30,
                )
                if (result.returncode == 0) != success:
                    raise AssertionError(
                        f"replay provisioning exit {result.returncode}: {result.stderr[:500]}"
                    )

            command("mb", "--with-lock", "local/audit-integrity")
            provision("fabric-replay")
            provision("fabric-replay")  # idempotent
            policy = json.loads(command("ilm", "rule", "export", "local/fabric-replay"))
            assert policy["Rules"][0]["Expiration"]["Days"] == 30
            quota = json.loads(
                command("--json", "quota", "info", "local/fabric-replay")
            )
            assert quota["quota"] == 1024**3 and quota["type"] == "hard"
            assert "is un-versioned" in command(
                "version", "info", "local/fabric-replay"
            )
            for role in ("read", "write"):
                path = evidence / ("replay-" + role + ".json")
                path.write_text(
                    env_jinja.get_template("replay-" + role + ".json.j2").render(
                        **variables
                    )
                )
                command(
                    "admin", "policy", "create", "local", "replay-" + role, str(path)
                )
                command(
                    "admin",
                    "user",
                    "add",
                    "local",
                    "fixture-replay-" + role,
                    "fixture-replay-password",
                )
                command(
                    "admin",
                    "policy",
                    "attach",
                    "local",
                    "replay-" + role,
                    "--user",
                    "fixture-replay-" + role,
                )
                env["MC_HOST_replay" + role] = (
                    f"{scheme}://fixture-replay-{role}:fixture-replay-password@127.0.0.1:{api}"
                )
            for role in ("read", "write"):
                command(
                    "admin",
                    "policy",
                    "create",
                    "local",
                    "audit-" + role,
                    str(core / f"ops/minio/policies/audit-{role}.json"),
                )
                command(
                    "admin",
                    "user",
                    "add",
                    "local",
                    "fixture-audit-" + role,
                    "fixture-audit-password",
                )
                command(
                    "admin",
                    "policy",
                    "attach",
                    "local",
                    "audit-" + role,
                    "--user",
                    "fixture-audit-" + role,
                )
                env["MC_HOST_audit" + role] = (
                    f"{scheme}://fixture-audit-{role}:fixture-audit-password@127.0.0.1:{api}"
                )
            command(
                "pipe",
                "replaywrite/fabric-replay/site/record",
                stdin="synthetic replay",
            )
            assert (
                command("cat", "replayread/fabric-replay/site/record")
                == "synthetic replay"
            )
            command("pipe", "auditwrite/audit-integrity/site/record", stdin="metadata")
            assert command("cat", "auditread/audit-integrity/site/record") == "metadata"
            command("cat", "auditread/fabric-replay/site/record", success=False)
            command("cat", "replayread/audit-integrity/site/record", success=False)
            command(
                "pipe",
                "replaywrite/audit-integrity/site/forbidden",
                stdin="forbidden",
                success=False,
            )
            command(
                "pipe",
                "replayread/fabric-replay/site/forbidden",
                stdin="forbidden",
                success=False,
            )
            command(
                "pipe",
                "replaywrite/fabric-replay/foreign/forbidden",
                stdin="forbidden",
                success=False,
            )
            command("pipe", "local/fabric-replay/foreign/record", stdin="foreign")
            command("cat", "replayread/fabric-replay/foreign/record", success=False)
            command("mb", "--with-lock", "local/locked-replay")
            provision("locked-replay", success=False)
            command("mb", "local/versioned-replay")
            command("version", "enable", "local/versioned-replay")
            provision("versioned-replay", success=False)
            command("version", "suspend", "local/versioned-replay")
            provision("versioned-replay", success=False)
            if tls:
                test_env = dict(env, GOAT_FIXTURE_TLS_ENDPOINT=endpoint, GOAT_FIXTURE_TLS_CA=str(evidence / "certs/ca.crt"), GOAT_FIXTURE_TLS_BAD_CA=str(evidence / "wrong-certs/ca.crt"), CARGO_TARGET_DIR=str(Path.home() / ".cache/goat-audit-target"))
                check = subprocess.run(["cargo", "test", "--offline", "--manifest-path", str(root / "ops/zenoh-trial-peers/audit-subscriber/Cargo.toml"), "native_tls_minio_round_trip", "--", "--ignored"], env=test_env, capture_output=True, text=True, timeout=600)
                (evidence / "subscriber-tls.log").write_text(check.stdout + check.stderr)
                if check.returncode:
                    raise AssertionError("subscriber TLS round trip failed; see subscriber-tls.log")
                server.terminate()
                server.wait(timeout=15)
                server = subprocess.Popen([str(minio), "server", str(evidence / "data"), "--address", f"127.0.0.1:{api}", "--console-address", f"127.0.0.1:{console}", "--certs-dir", str(evidence / "wrong-certs")], env=env, stdout=log, stderr=subprocess.STDOUT)
                for _ in range(100):
                    try:
                        with socket.create_connection(("127.0.0.1", api), timeout=1):
                            break
                    except OSError:
                        time.sleep(0.1)
                else:
                    raise RuntimeError("wrong-host TLS fixture did not start")
                bad_env = dict(test_env, GOAT_FIXTURE_TLS_WRONG_HOST="1", GOAT_FIXTURE_TLS_CA=str(evidence / "wrong-certs/ca.crt"))
                check = subprocess.run(["cargo", "test", "--offline", "--manifest-path", str(root / "ops/zenoh-trial-peers/audit-subscriber/Cargo.toml"), "native_tls_minio_round_trip", "--", "--ignored"], env=bad_env, capture_output=True, text=True, timeout=600)
                (evidence / "subscriber-wrong-host.log").write_text(check.stdout + check.stderr)
                if check.returncode:
                    raise AssertionError("subscriber wrong-host rejection failed; see subscriber-wrong-host.log")
            (evidence / "result.json").write_text(
                json.dumps(
                    {
                        "passed": True,
                        "native_tls": tls,
                        "sdk_untrusted_ca_rejected": tls,
                        "sdk_wrong_hostname_rejected": tls,
                        "sdk_plaintext_rejected": tls,
                        "retention_days": 30,
                        "unversioned": True,
                        "iam_isolated": True,
                        "locked_and_versioned_refused": True,
                    }
                )
                + "\n"
            )
            print(
                "PASS: rendered replay lifecycle, idempotency, IAM isolation and incompatible bucket refusal"
            )
        finally:
            server.terminate()
            try:
                server.wait(timeout=15)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=5)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--minio", required=True, type=Path)
    parser.add_argument("--mc", required=True, type=Path)
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--tls", action="store_true")
    parser.add_argument(
        "--goatcore", type=Path, default=os.environ.get("GOATCORE_ROOT"),
        help="goatcore checkout holding ops/ansible/roles/minio and ops/minio/policies (default: $GOATCORE_ROOT)")
    args = parser.parse_args()
    if args.goatcore is None or not (args.goatcore / "ops/ansible/roles/minio/templates").is_dir():
        parser.error("--goatcore (or GOATCORE_ROOT) must name a goatcore checkout")
    missing = [str(p) for p in (args.goatcore / f"ops/minio/policies/audit-{r}.json" for r in ("read", "write")) if not p.is_file()]
    if missing:
        parser.error("inputs missing from the goatcore checkout: " + ", ".join(missing))
    run(args.minio.resolve(), args.mc.resolve(), args.evidence.resolve(), args.goatcore.resolve(), args.tls)
