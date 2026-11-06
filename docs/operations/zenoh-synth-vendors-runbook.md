# synth-vendor publishers — bring-up + verify + teardown

| Field | Value |
|---|---|
| Audience | Operator on the nova trial fabric who wants to populate the substrate with real publish/subscribe traffic |
| Pre-reqs | nova-a's zenoh-router container Healthy on docker network `goat-zenoh_default`; goat-zenoh-portal Healthy at `https://portal.nova.netbird.nova.example` **running in real-mint mode** (`portal_cert_sign_fake: false`; pre-staged: `/usr/local/bin/cert-sign` arm64-static binary + `/etc/nova-ca/{root-cert.pem,ca.key.age}` + `/etc/nova-age/keys.txt`); operator API key for the portal in env |
| Composes with | [`zenoh-trial-fabric-synth-vendors.md`](../design/zenoh-trial-fabric-synth-vendors.md) (design) + `nova-zenoh-trial-deploy.md` (the substrate this rides on top of) |

## One-shot: register vendors + mint certs

Runs on operator laptop (any laptop on the netbird mesh).

```bash
# PORTAL_URL has no global default; pick the site.
export PORTAL_URL=https://portal.nova.netbird.nova.example

# PORTAL_OPERATOR_API_KEY should already be in your direnv via
# .envrc.local.d/30-site-services.envrc. If it's missing, fish the
# value from the live portal compose env:
#   ssh goat@100.127.0.189 \
#     'sudo grep ^PORTAL_OPERATOR_API_KEY /opt/goat-zenoh-portal/.env'
# (and append the export line to your .envrc.local.d so the next
# session doesn't have to repeat the lookup.) 1Password has the same
# value as a backup channel.

ops/zenoh-trial-peers/bootstrap-portal-vendors.sh
```

The script distinguishes HTTP 409 (already registered — normal on re-runs)
from other failure modes. A 400 / 500 surfaces the actual server response
inline so an operator can tell apart "already there, skipping" from "the
portal binary rejects an unknown field" or "the cert-mint backend errored."
The "every vendor already registered" path skips the SCP step explicitly
instead of crashing on an empty bundle map (the pre-2026-05-17 script
crashed with `unbound variable VENDOR_BUNDLE_URL` in that case).

Effects:

1. POSTs `/api/vendors` for `caracal`, `community`, `homemade-sensors`, and `pangolin` (idempotent — HTTP 409 on re-run is normal and skipped). Each vendor is registered with `"synthetic": true` so the portal UI badges them and panoscope's "Show synthetic" toggle can hide them once real vendors are publishing.
2. Downloads each vendor's one-time cert tarball (real PEM cert + key under the nova offline-CA, plus the nova CA root) into `./synth-vendor-certs/<vendor>/`. Real X.509 leaves with CN `<vendor>-trial-peer` + URI-SAN `urn:goat-nova:vendor=<vendor>` (the URI-SAN prefix is project-wide).
3. POSTs `/api/vendors/{v}/topics` for the seven demo topics:
   - `caracal/tracks-demo/entity/v1` (json)
   - `caracal/power-demo/site-nova/v1` (json, cross-vendor sub to `community/**`)
   - `community/beacon-demo/hello/v1` (json, cross-vendor sub to `caracal/**`)
   - `homemade-sensors/garage/temperature/v1` (json)
   - `homemade-sensors/garage/door-state/v1` (json, intra-vendor sub to the sibling temperature topic)
   - `homemade-sensors/yard/soil-moisture/v1` (**cbor**)
   - `pangolin/observer/heartbeat/v1` (json, observer subscribes to `**`)
4. SCPs `./synth-vendor-certs/<vendor>/` to `goat@100.127.0.189:/opt/synth-vendors/certs/<vendor>/` so the docker-compose role can mount it read-only.

Override `SCP_TARGET=""` to skip the SCP step (e.g. if the certs already live on the target).

## Deploy publishers on nova-a

```bash
cd ops/ansible
OBJC_DISABLE_INITIALIZE_FORK_SAFETY=YES \
  ansible-playbook -i inventory/mgmt-hosts.ini -i inventory/mgmt-hosts-mesh.ini \
    -l <mgmt-host> zenoh-synth-vendors-deploy.yml
```

Effects:

- `docker compose pull` against `ghcr.io/usareur-af/synth-vendor:latest`.
- Renders `/opt/synth-vendors/docker-compose.yml` with seven services (`tracks-demo`, `power-demo`, `beacon-demo`, `garage-temp`, `garage-door`, `yard-soil`, `pangolin-observer`).
- `docker compose up -d` brings them up; each container joins the existing `goat-zenoh_default` network and reaches the router via `goat-zenoh-router:7447`.

## Verify

```bash
ssh goat@100.127.0.189 'cd /opt/synth-vendors && docker compose ps'
```

Expect seven `Up` services. Tail logs:

```bash
ssh goat@100.127.0.189 'for s in tracks-demo power-demo beacon-demo garage-temp garage-door yard-soil pangolin-observer; do echo "--- synth-$s ---"; docker logs --tail 5 synth-$s; done'
```

Expect each to log `session open` with a distinct `zid=`.

Verify via Zenoh REST (one sample per vendor):

```bash
ssh goat@100.127.0.189 \
  'curl -s http://goat-zenoh-router:8088/caracal/tracks-demo/entity/v1?_t=raw | jq .'

ssh goat@100.127.0.189 \
  'curl -s http://goat-zenoh-router:8088/homemade-sensors/garage/door-state/v1?_t=raw | jq .'

# yard-soil is CBOR-encoded — inspect raw bytes / decode in panoscope's inspector
ssh goat@100.127.0.189 \
  'curl -s http://goat-zenoh-router:8088/homemade-sensors/yard/soil-moisture/v1 | xxd | head -5'

ssh goat@100.127.0.189 \
  'curl -s http://goat-zenoh-router:8088/pangolin/observer/heartbeat/v1?_t=raw | jq .'
```

Expect payloads matching the schemas in [`ops/zenoh-trial-peers/synth-vendor/schemas/`](../../ops/zenoh-trial-peers/synth-vendor/schemas/).

Visual:

- Open `https://panoscope.nova.netbird.nova.example/` — within 60s, seven nodes should appear (one per ZID) plus animated edges. Cross-vendor edges to look for: `caracal → community/beacon-demo`, `community → caracal/power-demo`, `homemade-sensors/temperature → garage-door`, and pangolin-observer receiving from everyone. **Click the "Show synthetic" checkbox in the legend footer to hide the synthetic demo set** — until real vendors land the graph will go empty; flip it back to see the demo data.
- Open `https://portal.nova.netbird.nova.example/` — Vendors page lists `caracal`, `community`, `homemade-sensors`, `pangolin` each with a tinted row + `SYNTHETIC` badge; per-vendor Topics page shows the matching topics with their schemas linked.
- Open the inspector at e.g. `https://portal.nova.netbird.nova.example/inspector.html?key=homemade-sensors%2Fgarage%2Fdoor-state%2Fv1` — expect samples to flow via the rocksdb storage backend. For the yard-soil topic, pick `cbor` from the format dropdown to decode.

## Upgrading the portal binary on a site that pre-dates `synthetic`

If a site is running a portal binary that pre-dates PR #161 (no `synthetic`
column on the vendors table, no Block 85 schema-dir flag) AND has the
sentinel-hash `homemade-sensors` example seed in its registry, the upgrade
path is NOT a clean `docker compose pull && up -d` — there are two pieces
to clean up by hand:

1. **Old seeded `homemade-sensors` row has no real cert.** The pre-#161
   binary seeded a row with `api_key_hash = 'EXAMPLE_VENDOR_NOT_USABLE'`,
   so no ACL allowlist entry exists. The bootstrap script's POST returns
   409 (prefix exists) but skips cert mint, leaving the row unusable.
   Fix: delete the seeded row before re-running bootstrap. From the
   portal host (sqlite3 is installed on nova-a; the distroless container
   doesn't have it):

   ```bash
   ssh goat@100.127.0.189 \
     'sudo sqlite3 /opt/goat-zenoh-portal/registry.db \
       "DELETE FROM topics WHERE vendor_prefix=\"homemade-sensors\"; \
        DELETE FROM vendors WHERE prefix=\"homemade-sensors\";"'
   ```

   The re-run bootstrap then mints `homemade-sensors` as a real
   `synthetic: true` vendor.

2. **Pre-existing `caracal` + `community` rows have `synthetic=false`.**
   The additive `ALTER TABLE … ADD COLUMN synthetic INTEGER NOT NULL
   DEFAULT 0` migration lands `false` on rows that pre-dated the
   column. The portal API only accepts `synthetic` at vendor CREATE
   time, not via PATCH. Fix is one SQL update:

   ```bash
   ssh goat@100.127.0.189 \
     'sudo sqlite3 /opt/goat-zenoh-portal/registry.db \
       "UPDATE vendors SET synthetic=1 WHERE prefix IN (\"caracal\", \"community\");"'
   ```

This dance is a ONE-TIME cutover. Fresh portal deploys (no pre-existing
DB) go through the bootstrap cleanly because every vendor lands with
`synthetic: true` at creation, and the sentinel-hash seed code is gone
from the binary entirely.

If the upgrade also crosses Block 85 (PR #167), the portal binary's
default `--schema-dir=/var/lib/goat-zenoh-portal/schemas` is unwritable
inside the distroless container — the role-template fix is in PR #179.
Sites that pre-date that PR need a fresh `goat-zenoh-portal` role apply
to render the compose with `--schema-dir` pointing at the mounted
`portal_data_dir/schemas` path.

## Tear down

For trial sunset or rebuild-from-scratch:

```bash
ssh goat@100.127.0.189 'cd /opt/synth-vendors && docker compose down -v'
ssh goat@100.127.0.189 'sudo rm -rf /opt/synth-vendors/certs/'
```

Then via portal UI (or `DELETE /api/vendors/{v}` once that endpoint exists) drop the four synthetic vendor rows. The corresponding ACL allowlist entries get re-rendered out by the goat-zenoh role on next run.

## Failure modes

| Symptom | Likely cause | Fix |
|---|---|---|
| `bootstrap-portal-vendors.sh` 401 | wrong `PORTAL_OPERATOR_API_KEY` | check `.envrc.local.d/30-site-services.envrc` first (direnv-loaded), then 1Password, then the portal container's compose env (`/opt/goat-zenoh-portal/.env`) |
| `bootstrap-portal-vendors.sh` reports `FAILED (HTTP 400)` on a vendor with `"json: unknown field \"synthetic\""` | portal binary pre-dates PR #161 (no `synthetic` column) | upgrade the portal image on the host (`docker compose pull && up -d`), confirm the new binary serves `synthetic: bool` on `/api/vendors`, then re-run bootstrap |
| Bundle download URL 404 on re-run | bundle tokens are one-shot; vendor row exists but the cert tarball is gone | direct-SQL DELETE the vendor row on the portal host (see "Upgrading the portal binary on a site that pre-dates `synthetic`" above), re-run bootstrap |
| Portal vendors page shows the row but with no `SYNTHETIC` badge | bootstrap predates this PR and registered the vendor without `synthetic: true` | re-mint via portal UI (operator → register form → check the `Synthetic (demo data)` box), OR drop the row + re-run the bootstrap script |
| containers `Up` but no panoscope nodes | `goat-zenoh-router` container down OR not on the `goat-zenoh_default` network | `docker network inspect goat-zenoh_default` + `docker compose ps` on the zenoh project |
| `tls/` endpoint + ACL rejects writes | cert CN doesn't match the allowlist (vendor row stale, CN mismatch) | re-run `bootstrap-portal-vendors.sh` after dropping the vendor row, OR confirm `cert_common_names` rule in `/etc/goat-zenoh/config.json5` matches the cert's CN |
| beacon-demo `observed_caracal_topics` empty | tracks-demo or power-demo not actually publishing, OR beacon-demo's subscriber declaration failed | check tracks-demo / power-demo logs first; if both are publishing, check beacon-demo logs for `subscriber recv error` |
| garage-door's `related_topics_seen` empty | garage-temp not publishing yet (it ticks once per 30s — give it 60s after deploy) | check `docker logs synth-garage-temp` for `put failed`; if clean, just wait |
| panoscope "Hide synthetic" hides too many nodes | synthetic flag inadvertently set on a real vendor (or fallback embedded set incorrectly classifies one) | check `https://portal.nova.netbird.nova.example/api/vendors` JSON output for the vendor row's `synthetic` field; toggle off via portal admin |
| `yard-soil` inspector shows `(no samples)` | rocksdb storage spawn regression | check `docker logs goat-zenoh-router` for `Plugin compatibility mismatch` or `Cannot spawn volume 'rocksdb'`; live data still shows in panoscope regardless of storage state |
