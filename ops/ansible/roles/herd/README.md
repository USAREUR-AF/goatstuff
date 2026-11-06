# herd

Deploys the herd bearer-gear console on a mgmt host, behind the goat
oauth2-proxy fence and Traefik (ADR 1131). The SSH egress is not deployed here;
it belongs to a separate `NETWORK-JUMP` peer (role `herd-connector`).

## The image is not public

The herd image is built from a repository that is not published, and no public
image of it exists. This role ships with `herd_image: "PENDING"` and refuses to
deploy until you set `herd_image` to an image you supply, pinned to its
multi-arch index digest:

```yaml
herd_image: "<registry>/herd:<tag>@sha256:<index-digest>"
```

## Use

- Default off: `herd_enabled: false`.
- Play: `ops/ansible/herd-deploy.yml` (runs `herd-oidc-client` first, then this role).
- Bring-up: `docs/operations/herd-gear-console-bringup.md`.
