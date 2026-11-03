# Deploying goat gym

Brings `gym.<site>.<zone>` up on a goatnet's mgmt host. Design:
[goat-gym.md](../design/goat-gym.md) ([ADR 1132](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1132-goat-gym-training-surface.md)).

**Order: service, then DNS, then the status tile.** The isitup row and the mesh
DNS record are the same declaration — the per-site zone is *generated* from
`isitup_services` — so the row is authored first, but it is only *published*
after the service answers. Deploying the name before the service gives you a
window where `gym.<site>` resolves to a 404; deploying the service first does
not, because you can prove it with `curl --resolve` before any peer can reach
the name. Publishing DNS and never deploying is the F-197 shape: up, healthy,
unresolvable — or worse, resolvable and empty.

## 0. Prove the registry locally

```bash
python3 ops/gym/test_check.py      # the gate refuses what it should
python3 ops/gym/check.py           # the registry passes, with the coverage line
python3 ops/gym/generate.py        # renders into ops/gym/site/ — open index.html
```

The role runs `check.py` too, and a failure is fatal: dead pointers must not
ship.

## 1. Declare the row

Add to `ops/ansible/group_vars/<env>_hosts/vars.yml` under `isitup_services`,
in the `learn` category beside `goat-docs` and `brief`:

```yaml
  - id: gym
    name: "goat gym"
    desc: >-
      Role-based training lanes — what to read first, one drill per module,
      and the quirks each surface does not show you.
    fqdn: "gym.{{ site_id }}.{{ site_dns_zone }}"
    container: gym
    port: 80
    health_path: /index.html
    kind: ui
    # `public` is deliberate and matches the onboarding guide: mesh-only, no
    # oauth2 fence. Nothing here is a credential and every drill is safe to
    # fail; fencing it would hide the training from the people it is for
    #.
    access: public
    category: learn
    icon: docs
```

`health_path: /index.html` and not `/`: the row exists to answer "can someone
read the gym", and nginx answering on `/` does not prove the site rendered.

## 2. Deploy the service

```bash
cd ops/ansible
OBJC_DISABLE_INITIALIZE_FORK_SAFETY=YES \
  ../scripts/ansible-preflight.sh -i inventory/hosts.ini -i inventory/mgmt-hosts-mesh.ini \
  -l <mgmt-host> -e target=<env>_hosts gym-deploy.yml
```

Then prove it serves before anyone can resolve it — Traefik routes on `Host:`,
so `--resolve` reaches it without DNS:

```bash
curl -sS --resolve gym.<site>.<zone>:443:<mesh-ip> \
  -o /dev/null -w 'http=%{http_code} tls=%{ssl_verify_result}\n' https://gym.<site>.<zone>/
```

`http=200 tls=0` means the route, the container and the on-demand LE cert are
all good.

The preflight wrapper is not optional here. The served site is rendered from
**your working tree**: a stale trunk serves stale training, and stale training
is worse than none because it is believed. For the same reason the play stamps
`/gym.json`'s `rev` with `-dirty` when the tree has uncommitted changes, and
says so on the way past — a first deploy once served a module that was not in
the commit it named, which is the failure that field exists to make visible.

`-dirty` ignores `docs/project/active-work/`. That directory is live
multi-session coordination yaml which is essentially never clean, and none of
it is rendered — so keying the marker on it meant every deploy shipped `-dirty`
regardless, which is a warning
nobody reads. Everything the generator actually reads — `ops/gym/` and the docs
a module cites — still trips it.

Expect the play to print the coverage report — that is `check.py`'s output, and
it is the only place the operator sees how much of the platform the gym covers.

## 3. Publish the name

```bash
python3 ops/dns/generate-mesh-hosts.py
# it also bumps ops/dns/<site>/name-snapshot.json — RE-SIGN it or CI refuses:
cd ops/enrollment && go run ./cmd/name-snapshot-sign \
  --snapshot ../dns/<site>/name-snapshot.json \
  --root-key ~/.goat/state/<ca>/ca.key.age \
  --age-identity ~/.config/age/keys.txt
```

Push the zone to **every host that actually answers for the zone**, zone-only so
a routine push cannot rewrite the nameserver group:

```bash
cd ops/ansible
# the dedicated replicas (SSM)
OBJC_DISABLE_INITIALIZE_FORK_SAFETY=YES aws-vault exec usareur-af --no-session -- \
  ../scripts/ansible-preflight.sh -i inventory/<site>-mesh-dns.ini -l <site>_mesh_dns \
  goat-coredns.yml -e mesh_dns_register_group=false
# AND the mgmt host, if peers are pointed at it
OBJC_DISABLE_INITIALIZE_FORK_SAFETY=YES \
  ../scripts/ansible-preflight.sh -i inventory/hosts.ini -i inventory/mgmt-hosts-mesh.ini \
  -l <mgmt-host> goat-coredns.yml -e mesh_dns_register_group=false
```

**Check which resolver your peers were actually handed before you decide that
list.** An inventory comment can say the group is the dedicated replicas with
the mgmt host removed while `netbird status -d` on a live peer still names the
mgmt host. Pushing to the replicas alone then leaves the name resolving on the
replicas and nowhere a peer would ask.

Peers holding a stale negative cache may need `netbird down && sudo netbird up`;
on macOS the OS caches the NXDOMAIN separately, so a probe you ran *before* the
record existed will keep failing until `sudo dscacheutil -flushcache`.

Watch for F-039 on the zone file: the CoreDNS hosts plugin silently loads **zero
records** at mode 0600 and reports no error. `chmod 644` before `mv`.

## 4. Re-render the status page

The row is in group_vars; the tile appears when isitup re-renders. Use the
targeted play — **never `mgmt-host.yml`**, whose base-file `docker compose up`
recreates Traefik without its ACME overlay and takes every HTTPS surface on the
goatnet to the default self-signed cert.

```bash
cd ops/ansible
OBJC_DISABLE_INITIALIZE_FORK_SAFETY=YES \
  ../scripts/ansible-preflight.sh -i inventory/hosts.ini -i inventory/mgmt-hosts-mesh.ini \
  -l <mgmt-host> isitup-deploy.yml
```

## 5. Verify — from off the host

```bash
curl -sS -o /dev/null -w '%{http_code}\n' https://gym.<site>.<zone>/
curl -sS https://gym.<site>.<zone>/gym.json | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["rev"], len(d["modules"]), "modules")'
```

Check the `rev` against the commit you deployed. That field exists because
"deployed" and "serving what you deployed" are different claims, and only the
second one matters.

Then open it in a browser as a **non-admin** peer. The gym is mesh-only and
otherwise unfenced; if an ordinary developer gets a 403, the middleware is
wrong, not the user.

## Rollback

```bash
ops/scripts/rollback.sh gym
```

Rolls the nginx image pin. **The content does not roll back with it** — content
lives in the repo, so reverting content means reverting the commit and
re-running the deploy. Say this out loud during an incident: the ledger tracks
the image, not the training.

## Notes for the next person

- **No release tag.** Unlike `docs.<site>`, there is nothing to bump. Redeploy
  after a merge that changes `ops/gym/` and the site follows.
- **The role recreates on a stale `nginx.conf`.** It checksums the container's
  copy against the host's, because an interrupted run leaves a single-file bind
  mount converged on the host and stale in the container, and the re-run reports
  `changed=0 failed=0`.
- **The health check can fail.** It probes from inside the container with no
  `failed_when: false`. If it goes red the deploy fails, which is the point
  
