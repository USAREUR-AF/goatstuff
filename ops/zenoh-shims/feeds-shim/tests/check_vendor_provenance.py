#!/usr/bin/env python3
"""Refuse adapter source changes or manifest edits beyond the reviewed pin."""
import hashlib
import json
from pathlib import Path
import shutil
import sys
import tempfile
import tomllib


def check(root):
    vendor = root / "vendor"
    provenance = json.loads((vendor / "provenance.json").read_text())
    packages = provenance["packages"]
    expected = {"zenoh-link-commons", "zenoh-link-tls", "zenoh-link-quic"}
    assert {p["package"] for p in packages} == expected
    patch = tomllib.loads((root / "Cargo.toml").read_text())["patch"]["crates-io"]
    assert set(patch) == expected
    for package in packages:
        name = package["package"]
        assert patch[name] == {"path": "vendor/" + name}
        assert package["version"] == "1.0.4"
        assert package["upstream_git"] == "030afc154c33375b397089a1e4cbbb52a2a2d9bf"
        directory = vendor / name
        original = package["original_files_sha256"]
        actual = {str(p.relative_to(directory)) for p in directory.rglob("*") if p.is_file()}
        assert actual == set(original), (name, actual ^ set(original))
        for filename, digest in original.items():
            path = Path(filename)
            assert not path.is_absolute() and ".." not in path.parts
            data = (directory / path).read_bytes()
            if filename == "Cargo.toml":
                assert hashlib.sha256(data).hexdigest() == package["modified_manifest_sha256"]
                before = b'[dependencies.rustls-webpki]\nversion = "=0.103.16"'
                after = b'[dependencies.rustls-webpki]\nversion = "0.102.8"'
                assert data.count(before) == 1
                data = data.replace(before, after)
            assert hashlib.sha256(data).hexdigest() == digest, (name, filename)
        source = json.loads((directory / ".cargo_vcs_info.json").read_text())
        assert source["git"]["sha1"] == package["upstream_git"]
    for name, blob in {"LICENSE": "8c3fbf603f21987ed0d9c8999945f2d5f80cdf20",
                       "NOTICE.md": "7aff175a881c02144b247ef522046a73f7029d26"}.items():
        data = (vendor / name).read_bytes()
        assert hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest() == blob


def main():
    root = Path(__file__).resolve().parents[1]
    check(root)
    if sys.argv[1:] == ["--self-test"]:
        with tempfile.TemporaryDirectory(prefix="feeds-provenance-control-") as directory:
            fixture = Path(directory)
            shutil.copytree(root / "vendor", fixture / "vendor")
            shutil.copy2(root / "Cargo.toml", fixture / "Cargo.toml")
            check(fixture)
            for relative in ["vendor/zenoh-link-commons/src/tls.rs", "vendor/zenoh-link-tls/Cargo.toml"]:
                path = fixture / relative
                original = path.read_bytes()
                path.write_bytes(original + b"\n# planted change\n" if path.suffix == ".toml" else original + b"\n// planted change\n")
                try:
                    check(fixture)
                except AssertionError:
                    pass
                else:
                    raise AssertionError("planted violation was accepted: " + relative)
                path.write_bytes(original)
            check(fixture)
        print("PASS: planted source/manifest changes rejected; original controls pass")
    else:
        assert not sys.argv[1:], "usage: check_vendor_provenance.py [--self-test]"
    print("PASS: three published adapters, source unchanged, only reviewed WebPKI pins modified")


if __name__ == "__main__":
    main()
