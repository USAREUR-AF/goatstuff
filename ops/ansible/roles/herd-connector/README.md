# herd-connector

Deploys the herd gear connector on a `NETWORK-JUMP` peer. It dials the herd
console (role `herd`) and carries SSH sessions to network gear at its site
(ADR 1131). It must not run on a mgmt host.

## The image is not public

The herd-connector image is built from a repository that is not published, and
no public image of it exists. This role ships with
`herd_connector_image: "PENDING"` and refuses to deploy until you set
`herd_connector_image` to an image you supply, pinned to its multi-arch index
digest:

```yaml
herd_connector_image: "<registry>/herd-connector:<tag>@sha256:<index-digest>"
```

## Use

- Default off: `herd_connector_enabled: false`.
- Play: `ops/ansible/herd-connector-deploy.yml`.
- Bring-up: `docs/operations/herd-gear-console-bringup.md`.
