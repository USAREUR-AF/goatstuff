# AGENTS.md

Instructions for AI coding agents, and a short version for anyone working here. [CONTRIBUTING.md](CONTRIBUTING.md) is the full process.

## What this repository is

goatstuff holds the inspection tools and demonstration services that run on a goatnet: panoscope, netvis, flashlight, stethoscope, microscope, trashgoat, the fabric health publishers, the synthetic data producers and the hosted-service registry.

It sits beside [goatcore](https://github.com/usareur-af/goatcore), which builds and runs a goatnet. A real deployment is goatcore at a pinned commit plus a private overlay repository with that deployment's configuration; no deployment's configuration is ever in this repository.

## Layout

| Path | What |
|---|---|
| `crates/` | Rust workspace: `netvis`, `stethoscope`, `stethoscope-checks`, `microscope`, `flashlight-analytics` |
| `ops/<surface>/` | each surface's service code, Dockerfile and tests (`panoscope`, `flashlight`, `goat-trashgoat`, `netview`, `peerview`, …) |
| `ops/zenoh-trial-peers/` | fabric health publishers and synthetic producers |
| `ops/ansible/` | the roles that deploy these surfaces |
| `docs/design/` | one design document per surface |

## Rules CI enforces

1. **No identifier of a real deployment.** Hostnames, domains, addresses, account ids, keys, people, places and organisations of any real goatnet never enter a file or a commit message. Use the example goatnet `zagros`, documentation addresses (RFC 5737, and `100.127.0.0/16` for mesh), and `example.org` mail. The leak gate (`.github/workflows/leak-gate.yml`) checks every added line against a private list of deployment names. It never prints what matched, so a red leak gate means: rename the identifier. The name list is not available outside CI.
2. **Commits.** Conventional Commits subject of 100 characters or fewer, with a scope (`fix(<component>): …`). Every commit is signed off (`git commit -s`, the DCO) and signed. One logical change per pull request.
3. **Design documents move with the code.** A change to behaviour, data flow, topology or an integration point updates its design document in the same pull request, using the status words built, measured, designed and not built exactly. Decisions are ADRs in [goatcore](https://github.com/usareur-af/goatcore/blob/main/docs/adr/INDEX.md).
4. **A file at the same path as one in goatcore stays byte-identical to it.** Deployments assemble goatcore, goatstuff and goatrodeo into one tree, and the assembly stops on any path two of them carry with different bytes (the exceptions are each repository's root documents, `.github/`, `crates/Cargo.toml`, `crates/Cargo.lock` and `docs/RELEASE-CONTENTS.md`). Change such a file in goatcore first, then copy it here unchanged. To list them: `comm -12 <(git ls-files | sort) <(git -C ../goatcore ls-files | sort)`.

## Before you push

```bash
cd crates && cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace
bash ops/ci/check-no-identity-spillage.sh
bash ops/ci/check-bash-portability.sh        # shell runs on bash 3.2
```

The gates table in [CONTRIBUTING.md](CONTRIBUTING.md#gates) maps each kind of change to its script.

## Read first

- [architecture-spine.md](https://github.com/usareur-af/goatdocs/blob/main/docs/design/architecture-spine.md) in goatdocs: the architecture in one read.
- [zenoh-brokerless-consequences.md](https://github.com/usareur-af/goatcore/blob/main/docs/design/zenoh-brokerless-consequences.md) in goatcore: no surface may turn a viewer's request into load on the fabric.
