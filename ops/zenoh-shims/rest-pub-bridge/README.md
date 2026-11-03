# rest-pub-bridge

A small Rust sidecar that accepts HTTP-PUT publishes and re-emits them on a
**native mTLS Zenoh session**. It is a drop-in replacement for the Zenoh
router's REST plugin PUT surface — but unlike the router's plugin, it
presents a per-vendor X.509 leaf, so each publish carries a transport-level
identity the router's `access_control` can scope on.

Design + rationale: [`docs/design/zenoh-rest-pub-bridge.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/zenoh-rest-pub-bridge.md) · [ADR 1023](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1023-zenoh-rest-pub-bridge-for-non-rust-publishers.md).

## Why

Every native Zenoh peer in this repo opens a TLS session and presents a leaf
whose Common Name is the ACL identity (`CN=<vendor>-trial-peer` →
allow `put` on `<vendor>/**`). A publisher that can't open a native session
has no wire identity. The motivating case is the **demogame** game
server: it's Go, and at the time (early 2026) there was no maintained Go
client for Zenoh 1.x (an official cgo `zenoh-go` later shipped in 1.9.0; see
docs/design/zenoh-rest-pub-bridge.md), so it
publishes via plain HTTP PUT to the router's REST plugin — a surface with no
client-cert requirement. After the ADR 1020 ingress/egress ACL split the
router grants publish only to identified per-vendor subjects, so those
identity-less REST PUTs are denied and the data silently stops flowing.

This bridge restores the identity without touching the Go publisher: the
publisher points `ZENOH_REST_ENDPOINT` at the bridge instead of the router,
and the bridge re-publishes over mTLS using the vendor leaf it mounts.
Reusable by any non-Rust publisher in the same bind.

## Wire contract (mirrors the Zenoh REST plugin)

```
PUT  /<keyexpr>     body = value bytes     → publish to <keyexpr>
POST /<keyexpr>     (accepted for parity)
GET  /health        → 200 "ok"             (liveness for smoketests)
```

The URL path (minus the leading `/`) is the key-expression; the body is the
value; the request `Content-Type`, when present, becomes the Zenoh encoding.

## Configuration (CLI flag / env var)

| Flag | Env | Default | Notes |
|---|---|---|---|
| `--listen` | `REST_PUB_BRIDGE_LISTEN` | `0.0.0.0:8000` | HTTP listener |
| `--endpoint` | `REST_PUB_BRIDGE_ENDPOINT` | `tls/goat-zenoh-router:7447` | `tls/` = native mTLS |
| `--cert` | `REST_PUB_BRIDGE_CERT` | — | client leaf PEM (required for `tls/`) |
| `--key` | `REST_PUB_BRIDGE_KEY` | — | client key PEM (required for `tls/`) |
| `--root-ca` | `REST_PUB_BRIDGE_ROOT_CA` | — | trust root PEM (required for `tls/`) |
| `--allow-prefix` | `REST_PUB_BRIDGE_ALLOW_PREFIX` | `` (any) | comma-separated key-prefix allowlist |

`--allow-prefix` is defense-in-depth: the bridge holds a real vendor cert,
so a compromised upstream must not be able to ask it to publish outside the
vendor namespace. Set it to the vendor prefix (e.g. `demogame/`). The
router ACL already scopes the cert to `<vendor>/**`; this is the belt to
that suspenders.

## Build

```
cargo build --release --locked
```

Image: `ghcr.io/usareur-af/rest-pub-bridge` (built by
`.github/workflows/rest-pub-bridge-image.yml`, multi-arch, Chainguard base).
The Ansible role mounts the cert tree read-only; the image carries no
identity material.

## Deployment

Deployed as a sidecar in the demogame compose project
(`ops/ansible/templates/demogame-docker-compose.yml.j2`). The vendor
leaf is minted by `ops/zenoh-trial-peers/bootstrap-demogame-portal.sh`
and the matching ACL rule comes from adding the vendor to
`nova_seed_vendors`.
