# dataflows — Data Flow Patterns deck runbook

`dataflows.nova.netbird.nova.example` — static presentation container
(`ghcr.io/usareur-af/dataflows`) served on the nova fabric behind
Traefik + Let's Encrypt + mesh-only allowlist gating. Sibling-of-brief
(same shape, separate content + image + Traefik dynamic-config
filename). Source deck at [`ops/serve/dataflows/`](../../ops/serve/dataflows/);
deploy plumbing at [`ops/ansible/roles/dataflows/`](../../ops/ansible/roles/dataflows/)
+ [`ops/ansible/dataflows-deploy.yml`](../../ops/ansible/dataflows-deploy.yml).

## What it is

A 10-slide deck explaining the four families of system-to-system data
movement — internal-only, point-to-point ICDs, request/response APIs,
event-driven pub/sub — and where GOAT-Net sits. One interactive iframe
on slide 7 contrasts N×(N−1) bilateral wires against pub/sub fan-out as
the consumer count grows. Audience parity with the GOAT-Net briefing:
vendors, partners, leadership, integration engineers landing on the
fabric for the first time.

## Pre-flight

1. Image present at `ghcr.io/usareur-af/dataflows:latest` — published by
   `.github/workflows/dataflows-image.yml`
   on `main` pushes touching `ops/serve/dataflows/**`. Manual trigger:
   `workflow_dispatch` from the Actions tab.
2. `dataflows_enabled: true` in `ops/ansible/group_vars/nova_hosts/vars.yml`
   — gates both the SAN entry and the deploy playbook.
3. Mesh DNS entry present in
   `ops/dns/nova/mesh-hosts`
   and re-rolled by `ops/ansible/mesh-coredns.yml`
   (post-PR-#81 posture: service hostnames live in mesh CoreDNS, not
   public Cloudflare).
4. `dataflows.nova.<zone>` is in the master Let's Encrypt SAN list — re-run
   [`ops/ansible/traefik-acme.yml`](https://github.com/usareur-af/goatcore/blob/main/ops/ansible/traefik-acme.yml)
   after flipping `dataflows_enabled: true` so the cert is minted.
5. The GHCR package is currently public. If it gets flipped to private
   in the future, follow the same `<role>_pull_auth_enabled` pattern
   the `brief` role uses (see [`docs/design/ghcr-private-pull.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/ghcr-private-pull.md)).

## Deploy

```bash
cd ops/ansible
OBJC_DISABLE_INITIALIZE_FORK_SAFETY=YES \
  ansible-playbook -i inventory/hosts.ini -i inventory/mgmt-hosts-mesh.ini \
  -l <mgmt-host> dataflows-deploy.yml
```

Idempotent — re-running on a healthy host is a no-op beyond a `docker
compose pull` when the image tag changed.

## Verify

From a netbird-enrolled peer:

```bash
curl -sI https://dataflows.nova.netbird.nova.example/ \
  | head -1                               # HTTP/2 200
```

From off-mesh: 403 (mesh-only allowlist middleware refusing). That's
the whole auth surface — no application-layer auth on the static deck.

Local smoke test before pushing image:

```bash
docker compose -f ops/serve/dataflows/docker-compose.yml build
docker compose -f ops/serve/dataflows/docker-compose.yml up -d
open http://localhost:8089
docker compose -f ops/serve/dataflows/docker-compose.yml down
```

## Rollback

`dataflows_enabled: false` in `group_vars/nova_hosts/vars.yml` →
`traefik-acme.yml` removes the SAN; `dataflows-deploy.yml`
is a no-op. Container survives until manual `docker compose down` in
`/opt/dataflows/`. Single-host blast radius; image pull is the
significant network-side action.

## Cross-refs

- `ops/serve/brief/` — sibling pattern (GOAT-Net briefing)
- `docs/design/nova-goatnet.md` — host topology
- `docs/design/nova-public-posture.md` — mesh-only gating contract
