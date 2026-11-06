#!/usr/bin/env python3
"""Build a registry-only Zenoh 1.0.4 router, then test the real feeds executable.

The disposable router uses the same resolved versions as the candidate except
the three adapter packages come from their original, checksum-verified registry
archives. It inherits no path patches and creates no tracked workspace/lock.
"""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib


def main():
    root = Path(__file__).resolve().parents[1]
    # Share compiled dependencies in CI as well as in local invocations. The
    # fixture's registry package IDs remain distinct from the path adapters.
    build_env = dict(os.environ)
    build_env.setdefault("CARGO_TARGET_DIR", str(root / "target"))
    provenance = json.loads((root / "vendor/provenance.json").read_text())
    manifest = (root / "Cargo.toml").read_text()
    assert manifest.count("[patch.crates-io]") == 1
    manifest = manifest.split("[patch.crates-io]", 1)[0]
    before = 'name = "feeds-shim"\npath = "src/main.rs"'
    assert manifest.count(before) == 1
    manifest = manifest.replace(before, 'name = "upstream-fixture-producer"\npath = "src/main.rs"')
    lock = (root / "Cargo.lock").read_text()
    for package in provenance["packages"]:
        pattern = (r'(\[\[package\]\]\nname = "' + re.escape(package["package"]) +
                   r'"\nversion = "1\.0\.4"\n)(?=dependencies = )')
        lock, count = re.subn(pattern, lambda m: m.group(1) +
                             'source = "registry+https://github.com/rust-lang/crates.io-index"\n' +
                             'checksum = "' + package["archive_sha256"] + '"\n', lock)
        assert count == 1, package["package"]
    with tempfile.TemporaryDirectory(prefix="feeds-upstream-router-") as directory:
        fixture = Path(directory)
        shutil.copytree(root / "src", fixture / "src")
        shutil.copytree(root / "tests", fixture / "tests")
        shutil.copytree(root / "schemas", fixture / "schemas")
        (fixture / "Cargo.toml").write_text(manifest)
        (fixture / "Cargo.lock").write_text(lock)
        # Reintroduce only the legacy adapter helper needed by this isolated
        # upstream control. Refuse any unrelated version resolution change.
        metadata = subprocess.run(
            ["cargo", "metadata", "--manifest-path", str(fixture / "Cargo.toml"),
             "--format-version=1"], check=True, text=True, stdout=subprocess.PIPE, env=build_env)
        packages = json.loads(metadata.stdout)["packages"]
        expected = {(p["name"], p["version"])
                    for p in tomllib.loads((root / "Cargo.lock").read_text())["package"]}
        actual = {(p["name"], p["version"]) for p in packages}
        assert actual == expected | {("rustls-webpki", "0.102.8")}, (actual - expected, expected - actual)
        for original in provenance["packages"]:
            matches = [p for p in packages if p["name"] == original["package"]]
            assert len(matches) == 1 and matches[0]["version"] == "1.0.4"
            assert matches[0]["source"] == "registry+https://github.com/rust-lang/crates.io-index"
        result = subprocess.run(
            ["cargo", "test", "--locked", "--manifest-path", str(fixture / "Cargo.toml"),
             "--test", "transport", "--no-run", "--message-format=json"],
            check=True, text=True, stdout=subprocess.PIPE, env=build_env)
        artifacts = [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]
        binaries = [a["executable"] for a in artifacts
                    if a.get("reason") == "compiler-artifact" and a.get("executable")
                    and a["target"]["name"] == "transport" and a["profile"]["test"]]
        assert len(binaries) == 1, binaries
        print("Verified independent registry-only Zenoh 1.0.4 router", flush=True)
        producer = subprocess.run(
            ["cargo", "build", "--locked", "--manifest-path", str(root / "Cargo.toml"),
             "--bin", "feeds-shim", "--message-format=json"],
            check=True, text=True, stdout=subprocess.PIPE, env=build_env)
        artifacts = [json.loads(line) for line in producer.stdout.splitlines() if line.startswith("{")]
        producers = [a["executable"] for a in artifacts
                     if a.get("reason") == "compiler-artifact" and a.get("executable")
                     and a["target"]["name"] == "feeds-shim" and not a["profile"]["test"]]
        assert len(producers) == 1, producers
        env = dict(build_env, FEEDS_UPSTREAM_TEST_BIN=binaries[0], FEEDS_PRODUCER_BIN=producers[0])
        subprocess.run(
            ["cargo", "test", "--locked", "--manifest-path", str(root / "Cargo.toml"),
             "--test", "transport", "--", "--ignored", "--exact",
             "production_binary_preserves_tls_quic_and_compression", "--nocapture"],
            check=True, env=env)


if __name__ == "__main__":
    main()
