# panoscope — local demo (laptop)

Run the panoscope inspection surface on your laptop with **live, dynamic
data**, using the same panoscope image as production — no mesh, no Ory, no
TLS, no cloud. One `docker compose` stack; a single container generates all
the notional data.

See the [view-model design](../../docs/design/panoscope-view-model.md) for how
the backend decides what to draw, and the
[synthetic-vendor design](../../docs/design/zenoh-trial-fabric-synth-vendors.md)
§"Fleet mode" for the one-container, many-peer demo.

## Prerequisites

- **Docker Desktop** (or Docker Engine) running. Nothing else — no Rust, no
  Node; everything compiles inside the build.
- ~3 GB free disk for the build cache; TCP **8080** free (the UI).

## Quickstart

From this directory (`ops/panoscope/`):

```bash
./demo.sh up        # build (first run) + start
```

Then open **http://127.0.0.1:8080**. Give it ~30 s to populate (the slow
0.1 Hz peers and the 30 s schema-declarations fill in last).

Equivalent raw command, if you'd rather not use the wrapper:

```bash
docker compose -f docker-compose.dev.yml up --build -d
```

> **First build takes ~15-20 min** (it compiles panoscope + the data
> generator from source). It's fully cached afterwards — subsequent
> `up`s start in seconds.

## What you get

Three containers (`./demo.sh status`):

| Container | What it does |
|---|---|
| `panoscope-dev-zenoh` | Zenoh router — the data backbone (open-ACL, plaintext, dev-only). |
| `panoscope-dev-synth-fleet` | **One** container generating all ~22 notional peers' data (see "How the data works"). |
| `panoscope-dev` | panoscope itself — the web UI on `127.0.0.1:8080`. |

In the browser you'll see the live topology graph (~22 peer nodes with
role-based shapes, animated topic-flow edges), a peer/topic side panel, and a
decoded protobuf/JSON stream view — the same surface that runs hosted, fed by
synthetic data.

Quick sanity check from the CLI:

```bash
curl -s http://127.0.0.1:8080/api/snapshot | python3 -c 'import sys,json;d=json.load(sys.stdin);print("nodes",len(d["nodes"]),"edges",len(d["edges"]))'
```

## Commands

```bash
./demo.sh up         # build + start, print the URL
./demo.sh open       # up, then open the browser
./demo.sh status     # container status
./demo.sh logs       # follow all logs  (./demo.sh logs synth-fleet for one)
./demo.sh rebuild    # force a clean image rebuild
./demo.sh down       # stop + remove containers
```

## How the data works

The hosted/nova-a deployment runs **one container per notional peer** (22 of
them). That's wasteful on a laptop, so the demo runs a **single**
`synth-vendor` container in **fleet mode** (`--roster`): one process opens one
Zenoh session per peer, each with a distinct ZID. Because panoscope keys peers
by ZID, those internal sessions render as distinct nodes — the whole ecosystem
from one container.

The peer list is [`../zenoh-trial-peers/synth-vendor/demo-roster.json`](../zenoh-trial-peers/synth-vendor/demo-roster.json),
baked into the image. Edit it to change the local demo’s synthetic peers.

### Customizing the data

Edit `demo-roster.json` (add/remove peers, change `personality`, `topic`,
`rate_hz`, `role_class`, `subscribe_key`, `no_publish`) and `./demo.sh
rebuild`. Personalities: `tracks-demo`, `power-demo`, `beacon-demo`,
`garage-temp`, `garage-door`, `yard-soil`, `observer`. Set `"no_publish": true`
for a subscriber-only sink.

## Troubleshooting

| Symptom | Fix |
|---|---|
| `Docker daemon not reachable` | Start Docker Desktop, then `./demo.sh up`. |
| UI loads but graph is empty | Wait ~30 s; slow peers + schema-decls populate last. Check `./demo.sh logs synth-fleet` for `fleet up … started=22`. |
| Port 8080 in use | Stop the other process, or change the published port in `docker-compose.dev.yml` (`panoscope.ports`). |
| First build feels stuck | It's compiling Rust from source (~15-20 min cold). `./demo.sh logs` shows progress. |
| Edited the roster, no change | `./demo.sh rebuild` — the roster is baked into the image at build time. |

This is dev-only: the router has no auth/TLS and the data is synthetic. It is
**not** a deployment path. For deployment, use the
[Ansible role](../ansible/roles/panoscope/) and
[operator runbook](../../docs/operations/panoscope-runbook.md).
