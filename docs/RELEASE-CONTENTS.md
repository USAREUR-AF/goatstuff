# What this release contains

This repository, `goatstuff`, holds the surfaces, demos and example applications that run on a goat data fabric. None of them is needed to run a goatnet. Each one that deploys assumes a goatnet built from [`goatcore`](https://github.com/usareur-af/goatcore), whose [release contents](https://github.com/usareur-af/goatcore/blob/main/docs/RELEASE-CONTENTS.md) also describe the other three repositories.

## Components

**Tested in CI** means a workflow under `.github/workflows/` builds it and runs its tests on each pull request. **Built** means a workflow checks that it parses or compiles, and no workflow runs its behaviour. **Not run** means no workflow here runs it.

| Component | Path | Status | Workflow |
|---|---|---|---|
| stethoscope, netvis, microscope, flashlight analytics | `crates/` | Tested in CI | `ci.yml`, job `rust`; needs tag `goatcore/v0.2.0` of `goatcore` |
| panoscope web interface | `ops/panoscope/frontend/` | Tested in CI | `panoscope-frontend-test.yml` |
| Map surface | `ops/serve/goat-map/` | Tested in CI | `ci.yml` runs `tests-public/` |
| Peer view | `ops/peerview/` | Tested in CI | `ci.yml` |
| Browser playground, trust sandbox, dataflows | `ops/serve/` | Built | image workflows |
| Topology, trashgoat, synthetic vendors, health publishers | `ops/goat-topology/`, `ops/goat-trashgoat/`, `ops/zenoh-trial-peers/` | Built | image workflows |
| Gym training surface | `ops/gym/` | Not run | |
| Roles and playbooks | `ops/ansible/` | Built | `ci.yml` checks that playbooks parse and role references resolve |
| Container images | `*-image.yml` workflows | Not run | They run on a push to `main` |

## What you bring

To build, you need Rust; `crates/Cargo.toml` fetches `panoscope-core` from `goatcore` by tag. To deploy, you need a running goatnet. Deploying a surface also takes its image and the goatnet's own values.

## What needs your configuration

| Thing | What it needs |
|---|---|
| Any surface on a goatnet | The goatnet's inventory and group vars, from `goatcore` or your own overlay |
| Map surface | A profile in `ops/serve/goat-map/deploy/goatnets/`, from `.env.example`, and a basemap you supply |
| Netvis and the other surfaces that compile in a mesh policy | `ops/ansible/netbird_acl_matrix.yml` holds an example. Replace it with your goatnet's generated file and rebuild |
| Images | The workflows publish to `ghcr.io/<your owner>/` with `GITHUB_TOKEN` alone |

## How to change the main things

| To change | Edit | Then run |
|---|---|---|
| A surface | its directory in the first table | `cd crates && cargo test --workspace`, or `python3 -m pytest -q ops` for Python |
| How a surface deploys | `ops/ansible/roles/<surface>/` | `ansible-playbook --syntax-check` on its playbook |
| The `goatcore` version | the `tag` in `crates/Cargo.toml` | `cd crates && cargo update` |
| A gym module | `ops/gym/gym.yaml` | `python3 ops/gym/generate.py` |

Design documents are under `docs/design/`.
