# openobserve-oidc-client

Provisions an OAuth2 client for OpenObserve at operator-IdP Hydra per [ADR 0842](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0842-dedicated-operator-idp-for-observability.md).

Same shape as [`grafana-oidc-client`](https://github.com/usareur-af/goatcore/tree/main/ops/ansible/roles/grafana-oidc-client) — only the Hydra-side OAuth2 client config; the OpenObserve env vars live in [`openobserve`](../openobserve/) role's compose template (extended in this batch).

## Required env var

```
export <SITE>_C_OPERATOR_IDP_OPENOBSERVE_CLIENT_SECRET=$(openssl rand -hex 32)
```

## Upstream-uncertainty note

OpenObserve's external-OIDC env-var naming has changed across minor versions. The compose template uses `ZO_OPENID_*` (documented for v0.10+). If first bringup smoke fails with "OIDC not configured" or similar, surface as a finding + update both the openobserve role's compose template AND this role's redirect URI / scope assumptions together.

## Group → OpenObserve role mapping

OpenObserve's role model is coarser than Grafana's: it has organizations + roles. v1 mapping for operator IdP groups:

| Operator groups | OpenObserve role |
|---|---|
| `obs-admin` OR `openobserve-admin` | admin |
| `obs-readonly` OR `openobserve-readonly` | viewer |
| (no matching group) | login fails |

Mapping enforced via OpenObserve's `ZO_DEFAULT_USER_ROLE` env + a `role_claim` mapping (verify against upstream at first bringup).

## Tests

`tests/syntax-check.sh` — Ansible parse-only smoke + register-uniqueness lint.
