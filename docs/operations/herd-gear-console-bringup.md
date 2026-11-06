# Herd gear console — operator bring-up runbook

| Field | Value |
|---|---|
| Goal | Bring the bearer-gear console live on ZAGROS: the console on the mgmt host, and the connector on its own `NETWORK-JUMP` peer. |
| Target | **ZAGROS first** (the goatnet's mgmt host). zagros-prod only after ZAGROS is re-probed. |
| Status | ⛔ **Not done.** Everything below is written and unit-tested; **nothing has run against a device.** Both roles are default-OFF. **The herd and herd-connector images are not public:** they are built from a repository that is not published, so both roles ship with `PENDING` image defaults and need operator-supplied images (`herd_image`, `herd_connector_image`). |
| Scope | **A per-environment SERVICE bring-up, not a goatnet standup.** The one holistic path
for standing a goatnet up is [goatnet-standup.md](https://github.com/usareur-af/goatcore/blob/main/docs/operations/goatnet-standup.md) and this does not fork it —
it assumes a goatnet already stood up that way and adds one service onto it (doctrine P1,
[goatnet-deployment-doctrine.md](https://github.com/usareur-af/goatcore/blob/main/docs/design/goatnet-deployment-doctrine.md)). |
| Refs | [ADR 1131](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1131-network-jump-class-and-herd-gear-scope.md) · herd-adoption.md · [ADR 1113](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1113-network-node-class-and-delegated-network-admin.md) (the entitlement) · ADR 0412 (SPIRE) · runner-console-mtls.md |

## Why ZAGROS first, and the four traps

1. **SPIRE is a prerequisite.** The connector authenticates to the console with a SPIFFE SVID, so a goatnet needs SPIRE running before this can go there — a prerequisite, not a reason to skip the environment.
2. **Trap — the connector must NOT land on the mgmt host.** A mgmt host *is* `INFRA`, and `infra → network (any port)` already exists, so a connector there would reach every switch on every port and the ACL would contribute nothing. Groups only ever *union* reach, so no membership fixes it after the fact. The connector goes on its own peer or it is not worth deploying.
3. **Trap — `NETWORK-JUMP` is operator-only, on purpose.** It is deliberately **not** in `delegableInfraNodeClasses`: the entitlement that lets a site admin enrol a switch must not let them enrol the box that reaches every switch. Enrol it as the operator.
4. **Trap — F-330, the bind-mounted key.** The portal service key is a single bind-mounted *file*, so the container is pinned to the inode it was created with. Re-rendering it on the host does **not** change what the process reads, and ansible reports `changed=0` — exactly what a correctly converged host prints. The role checksums the running copy against the host copy and recreates on mismatch; do not "fix" a stale key by re-running the template task alone.

All ansible runs go through the **env door** ([ADR 1117](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1117-per-env-doors-cross-env-management.md), `ops/scripts/env-door.sh`). **Never flip the laptop's netbird profile for a deploy** — the profile is a shared laptop-wide resource.

---

## Pre-flight (live — do not skip)

```bash
# 1. The console's two gates depend on the portal being reachable and on the
#    grant endpoint existing. Both are new; confirm before deploying anything.
curl -s -o /dev/null -w '%{http_code}\n' \
  -H "Authorization: Bearer $PORTAL_KEY" \
  https://portal.zagros.netbird.zagros.example/api/machines/grant/<some-handle>
# 200 = a grant exists · 404 = no grant (also a valid answer) · anything else = stop.

# 2. Confirm the NETWORK-JUMP group actually exists on the goatnet. The ACL
#    matrix declares it; that is not the same as it being applied.
aws-vault exec usareur-af --no-session -- \
  ops/scripts/env-door.sh zagros -- \
  curl -s -H "Authorization: Token $ZAGROS_NB_API_TOKEN" \
  https://netbird.zagros.example/api/groups | python3 -c \
  'import sys,json;print([g["name"] for g in json.load(sys.stdin) if "NETWORK-JUMP" in g["name"]])'
# [] means the taxonomy has not been applied — run netbird-acl-bootstrap.yml first.
```

## Step 1 — enrol the `NETWORK-JUMP` peer (operator)

The peer is a small dedicated box or VM. It needs a mesh client and nothing else; it runs no goat software beyond the connector.

```bash
goat admin machine enroll --context zagros-op \
  --node-class NETWORK-JUMP \
  --handle zagros-gear-jump-01 \
  --site zagros
```

**Verify the class took, and that it is source-only.** A peer in the wrong class is the whole failure mode this design exists to prevent:

```bash
goat admin machine show --context zagros-op zagros-gear-jump-01   # node_class must read NETWORK-JUMP
```

Nothing on the mesh should be able to initiate toward it. That is the design, and it is checkable — see §Verification.

## Step 2 — SPIFFE identity for the connector

The connector presents a client certificate and the console reads its SPIFFE ID off the **verified** cert. A hello frame claiming a different ID is refused, so the certificate is the identity.

Register the workload with SPIRE and confirm the three files land where the role expects: `/run/spiffe/cert.pem`, `/run/spiffe/key.pem`, `/run/spiffe/spire-bundle.pem`. The role refuses to deploy if any is missing — deploying without them would surface as a silent reconnect loop rather than an error anyone reads.

## Step 3 — secrets

Four for the console, in the operator's `.envrc.local.d` (never committed):

| Variable | What it is |
|---|---|
| `HERD_POSTGRES_PASSWORD` | herd's own database |
| `HERD_PORTAL_KEY` | herd's **machine** portal key — not a person's operator key |
| `HERD_OAUTH2_CLIENT_SECRET` | the Hydra client for the fence |
| `HERD_OAUTH2_COOKIE_SECRET` | oauth2-proxy cookie secret |

The portal key must also be dropped into the portal's hot-reloaded key dir (`PORTAL_OPERATOR_API_KEYS_DIR`), the same shape `onboard-runner` uses — **granting and revoking it needs no portal redeploy**.

## Step 4 — enable and deploy, console first

```bash
# group_vars/zagros_hosts: herd_enabled: true
aws-vault exec usareur-af --no-session -- \
  ops/scripts/env-door.sh zagros -- \
  ops/scripts/ansible-preflight.sh -i inventory/mgmt-hosts-mesh.ini herd-deploy.yml
```

Console before connector, deliberately: the connector dials the console, so a connector brought up first simply retries until the console exists. The reverse order produces a console that correctly refuses every session for want of a connector — which is the *right* behaviour and a confusing thing to debug if you did not expect it.

Then the connector, against the `NETWORK-JUMP` peer's own inventory entry.

## Step 5 — declare it, in the SAME change as the first deploy

Only now add the `isitup_services` row and the mesh DNS record, and regenerate `ops/dns/<site>/mesh-hosts`.

**Not before.** A declared-but-undeployed service is the F-335 failure: a service can carry both while never having been deployed and stay invisible, because isitup grades an absent container `unknown` rather than `down` and `unknown` is excluded from the degraded count. Being in the service registry earns a row and a name; it does not earn governance.

---

## Verification — what actually proves it

Each of these is a fact, not an impression:

```bash
# 1. The console answers and its fence redirects an unauthenticated caller.
curl -s -o /dev/null -w '%{http_code}\n' https://herd.zagros.netbird.zagros.example/   # expect 302

# 2. The connector REGISTERED. This is the only real liveness signal — the
#    connector has no listener by design, so "the container is running" proves
#    nothing about whether it reached the console.
docker logs herd 2>&1 | grep 'gear connector registered'

# 3. Nothing can initiate toward the jump peer. Run from the mgmt host:
nc -vz -w3 <jump-peer-mesh-ip> 22    # expect a TIMEOUT, not a refusal
```

That third check is the one worth doing carefully. A **timeout** is the pass — the ACL drops the packet. A *connection refused* means the packet arrived and nothing was listening, which would mean the peer is reachable and the containment is not what the design claims.

**A session is only proven by opening one**: a real switch, a person holding a site-scoped `network_admin` grant, and a shell prompt. Everything short of that — green containers, a registered connector, a passing health endpoint — is consistent with a console that cannot open anything.

## Rollback

Both components carry deploy-history ledgers, so `ops/scripts/rollback.sh herd` and `… herd-connector` revert to the previous pin. **Add the host row in the same change as the first deploy to a host** — a missing row means rollback covers part of the fleet and still reports success.

To stop gear access entirely without a rollback: set `herd_connector_enabled: false` and redeploy the connector. The console then refuses every session for want of a connector, which is the designed fail-closed state rather than an outage workaround.
