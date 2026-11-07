<h1 align="center">goatstuff</h1>

<p align="center">
  Inspection tools and demonstration applications that run on a goat data fabric.
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License: Apache 2.0"></a>
</p>

> [!NOTE]
> goat is pre-1.0: interfaces can still change between releases.

The inspection surfaces, demos and example applications that run on a goat data fabric. Everything here sits beside [`goatcore`](https://github.com/usareur-af/goatcore) — the mesh, the fabric, identity, the build tool and the operator CLI — and none of it is needed to run a goatnet.

| Surface | What it shows | Where |
|---|---|---|
| **panoscope** | the whole goatnet at a glance — the only surface that queries the fabric on request | `ops/panoscope/`, role `ops/ansible/roles/panoscope/`, design `docs/design/panoscope-view-model.md` |
| **stethoscope** | listens to one goatnet's health publications and grades them | `crates/stethoscope/`, `crates/stethoscope-checks/` |
| **flashlight** | audit-tail analytics: who did what, when | `ops/flashlight/`, `crates/flashlight-analytics/` |
| **netvis** | the mesh ACL as a picture, gated by `goat acl test` assertions | `crates/netvis/` |
| **microscope** | one peer in detail | `crates/microscope/` |
| **trashgoat** | operator-controlled chaos publisher for pub/sub fabrics | `ops/goat-trashgoat/` |
| **goat-topology**, **netview**, **peerview** | topology and peer views | `ops/goat-topology/`, `ops/netview/`, `ops/peerview/` |
| **gym** | the training surface: modules teach in place | `ops/gym/` |
| **example apps** | dataflows, the map, the browser playground, the SPIFFE trust sandbox | `ops/serve/*` |
| **synthetic vendors and trial peers** | publishers that stand in for real sensors | `ops/zenoh-trial-peers/`, `ops/zenoh-shims/` |

The design documents published so far are under `docs/design/`; a surface without one is described by its role and code. A surface that deploys has an Ansible role and a playbook under `ops/ansible/` that assume a goatnet built from `goatcore`.

## Base images

Every image here builds on base images named by role, `ops/base-images/<role>/Dockerfile`: `rust-build`, `node-build` and `runtime-cc`. The defaults are Chainguard's free images, pinned by digest. To build on your own bases, such as a private Chainguard catalogue, put your own role files at the same paths in your environment repository and copy it over this checkout before building: [build goat images on your own base images](https://github.com/usareur-af/goatcore/blob/main/docs/operations/build-on-your-own-base-images.md). `ops/base-images/base-images.py` is the same file as goatcore's.

Each surface's status, what you bring and what needs your configuration: [docs/RELEASE-CONTENTS.md](docs/RELEASE-CONTENTS.md).

## Building

The Rust crates share one workspace and depend on `panoscope-core` from `goatcore` by git tag (see `crates/Cargo.toml`). `cd crates && cargo build --release` builds every surface. Images are published to `ghcr.io/usareur-af/<surface>` by the workflows under `.github/workflows/`.

## Contributing and licence

[CONTRIBUTING.md](CONTRIBUTING.md) · [SECURITY.md](SECURITY.md) · [GOVERNANCE.md](GOVERNANCE.md) · Apache License 2.0 ([LICENSE](LICENSE), [NOTICE](NOTICE)).

## Licence

`goatstuff` was written by U.S. Federal government employees. In the U.S. it is not subject to copyright and is in the public domain; in countries where copyright is recognized, it is released under the [Apache License 2.0](LICENSE). See [INTENT.md](INTENT.md).
