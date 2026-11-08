# trust-sandbox — hosted deployment (ZAGROS-mesh, Ory-gated)

Two **mesh-only, Ory-gated** instances of the trust-sandbox on the ZAGROS mesh, on a
small **cloud** VM (see
[`trust-sandbox.md` §Delivery](../../../../docs/design/trust-sandbox.md)).

- **A — shared:** `trust-sandbox.zagros.netbird.zagros.example` (one stack, many users).
- **B — scratch:** `trust-sandbox-b.zagros.netbird.zagros.example` (a second independent
  stack; the seed for the per-session-scoped mode — until the orchestrator is made
  session-scoped, B is simply a second isolated instance).

The VM self-hosts its gate (Caddy TLS + oauth2-proxy → ZAGROS Ory), so **ZAGROS is
touched only additively**: DNS (`dns_extra_hosts`), an Ory client per instance, and
an isitup row per instance. **No** mgmt-host Traefik edits, and **never** the
`site-bootstrap` / `zagros-bootstrap` playbooks.

All four images the hosted instance runs — `trust-sandbox-{web,orchestrator,spire}`
and the `caddy-cloudflare` TLS edge — are built by CI
(`.github/workflows/trust-sandbox-image.yml`, `:latest` + `:sha-<short>` multi-arch)
and **pinned by immutable digest** in `docker-compose.hosted.yml`
(`:sha-<short>@sha256:<index-digest>`, the fleet convention — git-tracked + PR-bumped,
multi-arch index digest). The local `docker compose up` path is unaffected: the base
`docker-compose.yml` *builds* the images and tags them `:dev` — `:dev` is the
local-build tag only, never a deploy pin.

**Bumping the hosted pins:** merge the image change → CI publishes `:sha-<merge>` for
all four → update the four `@sha256` digests in `docker-compose.hosted.yml` (a PR) →
`docker compose … pull && up -d` on the VM. Get a digest with
`docker buildx imagetools inspect ghcr.io/usareur-af/<pkg>:sha-<merge> --format '{{.Manifest.Digest}}'`.

## 1. Provision the VM

```bash
cd ops/serve/trust-sandbox/deploy/terraform
aws-vault exec legacy --no-session -- terraform init
aws-vault exec legacy --no-session -- terraform apply \
  -var aws_account_id="123456789013" \
  -var operator_cidr="$(curl -s ifconfig.me)/32" \
  -var public_key="$(cat ~/.ssh/id_ed25519.pub)"
# → outputs: public_ip, ssh
```

`aws_account_id` has no default: it feeds the provider's `allowed_account_ids`
fuse, which refuses to apply into whatever account the ambient profile happens to
point at.

## 2. Enroll the VM in the ZAGROS mesh

Mint a one-time setup key on the mgmt host (additive — does not disturb peers):

```bash
# on the mgmt host:
sudo netbird-mgmt create-setup-key --name trust-sandbox --type one-off
# then on the VM:
ssh ubuntu@<public_ip> \
  "sudo netbird up --management-url https://netbird.zagros.example --setup-key <KEY>"
ssh ubuntu@<public_ip> "ip -4 addr show wt0"   # → the VM's mesh IP (100.64.x.y)
```

## 3. Create an Ory client per instance (ZAGROS Hydra admin :4445)

```bash
# reachable over the mesh once the VM is enrolled; ADMINS group gates access
hydra create client --endpoint https://oauth2.zagros.netbird.zagros.example \
  --grant-type authorization_code,refresh_token --response-type code \
  --scope openid,groups --token-endpoint-auth-method client_secret_post \
  --skip-consent \
  --redirect-uri https://trust-sandbox.zagros.netbird.zagros.example/oauth2/callback
# repeat for trust-sandbox-b.* → record client_id + client_secret for each
```

`--skip-consent` is not optional. The gate is a first-party door, and every other
first-party client on a goatnet is written by a role that sets `skip_consent: true`,
so nobody is asked to "Allow" a service the goatnet itself runs. This client is the
one made by hand, so this command must carry the flag or it alone shows the consent
page. That is safe for this gate — oauth2-proxy reads `sub` and `groups`
from the id_token, so the `/userinfo` claim-stripping of F-163 does not reach it.

A client that already exists is brought in line by a play, not by hand. It finds the
client by its callback URL (the id is whatever Hydra generated), changes that one
field by JSON Patch so the secret on the VM stays valid, and reads it back:

```bash
cd ops/ansible
../scripts/ansible-preflight.sh -i inventory/mgmt-hosts-mesh.ini \
  trust-sandbox-oidc-client.yml -e target=zagros_hosts
```

Re-running it is a no-op. It fails, saying so, on a goatnet with no sandbox client.

## 4. Deploy the two instances on the VM

Copy `ops/serve/trust-sandbox/` to the VM, then per instance set the env + bring up
the base compose + the hosted overlay:

All four images, the three `trust-sandbox-*` images and the `caddy-cloudflare`
edge, are public and pull without a login. To build the edge yourself instead,
use the public `deploy/Dockerfile.caddy` and point the overlay's `edge` service at
the local tag:

```bash
docker build -f deploy/Dockerfile.caddy -t caddy-cloudflare:local .
# then in deploy/docker-compose.hosted.yml: edge.image: caddy-cloudflare:local
```

```bash
# instance A  (TS_ZAGROS_OIDC_ISSUER must match the discovery doc EXACTLY, incl. trailing slash)
export TS_PUBLIC_HOST=trust-sandbox.zagros.netbird.zagros.example
export TS_PUBLIC_ORIGIN=https://trust-sandbox.zagros.netbird.zagros.example   # used by the in-sandbox teaching IdP
export TS_ZAGROS_OIDC_ISSUER=https://oauth2.zagros.netbird.zagros.example/
export TS_GATE_CLIENT_ID=...  TS_GATE_CLIENT_SECRET=...
export TS_GATE_COOKIE_SECRET="$(openssl rand -hex 16)"   # MUST be 16/24/32 bytes — `-hex 16` = 32 bytes (NOT `-base64 32`, which is 44)
# export TS_GATE_ALLOWED_GROUPS=ADMINS                   # optional; empty (default) = any authenticated mesh peer
export CLOUDFLARE_API_TOKEN_ZAGROS=...
docker compose -p trust-sandbox-a -f docker-compose.yml -f deploy/docker-compose.hosted.yml up -d
```

`deploy/edge.env` (optional, not shipped) carries a deployment's own values: `TS_ACME_EMAIL`
for the edge, and `TS_GOATNET`, the goatnet the sandbox models. The orchestrator stamps
`TS_GOATNET` into the URI-SAN of the leaves it mints, and the web container rewrites the
pages to name it. Unset, both use the example goatnet.

A second instance on the **same VM** must SHARE the edge (one host = one :443): bring
it up without its own edge (`up -d certs-init orchestrator web spire-server spire-agent
zenohd hydra hydra-init coredns target gate`), connect the edge to its network, and
add a second site block to the Caddyfile. One shared instance is the default shape.

> The Caddy edge needs the `caddy-dns/cloudflare` module (stock image lacks it) —
> published at `ghcr.io/usareur-af/caddy-cloudflare` by `trust-sandbox-image.yml`
> (built from `Dockerfile.caddy`, Caddy 2.10 + xcaddy; 2.8 hits a removed `zapslog`
> API).

## 5. Register DNS + isitup (additive, surgical)

`ops/ansible/group_vars/zagros_hosts/vars.yml`:

```yaml
dns_extra_hosts:
  - { name: "trust-sandbox.{{ site_id }}.{{ site_dns_zone }}",   ip: "<VM mesh IP>" }
  - { name: "trust-sandbox-b.{{ site_id }}.{{ site_dns_zone }}", ip: "<VM mesh IP>" }
isitup_services:
  - { id: trust-sandbox,   name: "Zero-Trust Sandbox (shared)",  fqdn: "trust-sandbox.{{ site_id }}.{{ site_dns_zone }}",   type: external, health_path: /, kind: ui, access: admins, category: platform }
  - { id: trust-sandbox-b, name: "Zero-Trust Sandbox (scratch)", fqdn: "trust-sandbox-b.{{ site_id }}.{{ site_dns_zone }}", type: external, health_path: /, kind: ui, access: admins, category: platform }
```

Apply ONLY the incremental playbooks (safe per the ZAGROS deploy map — never the
bootstrap ones):

```bash
cd ops/ansible
python3 ../dns/generate-mesh-hosts.py
OBJC_DISABLE_INITIALIZE_FORK_SAFETY=YES aws-vault exec legacy --no-session -- \
  ../scripts/ansible-preflight.sh -i inventory/mgmt-hosts-mesh.ini -l mgmt-zagros-01-thor goat-coredns.yml
OBJC_DISABLE_INITIALIZE_FORK_SAFETY=YES aws-vault exec legacy --no-session -- \
  ../scripts/ansible-preflight.sh -i inventory/mgmt-hosts-mesh.ini -l mgmt-zagros-01-thor isitup-deploy.yml
```

## 6. End-to-end test (each instance)

From a mesh-connected browser/host:
- `https://trust-sandbox.zagros.…/` → redirected to ZAGROS Ory; sign in with an ADMINS
  user → the playground loads (non-ADMINS are refused at the gate).
- Run an attempt (acme-sensors fresh → lands; wrong-lane → silent deny; forged →
  refused), the SPIRE workload path (rotating SVID), the Ory in-sandbox login →
  mint/rotate/revoke, burn-&-respawn, and the concept drill-downs.
- Confirm both rows show green on `https://isitup.zagros.…`.
