# panoscope operator runbook

| Field | Value |
|---|---|
| Status | Initial (Block 82Q.9). Refreshed after the first 82Q.8 smoke deploy. |
| Audience | Operators on the nova-goatnet host (nova-a) bringing panoscope up, rotating certs, tuning sampling, or diagnosing a degraded visualization. |
| Related | `docs/design/panoscope.md`, `docs/operations/nova-cert-issuance.md`, [`docs/design/zenoh-trial-fabric.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/zenoh-trial-fabric.md), `docs/design/nova-x509-identity.md` |

## What this is

End-to-end runbook for the panoscope service: bringup, cert rotation,
sampling tuning, legend-panel interpretation, troubleshooting. Lives
alongside the role and crate; covers the operator workflow without
referring to internal-build details.

## 1. Pre-flight gate

Before the first deploy, confirm:

| Check | Command | Pass criterion |
|---|---|---|
| Nova Ory standup is live | `curl -sf https://auth.nova.netbird.nova.example/health/ready` | HTTP 200 |
| Hydra admin endpoint reachable | `curl -sf http://127.0.0.1:4445/health/ready` | HTTP 200 |
| Zenoh trial-fabric router is up | `ss -tlnp` shows `:7447` LISTEN on nova-a | yes |
| oauth2-proxy is running | `docker ps --filter name=oauth2-proxy` | container running |
| Traefik file provider is watching | `docker exec traefik ls /etc/traefik/dynamic/` | dir exists |

If any check fails, stop — fix the underlying dependency first.

## 2. Mint the Zenoh-client leaf

panoscope needs **one** X.509 leaf under the nova offline CA — the
mTLS client cert for the Zenoh router. The browser-facing TLS cert is
issued by Let's Encrypt through the existing Traefik instance on nova-a
(same `le` resolver auth / dashboard / oauth2 already
use); no nova-CA front leaf required.

| Cert | Purpose | EKU | SAN shape |
|---|---|---|---|
| **Zenoh-client** | mTLS to the Zenoh router as `urn:goat-nova:operator` | `clientAuth` | URI-SAN: `urn:goat-nova:operator` |

Use the operator's age identity to mint it:

```bash
# Zenoh-client leaf.
ops/enrollment/cmd/cert-sign \
  --root-cert    ops/enrollment/ca/nova/root-cert.pem \
  --root-key     ops/enrollment/ca/nova/ca.key.age \
  --age-identity ~/.config/age/keys.txt \
  --ca-id        dev-nova-ca-ecdsa-2026-05-14 \
  --site-id      nova-trial \
  --cn           panoscope \
  --uri-san      "urn:goat-nova:operator" \
  --eku          clientAuth \
  --validity     8760h \
  --out-cert     /etc/panoscope/zenoh-client-cert.pem \
  --out-key      /etc/panoscope/zenoh-client-key.pem \
  --audit-log    ~/.goat/ca-audit-nova.jsonl

# Place the nova CA root cert for the panoscope client's trust store.
install -m 0644 \
  ops/enrollment/ca/nova/root-cert.pem \
  /etc/panoscope/nova-ca-root.pem
```

Verify with `openssl x509 -in <cert> -noout -text` — confirm `Subject:`,
`X509v3 Extended Key Usage`, and the URI-SAN match.

DNS for `panoscope.<site_dns_zone>` lives in mesh CoreDNS on nova-a
(`ops/dns/nova/mesh-hosts` →
re-run `ops/ansible/mesh-coredns.yml`
after edits). The `panoscope_publish_cloudflare_dns` flag is `false`
on nova per `docs/operations/nova-public-dns-teardown.md`;
the Cloudflare-publish path remains for any future site that genuinely
needs a public service-hostname surface (none today).

## 3. First-time image publish (one-time only)

The container image lives at `ghcr.io/usareur-af/panoscope` — published by
[`.github/workflows/panoscope-image.yml`](../../.github/workflows/panoscope-image.yml)
on every `main` push + every `panoscope/v*` tag. Once the panoscope
branch lands on `main`, every subsequent deploy just `docker pull`s the
image. The chicken-and-egg first deploy needs one operator push from a
workstation with `write:packages` scope:

```bash
# One-time only — until the panoscope-image workflow has run once.
cd ops/panoscope
docker build -t panoscope:local --platform linux/arm64 .
echo "$GITHUB_TOKEN" | docker login ghcr.io -u <github-user> --password-stdin
docker tag panoscope:local ghcr.io/usareur-af/panoscope:v0.0.1-alpha
docker tag panoscope:local ghcr.io/usareur-af/panoscope:latest
docker push ghcr.io/usareur-af/panoscope:v0.0.1-alpha
docker push ghcr.io/usareur-af/panoscope:latest
```

After the branch merges, the workflow rebuilds on every change to
`ops/panoscope/**`. Tags `panoscope/v*` cut versioned
releases.

## 4. Deploy

```bash
cd <repo-root>/ops/ansible
OBJC_DISABLE_INITIALIZE_FORK_SAFETY=YES \
  CLOUDFLARE_API_TOKEN=$CLOUDFLARE_API_TOKEN_NOVA \
  ansible-playbook -i inventory/mgmt-hosts-mesh.ini panoscope.yml
```

The role does, in order:

1. (When `panoscope_zenoh_mtls_enabled: true` — which nova now is)
   pre-flight `stat`s the 3 mTLS cert files at
   `/etc/panoscope/{nova-ca-root.pem, zenoh-client-cert.pem,
   zenoh-client-key.pem}` and fails fast if any is missing. The
   operator-only mint + scp step in
   `docs/operations/nova-cert-issuance.md`
   has to run before the role.
2. (When `panoscope_publish_cloudflare_dns: true` — currently `false`
   on nova; mesh CoreDNS owns the hostname) creates any missing
   Cloudflare A records for `panoscope.<site_dns_zone>` pointing at the
   `panoscope_cloudflare_a_record_ips` list. Path retained for any
   future site that genuinely needs a public service-hostname surface.
3. Creates `/opt/panoscope/`.
4. Templates `docker-compose.yml` + `.env` from defaults +
   per-host group_vars. Compose joins panoscope to two external
   networks: `mgmt` (so Traefik can resolve `panoscope:8080`) and
   `zenoh_default` (so it can reach `zenoh-router:7447`).
5. Renders the Traefik file-provider config at
   `/etc/traefik/dynamic/panoscope.yml` (same pattern the other surfaces
   use) — Host rule + `le` cert resolver. Traefik picks the file up
   automatically; first request triggers Let's Encrypt DNS-01.
6. Provisions the Hydra OAuth2 client `panoscope` (`PUT /admin/clients`,
   idempotent on 200/201/409) at the loopback Hydra admin URL.
7. `docker pull`s the image + `docker compose up`.
8. Polls `docker inspect panoscope` until `State.Running == true`
   (30 retries × 2s = 60s budget).

## 5. Verify the deploy

| Check | Command | Pass criterion |
|---|---|---|
| Container Up | `docker inspect -f '{{.State.Running}}' panoscope` | `true` |
| Healthz via Traefik | `curl -sf https://panoscope.<site_dns_zone>/healthz` from a browser-reachable host | `{"status":"ok"}` |
| Version | `curl -sf https://panoscope.<site_dns_zone>/version` | `name: panoscope` |
| Stats | `curl -sf https://panoscope.<site_dns_zone>/api/stats` | `data_samples_seen` > 0 within 60 s if vendors are publishing |
| Snapshot | `curl -sf https://panoscope.<site_dns_zone>/api/snapshot \| jq '.nodes \| length'` | ≥ 1 once Zenoh peers connect |
| Legend resolver | `curl -sf https://panoscope.<site_dns_zone>/api/legend \| jq '.schema_resolver_status'` | always `family-map` (the compiled-in family-map floor; per-peer coloring rides schema-decls) |
| Browser → page | open `https://panoscope.<site_dns_zone>/` | OIDC login → graph renders |

Note: the `/healthz` + `/version` + `/api/*` endpoints all require an
authenticated session. Test from a browser that's already gone through
the Ory login, or use a client-credentials token from Hydra.

If the page is blank in the browser:
- Open the browser console — JS errors land there.
- Check `docker logs panoscope` — Rust-side errors land there.
- Verify Traefik routed correctly: `docker logs traefik 2>&1 | grep
  panoscope`.

## 6. Sampling rate tuning

Default 50 events/sec/topic + 100 ms WS coalesce. Raise + lower via env
vars:

```bash
# Raise sampling to 200/sec/topic — useful for high-rate vendor demos.
ansible nova_hosts -m lineinfile \
  -a "path=/opt/panoscope/.env regexp='^PANOSCOPE_SAMPLING_PER_TOPIC_EPS=' line='PANOSCOPE_SAMPLING_PER_TOPIC_EPS=200'"
ansible nova_hosts -m shell \
  -a "cd /opt/panoscope && docker compose restart panoscope"
```

Watch `curl -sf http://127.0.0.1:8080/api/legend | jq '.sampling_drops'`
— if drops climb to non-zero for the topics you care about, raise
further. If you see drops on topics that don't matter, lower the global
ceiling and accept the drops.

The 100 ms WS coalesce tick (`PANOSCOPE_WS_COALESCE_MS`) is rarely worth
tuning. Drop it to 50 ms only if the visualization feels laggy at the
browser; raise it to 200 ms if you're CPU-bound on the browser side.

## 7. Cert rotation

Only the Zenoh-client leaf needs operator-driven rotation — the
browser-facing front cert is Let's Encrypt and renews automatically
through the existing Traefik instance (same auto-renewal that handles
auth / dashboard / oauth2).

The Zenoh-client leaf is valid for 1 year. Rotate ~30 days before
expiry:

```bash
# Mint a new client leaf with the same URI-SAN.
ops/enrollment/cmd/cert-sign ... # (same args as §2; new --out-* paths)

# Atomically swap in.
install -m 0644 new-zenoh-client-cert.pem /etc/panoscope/zenoh-client-cert.pem.new
install -m 0600 new-zenoh-client-key.pem  /etc/panoscope/zenoh-client-key.pem.new
mv /etc/panoscope/zenoh-client-cert.pem.new /etc/panoscope/zenoh-client-cert.pem
mv /etc/panoscope/zenoh-client-key.pem.new  /etc/panoscope/zenoh-client-key.pem

# Restart panoscope so it picks up the new client cert.
cd /opt/panoscope && docker compose restart panoscope
```

Re-verify per §4. Audit log entry should show the rotation.

## 8. Legend-panel interpretation

The bottom strip of the browser UI surfaces six fields:

| Field | Meaning |
|---|---|
| `resolver` | Always `family-map` — the compiled-in family-map floor is the resolver (the dead portal bulk-fetch path was removed, F-230). Per-peer edge coloring rides schema-decl self-registration on top; this is the normal healthy state, not a portal-reachability verdict. |
| `<N> families` | Number of distinct schema families in the compiled-in floor (6). |
| `aliases` | Slot-alias registry health (ADR 1039 §4.4). A count is healthy. `none assigned` means the portal answered and this goatnet has no aliases. Amber `off — no portal key` / `portal unreachable` / `<N> (stale <age>)` each name a different reason peers are rendering raw UUIDs — hover for the reason and the fix. See §9 below. |
| `drops` | Cumulative sampling-bucket drops since process start. Steady non-zero means your sampling ceiling is below the actual publish rate on some topic. |
| `τ` | EMA time constant (seconds). Edges go invisible at ~6τ of inactivity. |
| `tick` | WS coalesce window (ms). |
| `as of` | Last legend refresh timestamp. |

## 9. Troubleshooting

### Resolver shows `family-map` (this is normal)

The legend's `resolver` is always `family-map` — the compiled-in
family-map floor is the resolver. It is **not** a portal-reachability
verdict and never reads `embedded-fallback`/`portal-live`/`stale` (the
dead portal bulk-fetch path was removed, F-230). Edge coloring comes from
peer schema-declarations (`attest/schema/**`) plus this floor. If edges
are uncolored/gray, the cause is missing schema-decls for those peers
(real portal-minted vendors don't self-attest), not the resolver — see
`docs/design/panoscope.md` §"Schema-color data
source".

### Deploying: pass BOTH inventories on an A/B goatnet

```bash
cd ops/ansible
OBJC_DISABLE_INITIALIZE_FORK_SAFETY=YES ../scripts/ansible-preflight.sh \
  -i inventory/mgmt-hosts.ini -i inventory/mgmt-hosts-mesh.ini \
  panoscope.yml -e target=zagros_prod_hosts
```

The play deploys to the **active router only** (`router_role == 'A'`), and
`router_role` is declared **only in `mgmt-hosts.ini`** — the mesh file
overrides `ansible_host` / connection / user and nothing else. Run with the
mesh file alone and `router_role` is undefined, the play's `default('A')`
reads every host as active, and it deploys to the **standby**, which has no
`/etc/panoscope/*.pem` and fails its cert precondition. A correct run shows
Router-B as `ok=0 changed=0 failed=0 skipped=N`.

Sanity-check the inventory before a run you care about:

```bash
ansible-inventory -i inventory/mgmt-hosts.ini -i inventory/mgmt-hosts-mesh.ini \
  --host mgmt-zagros-prod-02-orin | jq .router_role   # must be "B"
```

Single-host goatnets (zagros, nova) declare no `router_role` and are unaffected —
the permissive default is deliberate, so that absence is not an error.

### Peers show raw UUIDs instead of names

Read the legend strip's `aliases` cell first — it names which of four
situations you are in, and they have different fixes. Headless equivalent:

```bash
curl -sf http://127.0.0.1:8080/api/legend | jq '.alias_registry'
```

| `state` | What it means | Fix |
|---|---|---|
| `ok` | Portal answered. `aliases: 0` genuinely means none assigned on this goatnet. | Nothing to fix here — a slot's alias is its onboarding handle, recorded when the slot is cut (portal `handleOnboard`), so an alias-less goatnet is one where nothing has been onboarded through that path. |
| `disabled` | `PANOSCOPE_PORTAL_API_KEY` was never rendered into the container, so `/api/aliases` is never called. **A deploy gap** — it will not self-heal. | Set `panoscope_portal_api_key` in `group_vars/<env>_hosts/panoscope.yml` and re-apply `panoscope.yml -e target=<env>_hosts`. Confirm with `docker inspect panoscope -f '{{json .Config.Env}}' \| tr ',' '\n' \| grep PORTAL` — **absence of the line is the tell**, since `env.j2` omits it entirely when the value is empty. |
| `unreached` | Key is wired but no poll has ever succeeded. `last_error` carries the reason. | 401 ⇒ the portal doesn't hold the key you present: compare `panoscope_portal_api_key` against `portal_operator_api_keys` in that goatnet's `vars.yml`. Connection refused ⇒ the portal isn't up yet. |
| `stale` | Worked before, latest poll failed; the last-good registry is still being served rather than blanked. | Usually self-heals next tick. If it persists, treat as `unreached`. |

The `disabled` case also WARNs once in the container log at startup:
`docker logs panoscope 2>&1 | grep -i 'slot-alias'`. A healthy surface logs
`slot-alias registry refreshed rows=N aliases=N` every
`PANOSCOPE_PORTAL_REFRESH_SECS`.

**Deploy-order trap.** A surface deployed before its goatnet's portal exists
comes up `disabled` and stays there — nothing re-polls a key that was never
rendered. On any new goatnet standup, re-apply the inspection surfaces
**after** the portal is up, and check this cell.

### No packet-dots animating

- Are vendors actually publishing? `curl -sf
  http://127.0.0.1:8080/api/stats | jq` — `data_samples_seen` should
  increment over time.
- Is the Zenoh client connected? `docker logs panoscope 2>&1 | grep
  'zenoh session open'` should appear once at startup. If the session
  fails, the Rust-side error log names the endpoint + cause.
- WS lagged? Open browser devtools → Network → WS → look for `lagged`
  frames. If you see them, the panoscope event channel is back-
  pressuring — raise the sampling ceiling or coalesce more aggressively.

### OIDC redirect loop on the browser

- Verify the Hydra client config: `curl -s
  $HYDRA_ADMIN_URL/admin/clients/panoscope | jq '.redirect_uris'` —
  must match the actual browser-facing URL exactly.
- Verify Traefik's ForwardAuth middleware is reaching oauth2-proxy:
  `docker exec traefik wget -qO- http://oauth2-proxy:4180/ping`.
- Verify oauth2-proxy is configured against the right Hydra issuer
  (check the oauth2-proxy env / config).
- Clear browser cookies for `*.nova.netbird.nova.example` and retry.

### Frontend changes do not show up after `cargo build`

The pages are embedded from `ops/panoscope/frontend/web/` at compile
time; there is no frontend build step. build.rs makes cargo rebuild
when a file there changes. If a change still does not show up, touch
`ops/panoscope/build.rs` and rebuild.

## 10. Tear-down

```bash
cd /opt/panoscope && docker compose down -v
docker rmi $(docker images -q ghcr.io/usareur-af/panoscope)
rm -rf /opt/panoscope
rm -f /etc/panoscope/zenoh-client-*.pem /etc/panoscope/nova-ca-root.pem
rm -f /etc/traefik/dynamic/panoscope.yml
docker kill -s USR1 traefik   # reload to drop the panoscope route
# Optional: revoke the Hydra client.
curl -s -XDELETE $HYDRA_ADMIN_URL/admin/clients/panoscope
```

Audit log entries for the leaves stay in
`~/.goat/ca-audit-nova.jsonl` per the offline-CA pattern.

## Cross-references

- `docs/design/panoscope.md` — design-of-record
- `docs/operations/nova-cert-issuance.md` — cert minting procedure
- `docs/design/nova-x509-identity.md` — X.509 identity model panoscope sits inside
- [`docs/design/zenoh-trial-fabric.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/zenoh-trial-fabric.md) — Zenoh substrate panoscope observes
- [`ops/panoscope/README.md`](../../ops/panoscope/README.md) — build + endpoint reference
- [`ops/ansible/roles/panoscope/`](../../ops/ansible/roles/panoscope/) — role implementation
