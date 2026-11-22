# goat-feeds

Self-service, approved delivery of fabric topics to systems on the mesh. Decision
[ADR 1259](https://github.com/USAREUR-AF/goatcore/blob/main/docs/adr/1259-fabric-feeds-self-service-approved-delivery-of-fabric-topics-to-mesh-systems.md),
design `docs/design/fabric-feeds.md` in goatcore. **Status: WIP**, on trial.

A person picks a topic from the site's catalog, the receiving system, the port, transport
and output format. A tactical admin for the site or a platform admin switches it on. One
`goat-xf-pipe-zenoh` process per topic and output form sends to every feed that shares it.

| File | What it is |
|---|---|
| `main.go` | entry: config, Postgres, supervisor, HTTP server |
| `config.go` | the site config (JSON, rendered by the role) and its catalog |
| `store.go` | registry and audit log in Postgres (`pgx`) |
| `policy.go` | roles from the sign-in fence, who may do what, request validation, first state |
| `supervisor.go` | delivery: groups feeds by topic and output form, runs and restarts the pipes, keeps counters and output |
| `peers.go` | mesh peers from the NetBird API for the destination picker |
| `server.go` | the API, `/healthz`, `/metrics`, the embedded page |
| `web/` | the page: plain HTML, CSS and JavaScript, no framework, no build step |

```
go test ./...                       # policy, validation, grouping, restarts, health, expiry
CGO_ENABLED=0 GOOS=linux GOARCH=arm64 go build -trimpath -o goat-feeds .
```

Deploy: `ops/ansible/goat-feeds-deploy.yml` (roles `goat-feeds`, `goat-feeds-oidc-client`).
The service listens on loopback; a sign-in fence (oauth2-proxy) on the host's mesh address
in front of it sets `X-Forwarded-Email` and `X-Forwarded-Groups`.
