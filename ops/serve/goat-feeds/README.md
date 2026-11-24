# goat-feeds

Self-service, approved delivery of fabric topics to systems on the mesh. Decision
[ADR 1259](https://github.com/USAREUR-AF/goatcore/blob/main/docs/adr/1259-fabric-feeds-self-service-approved-delivery-of-fabric-topics-to-mesh-systems.md),
design `docs/design/fabric-feeds.md` in goatcore. **Status: WIP**, on trial.

Two directions:

- **OUT (send a topic):** a person picks a topic from the site's catalog, the receiving
  system, the port, transport and output format. One `goat-xf-pipe-zenoh` process per topic
  and output form sends to every feed that shares it.
- **IN (receive a system's data):** a person names what a system sends, its standard
  (MAVLink 2, ASTERIX, CoT, SAPIENT, …) and picks a free receive port on the core node. One
  process per received feed listens on `udp://<core mesh IP>:<port>`, converts each message to
  the site's canonical form (`goat-processors-system-track`) and publishes it on
  `<key_prefix>/<feed id>`. While it is live it appears in the catalog as a topic, so anyone
  can request it out as CoT, CAT062 or SAPIENT.

A tactical admin for the site or a platform admin switches either kind on; anyone else's
request waits for one of them. The receive ports must be reachable from the sending systems
on the mesh (a warrant rule).

## Systems: naming, placing, archiving (0.4.0)

Once an estate is declared, every link's receiving (or sending) system must be a declared
system, found by mesh peer name, system name or mesh address. A request to anything else
does not take a bare name and address: the form opens **Declare this system** (site, kind,
function, display label, machine name). An admin's declaration is placed at once; a
member's is sent with the request and waits in **Tidy up** as a proposal. The API applies
the same rule and answers 400 with what is wrong and how to fix it.

**Naming rule** (one Go function, `naming.go`, table-tested): `<kind>-<site>-<function>-<nn>`,
lower case letters, digits and hyphens; the site token must be a declared site id; `nn` is
two or three digits. `GET /api/naming` publishes the rule, the sites, the known kinds and
functions and an example; `?name=&site=` checks a name, `?kind=&site=&function=` suggests
the first free one.

**Boundary nodes** (`class: "boundary"`) carry `far_side: {label, direction}`. Everywhere a
link goes through one, the page says "To <far side> through boundary node <name>" (or
"From …" for a received feed). One without a far side raises the attention reason
"boundary node: where the data goes is not declared" and is listed in Tidy up.

**Lifecycle.** A system has `status` `active` (default) or `archived`, and an archived one
an `archive` record: `at`, `by`, `operation`, `returns` (`same-system-same-site` |
`same-system-new-site` | `new-system` | `unknown`), `rejoin` (`peer-kept` | `rejoin-needed` |
`unknown`), `notes`, and `restore` (the link ids to switch back on). An archived system
never raises attention; its links read **archived**, are counted apart, have their own
filter chip and a collapsed group of their own.

**Tidy up** (tactical and platform admins; the nav shows a count) lists, each with its
reason: links whose system is not in the estate, tactical mesh peers the estate does not
list, names that break the rule, boundary nodes without a far side, archived systems with
links still on, and members' proposals. Actions: **Place** (site, kind, function, label,
new machine name, far side), **Archive** (how it comes back, whether it rejoins, the
operation, notes, which links to switch back on, pause its requested feeds now), **Bring
back** (unarchive, resume the paused feeds on its restore list, and name the file that
controls each declared link, which is read-only here). Placements are an overlay in
Postgres (`placements`), every write in the audit log as `system:<name>`; the effective
estate is the estate file plus the overlay, and each changed system says who placed it.
`GET /api/estate/effective` returns it in the estate file's own shape (unread fields kept),
to promote into the repository's estate file by pull request; then dismiss the overlay
entries.

| Route | What it does |
|---|---|
| `GET /api/naming` | the naming rule, sites, kinds, functions; check or suggest a name |
| `GET /api/systems/resolve?name=&ip=` | is this destination a declared system (or proposed) |
| `GET /api/tidy` | the Tidy up list, declared systems with their links, archived systems (admins) |
| `POST /api/systems/{name}/place` | place a system (new: name is the machine name) (admins) |
| `POST /api/systems/{name}/archive` | archive with `returns`, `rejoin`, `operation`, `notes`, `restore`, `pause` (admins) |
| `POST /api/systems/{name}/restore` | bring back; `resume` (default true) resumes its paused feeds (admins) |
| `POST /api/systems/{name}/dismiss` | drop a proposal, or an overlay entry once promoted (`reason` needed) (admins) |
| `GET /api/estate/effective` | the estate file with the overlay applied (admins) |

Settings: `roles` (functions offered besides those in use) and `tactical_peer_groups` (mesh
groups whose peers the estate must list; empty: any peer a role group places). goat-map reads
the same `status`: an archived system is not expected for readiness and reads `archived`.

| File | What it is |
|---|---|
| `main.go` | entry: config, Postgres, supervisor, HTTP server |
| `config.go` | the site config (JSON, rendered by the role) and its catalog |
| `store.go` | registry and audit log in Postgres (`pgx`) |
| `policy.go` | roles from the sign-in fence, who may do what, request validation, first state |
| `supervisor.go` | delivery: groups feeds by topic and output form, runs and restarts the pipes, keeps counters and output |
| `signals.go` | the two facts per feed: Integrated, and Sending or Receiving, each yes or no with its reason |
| `facets.go` | facets from the estate (region, site, system, receiving role), needs-attention rules, filters and counts (fabric-feeds.md §11); system lifecycle and boundary far side |
| `naming.go` | the naming rule for machine names: check, suggest |
| `placements.go` | the placement overlay in Postgres, the effective estate and its export |
| `tidy.go` | the declare rule for requests, the Tidy up list and the place, archive, bring back actions |
| `declared.go` | links started by deployment files (ADR 1260): reads their manifests and polls each unit every 10 s, shown read-only beside requested feeds |
| `peers.go` | mesh peers from the NetBird API for the destination picker |
| `server.go` | the API, `/healthz`, `/metrics`, the embedded page |
| `web/` | the page: plain HTML, CSS and JavaScript, no framework, no build step |

Tests, in three layers (all run in CI, `.github/workflows/goat-feeds.yml`):

| Layer | What it covers | Run |
|---|---|---|
| Unit | config, permissions for every action, role and state, validation, health, pipe commands, supervisor with a fake pipe | `go test ./...` |
| Integration | the Postgres store (including migration from the first schema) and the HTTP API over it | `GOAT_FEEDS_TEST_DSN=postgres://… go test -p 1 -run 'TestPG\|TestAPI' ./...` |
| Browser (e2e) | the real binary and page in Chromium: send and receive flows, approvals, actions, audit; no console errors, no missing-value text, no overflow at 390 and 1440 px, light and dark, keyboard reach | `go build -o goat-feeds . && cd e2e && npm ci && npx playwright install chromium && GOAT_FEEDS_E2E_DSN=postgres://… npx playwright test` |

The integration and browser tests wipe the database they are given.

```
go test ./...                       # unit layer
./build.sh 0.4.0                    # dist/goat-feeds-0.4.0-linux-{amd64,arm64} + SHA256SUMS
```

Deploy: `ops/ansible/goat-feeds-deploy.yml` (roles `goat-feeds`, `goat-feeds-oidc-client`).
The service listens on loopback; a sign-in fence (oauth2-proxy) on the host's mesh address
in front of it sets `X-Forwarded-Email` and `X-Forwarded-Groups`.
