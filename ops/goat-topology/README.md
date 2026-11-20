# goat-topology

A routing-topology page for a Zenoh router fabric. It draws every router, the
links between them and the region each link belongs to, counts what each router
carries (subscriptions, publishers, key expressions, route-table entries), and
lists findings: configurations that do not do what they appear to, routers that
have gone quiet, and routers that disagree with each other.

It runs against stock `zenohd` with the REST plugin. Nothing else is needed:
no goat component, no change to the routers beyond enabling the REST plugin.

![The page's routing graph: six routers that report drawn as full circles, fourteen routers known only from their neighbors' link-state graphs drawn as dashed circles, blue links inside one region and amber links from a gateway router to the routers south of it.](docs/graph-dark.png)

**Status.** Built and in use. Tested with zenoh **1.9.0 only** (see
[Versions](#versions)). Read [Known issues](#known-issues) before running it
on routers you care about. The design document is
[`docs/design/zenoh-topology-surface.md`](../../docs/design/zenoh-topology-surface.md).

## How it works

One binary, two modes:

```text
  router host                                  any host
  ┌──────────────────────────────┐            ┌──────────────────────┐
  │ zenohd ── REST plugin        │            │ goat-topology serve  │──► browser
  │   │       127.0.0.1:8000     │            │  subscribes to       │
  │   │            ▲             │            │  <prefix>/**         │
  │   │      goat-topology       │  one put   │  holds the latest    │
  │   │      publish ────────────┼──per──────►│  snapshot per router │
  │   │      (reads this router) │  interval  └──────────────────────┘
  └───┼──────────────────────────┘
      └── the Zenoh fabric
```

- **`publish`** runs next to each router. Every 30 s it reads that router's own
  adminspace through the REST plugin on loopback (`@/local/router`, the
  link-state graphs, the route table and the version) and the `gateway` block
  of the router's config file. It puts one small protobuf message,
  `RouterTopologyV1`, on `<prefix>/<site>/<node>` (default prefix
  `ops/fabric/topology`). Every 300 s it also counts the router's subscriber
  and publisher tables.
- **`serve`** runs once. It subscribes to `<prefix>/**`, keeps the latest
  message from each router in memory, and serves the page and its JSON
  (`/api/topology`).

**A browser request never reaches the fabric.** The page reads only what the
publishers already put on the bus; opening it, or refreshing it every second,
sends nothing to any router. The only fabric traffic this tool makes is one
message per router per interval. Each process opens one client session to the
router named in `--endpoint`, with multicast scouting off.

**Routers with no publisher still appear.** The routers that do report list
their neighbors in their link-state graphs, so a router with no publisher
(another team's, or one you have not deployed to yet) is drawn from those
graphs and marked "not reporting". Its links are real; its settings and load
are shown as unknown, never as zero.

Every read is of `@/local/...`, never `@/**`. An adminspace query is routed,
so `@/**` would return every reachable router's adminspace and turn a
per-router read into a fabric-wide one.

## Quick start: three routers in Docker

Needs Docker with Compose, `curl` and `python3`.

```bash
git clone https://github.com/usareur-af/goatstuff
cd goatstuff/ops/goat-topology/example
docker compose up -d
./check.sh                 # PASS: routers=3 reporting=3 links=3 ...
```

Then open <http://127.0.0.1:8080/>. Stop it with `docker compose down`.

The example ([`example/compose.yaml`](example/compose.yaml)) runs three
`eclipse/zenoh:1.9.0` routers (pinned by digest) on plain TCP, one publisher
per router, and one `serve`. `check.sh` waits until the page shows the three
routers, all reporting, with three links and no warning, and fails otherwise.
CI runs the same example against the code in this directory, so these commands
are tested on every change.

Set `TOPOLOGY_PORT` to serve on a port other than 8080, and
`GOAT_TOPOLOGY_IMAGE` to use an image you built yourself.

## Running it on your own routers

### 1. Each router: enable the REST plugin on loopback

In the router's config file:

```json5
plugins_loading: { enabled: true },
plugins: { rest: { http_port: "127.0.0.1:8000" } },
adminspace: { enabled: true, permissions: { read: true, write: false } },
```

Bind the REST plugin to loopback. It answers anyone who can reach it, it can
also put and get data on the fabric, and the publisher on the same host is
the only thing that needs it.

### 2. Get the binary

Any one of:

```bash
# The published image (linux/amd64 and linux/arm64), pullable without login:
docker pull ghcr.io/usareur-af/goat-topology:latest

# From source, Rust 1.88 or newer:
cargo install --locked --git https://github.com/usareur-af/goatstuff goat-topology

# Or build this directory:
cargo build --release --locked      # binary at target/release/goat-topology
```

For production, pin the image by digest rather than `:latest`.

### 3. Each router host: run `publish`

```bash
goat-topology publish \
  --endpoint tcp/127.0.0.1:7447 \
  --site lab --node router-1 \
  --admin-addr 127.0.0.1:8000 \
  --config-path /etc/zenoh/router.json5
```

- `--endpoint` is the router it publishes through, normally its own. A
  comma-separated list is tried in order (see finding T-013 for when that is
  needed).
- `--site` and `--node` name the router on the page and in its key. `--node`
  must be unique within the site. Neither may contain `/ * $ # ?`.
- `--config-path` is the router's config file. Only its `gateway` block is
  read; nothing else in it (access control included) is parsed, logged or
  published. Without it, the page shows the router's gateway setting as
  unreadable rather than guessing.
- In Docker, run the publisher with `--network host` (or in the router's
  network namespace) so that it can reach the REST plugin on loopback, and
  mount the config file read-only.

### 4. Once: run `serve`

```bash
goat-topology serve --endpoint tcp/router-1.example.org:7447 --bind 127.0.0.1:8080
```

The page has **no login of its own**. Bind it to loopback, as above, and put
it behind a reverse proxy that authenticates, or keep it on a network only
operators reach. It works at the root of a host name or under a sub-path
(`https://example.org/topology/`); under a sub-path, have the proxy redirect
`/topology` to `/topology/`. Times on the page are UTC.

### Access control and TLS

- **Access control.** With `access_control` on, the publisher needs `put` on
  `<prefix>/**` and the surface needs `declare_subscriber` on `<prefix>/**`.
  A denied put fails silently in Zenoh: the publisher logs nothing and the page
  stays empty, so check this first if no router appears.
- **TLS.** For a `tls/` or `quic/` endpoint, both modes need `--root-ca`, the
  CA the router's certificate is checked against. Add `--cert` and `--key` when
  the routers require a client certificate (mutual TLS); leave both out when
  they do not. `--tls-verify-name` also checks the router's certificate against
  the host name in `--endpoint`.

## Settings

Every flag can also be set by the environment variable shown. Run
`goat-topology <mode> --help` for the full list.

| Flag | Environment | Default | Mode | What |
|---|---|---|---|---|
| `--endpoint` | `TOPOLOGY_ENDPOINT` | (required) | both | Router to connect through. `publish` takes a comma-separated list, tried in order. |
| `--key-prefix` | `TOPOLOGY_KEY_PREFIX` | `ops/fabric/topology` | both | Key prefix. Must be the same on every publisher and the surface. |
| `--site`, `--node` | `TOPOLOGY_SITE`, `TOPOLOGY_NODE` | (required) | publish | The router's place in the key and its name on the page. |
| `--site` | `TOPOLOGY_SITE` | (all) | serve | Show one site only. |
| `--admin-addr` | `TOPOLOGY_ADMIN_ADDR` | `127.0.0.1:8000` | publish | The router's REST plugin. |
| `--config-path` | `TOPOLOGY_CONFIG_PATH` | (none) | publish | The router's config file, for its `gateway` block. |
| `--interval-secs` | `TOPOLOGY_INTERVAL_SECS` | `30` | publish | How often to publish. The page marks a router stale after three intervals with no report. |
| `--route-table` | `TOPOLOGY_ROUTE_TABLE` | `settled` | publish | When to read the route table: `settled`, `always` or `off`. See [Known issues](#known-issues). |
| `--route-table-settle-ms` | `TOPOLOGY_ROUTE_TABLE_SETTLE_MS` | `1000` | publish | How long the link-state graphs must hold still before `settled` reads the route table. |
| `--declarations-interval-secs` | `TOPOLOGY_DECLARATIONS_INTERVAL_SECS` | `300` | publish | How often to count the subscriber and publisher tables. `0` turns it off. |
| `--declarations-top-keys` | `TOPOLOGY_DECLARATIONS_TOP_KEYS` | `5` | publish | Key expressions named per router (the rest are counted, not listed). |
| `--bind` | `TOPOLOGY_BIND` | `0.0.0.0:8080` | serve | Where the page listens. Prefer `127.0.0.1:<port>`. |
| `--root-ca` | `TOPOLOGY_ROOT_CA` | (none) | both | CA for the router's certificate. Required for a `tls/` or `quic/` endpoint. |
| `--cert`, `--key` | `TOPOLOGY_CERT`, `TOPOLOGY_KEY_FILE` | (none) | both | Client certificate and key, for routers that require mutual TLS. Both or neither. |
| `--tls-verify-name` | `TOPOLOGY_TLS_VERIFY_NAME` | off | both | Also check the router's certificate against the endpoint's host name. |

`goat-topology inspect --admin-addr 127.0.0.1:8000 --config-path <file>`
prints what the publisher would send for this host's router, as JSON, without
connecting to the fabric.

## Known issues

These are in zenoh 1.9.0, and they are triggered by reading a router's
adminspace, which is what this tool does. Read them before running the
publisher on routers you depend on.

- **A route-table read can crash the router.** A query on
  `@/<zid>/router/route/successor/**` that arrives just after a router joins or
  leaves, before the routing trees are recomputed, panics `zenohd`
  (`index out of bounds`) and the process exits:
  [eclipse-zenoh/zenoh#2831](https://github.com/eclipse-zenoh/zenoh/issues/2831).
  In the three-router example, publishers that read the route table every
  cycle (`--route-table always`) crashed a router in 4 of 10 clean starts. The
  default `--route-table settled` reads the table only when the link-state
  graphs have not changed for one second; with it, 0 of 10 clean starts
  crashed a router. Ten runs do not show the risk is gone: `settled` narrows
  the window, it does not close it.
  `--route-table off` never reads it; the page then shows route entries as
  "not read". Use `always` only with a router build that fixes #2831.
- **A router can stop answering at start.** In a 22-router test of stock
  1.9.0, two routers stopped answering their REST plugin right after start
  (no CPU use, nothing logged) while publishers were reading them; restarting
  each one cleared it. We have not tied this to an upstream issue yet. The
  example's routers have a health check that reads the adminspace, so
  `docker compose ps` shows such a router as `unhealthy`. A publisher cannot
  report a router in that state, because it publishes through it, so the page
  shows that router as not reporting.

## Versions

The adminspace layout this reads (`@/<zid>/router`, `linkstate/<region>`,
`route/successor/...`, regions named `north` and `south:<id>:<mode>`) is not a
stable interface, and zenoh **1.9.0** is the only version this has been tested
against. The binary itself uses zenoh `=1.9.0` as its client library.

On a router that reports another version, the publisher still publishes, and
says on the page what it could not read rather than showing zeros.

## Words on the page

The page uses some plain-language words for Zenoh terms:

| On the page | In Zenoh |
|---|---|
| routing group | region: the router's own `north` region, or a `south:<id>:<mode>` subregion |
| routed link | a link between two routers in the same region |
| terminated link | a link from a router to a router in one of its south subregions |
| gateway (router role) | a router whose `gateway.south` is a custom list |
| applications | sessions whose mode is `client` |
| route entries | entries of the router's successor table (`route/successor`) |

## Who runs which router (optional)

`serve` can also show who operates each router, from files on the surface's
host. The reading of those files is built for goat's own deployments today. A
neutral file that anyone can write (match a router by ZID, address or CIDR;
give it a name, an operator and labels) is designed, not built: see §9.1 of the
[design document](../../docs/design/zenoh-topology-surface.md). With
no such file configured, routers are named by their node names, or by a name
derived from their ZID for routers that send no report.

## License

Apache-2.0.
