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

| File | What it is |
|---|---|
| `main.go` | entry: config, Postgres, supervisor, HTTP server |
| `config.go` | the site config (JSON, rendered by the role) and its catalog |
| `store.go` | registry and audit log in Postgres (`pgx`) |
| `policy.go` | roles from the sign-in fence, who may do what, request validation, first state |
| `supervisor.go` | delivery: groups feeds by topic and output form, runs and restarts the pipes, keeps counters and output |
| `signals.go` | the two facts per feed: Integrated, and Sending or Receiving, each yes or no with its reason |
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
./build.sh 0.2.0                    # dist/goat-feeds-0.2.0-linux-{amd64,arm64} + SHA256SUMS
```

Deploy: `ops/ansible/goat-feeds-deploy.yml` (roles `goat-feeds`, `goat-feeds-oidc-client`).
The service listens on loopback; a sign-in fence (oauth2-proxy) on the host's mesh address
in front of it sets `X-Forwarded-Email` and `X-Forwarded-Groups`.
