# Zenoh topology surface — `topology.<site>.<zone>`

| | |
|---|---|
| Status | Built and deployed, including not-reporting routers and publish fallback (§2.3, §2.4). Router names and owners (§2.5) built, measured on a local rig, not deployed. What each router carries (§2.6) built, measured on a local rig, not deployed |
| ADR | [1160](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1160-zenoh-topology-surface.md) |
| Component | `ops/goat-topology` — one binary, two modes |
| Evidence it rests on | M-111 (the fleet on `Auto`), M-112 (`Custom([...])` in a rig) |
| Canon it must obey | [ADR 1123](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1123-client-demand-must-not-amplify.md) (no query on request), [ADR 1036](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1036-environment-health-surface.md) (self-publish + roll-up), [ADR 1041](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1041-zenoh-router-zid-pinning-and-source-provenance.md) / [ADR 0844](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0844-zenoh-deployment-topology-and-acl-v1.md) (a ZID is a label, not a credential) |

## 0. Purpose, and what would count as success

**The problem.** `gateway.south` decides two things that matter: how much
routing state a goatnet carries, and who can see whom at the routing layer. It
runs fleet-wide on a default nobody chose (M-111),
which leaves two failure modes invisible — a config that is wrong, and a config
that does not match what is running — plus one genuinely dangerous mistake: the
filter list has the exact shape of an access control while two of its five axes
are asserted by the peer being filtered.

**Who it is for.** The operator carving, debugging, or reviewing a routing
plane. Explicitly **not** a daily-glance surface; nobody should be watching this
at rest, and the design does not pretend otherwise.

**Success is four things:**

1. "What is my routing plane doing" is answerable without a shell on a router host.
2. The peer-asserted-filter mistake is caught at review time, not after someone has scoped a partner with a routing hint.
3. A publisher going quiet reads as **unknown**, never as healthy.
4. Zero fabric cost per viewer, verifiable in a diff.

**Non-goals**, stated so they do not creep in: alerting, any control path, mesh
reachability, and any claim about scale beyond what M-111 and M-112 measured.

### 0.1 What that purpose implies about the layout

The surface is opened when something is wrong or about to change — so it is
ordered by the question actually being asked, most frequent first:

| Order | Question | Answered by |
|---|---|---|
| 1 | Is anything wrong? | The verdict line |
| 2 | Where am I? | The routing graph |
| 3 | What is known about this one router? | The router detail, opened by selecting a router |
| 4 | What exactly is wrong, and what do I do? | Findings |
| 5 | Did something change? | The change feed |
| 6 | What is the detail of every router? | The routers table |
| 7 | What would a carve cost? | The planner, on the measured model |

**The graph leads, but the verdict leads it.** The graph is the orientation
device — it answers "where am I" faster than any sentence can, and an operator
arriving cold needs that before words. What it must *not* be asked to do is
carry the verdict: nobody should have to read a picture to learn whether
anything is wrong, so a one-line verdict sits above it and says so outright.

### 0.2 Findings are written to be read cold

A finding is read by someone who has just opened the page, mid-incident,
possibly having never heard the word "subregion". So each one is built in a
fixed order that answers questions in the order they occur:

| Field | Answers |
|---|---|
| `headline` | **What is the problem, in plain words?** Consequence first, no jargon, no identifier. |
| `what` | What did we actually see? The evidence, with names and numbers. |
| `why` | Why does that matter? Mechanism, in short sentences. |
| `impact` | What happens if I do nothing? |
| `steps` | What do I do? Imperative, ordered, one action each. |

Three rules that fall out of that, each of which the earlier version broke:

- **The identifier is not the headline.** `T-001` is quotable in a ticket and
  tells a cold reader nothing, so it sits at the end of the header row.
- **A branch belongs in the steps, never inside a sentence.** The first version
  of `T-001` buried "if the intent is routing… if the intent is access…" in a
  paragraph, and a reader could not tell which half applied to them. It is now
  step 1: *decide which this is for*, then a step per branch.
- **Jargon is allowed below the headline, never in it.** A test builds one of
  every finding and asserts the headline is a plain sentence, carries no
  identifier, and contains none of `subregion` / `zid` / `link-state` / `Σ`.
  Below the headline the surface has room to define its terms, and a collapsible
  glossary at the section head does the rest.

Finding text in the design preview is emitted by the engine's own test rather
than transcribed, so the preview cannot drift from what the code says.

**The change feed is what an incident actually needs.** "A router joined the
north region twenty minutes ago" is the fact that explains a symptom, and a
surface showing only instantaneous state cannot produce it. The feed is held in
memory, so a restart empties it — the UI therefore says *since this surface
started watching*, because an empty feed must read as **not seen**, never as
**did not happen**.

**The planner exists because regions are for carving.** A surface about carving
that cannot help you evaluate a carve is missing its most useful move. It is
arithmetic on the M-112 model, and it says so on its face: a projection for
planning, bounded by a measurement taken at two regions of three routers on one
host, counting routing state only.

## 1. What this is for

A goatnet's zenoh routers form a routing domain whose cost and blast radius are
set by `gateway.south` — a setting that, as M-111
found, is on its `Preset(Auto)` default fleet-wide because **no configuration
sets it anywhere**. The behaviour is now documented in
`zenoh-data-fabric.md` §"Hierarchical routing". A
document is the right home for *what the primitive does*. It is the wrong place
to answer *what is my fabric doing right now, and is any of it wrong?*

`topology` answers that. It renders every router in a goatnet, the region each
of its sessions landed in, the shape of the resulting link-state domains, and a
graded list of findings with the remediation for each.

**Why it earns its own surface rather than a panoscope tab.** Panoscope is the
data COP — topics, samples, producers, hygiene. This is the *routing* plane: it
has a different subject (routers, not data), a different audience (whoever is
carving or debugging a fabric, not whoever is watching the mission picture), and
— decisively — a different failure mode. Panoscope is the one surface whose
client demand reaches the fabric ([ADR 1123](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1123-client-demand-must-not-amplify.md));
topology must never be, and keeping it out of panoscope keeps that property
easy to verify rather than something a future change could quietly erode.

### Scope boundary — read this before extending either surface

| Surface | Owns | Does **not** own |
|---|---|---|
| `netvis.md` | Mesh / IP reachability — can peer A reach peer B, and which ACL decides | Anything about zenoh routing |
| **topology** | Zenoh routing domains — regions, link-state graphs, route tables, `gateway.south` config | Anything about IP reachability or the mesh ACL |

They sit on different layers and answer different questions. A router pair can
be perfectly reachable at the IP layer and still be in different routing
regions, and the reverse. **Neither surface grows into the other**; a question
that spans both is answered by opening both, and by the cross-surface deep-link
convention ([`zenoh-surface-deep-links.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/zenoh-surface-deep-links.md)).

## 2. Shape — one binary, two modes

```
                 router host (each one)                 mgmt-host
   ┌──────────────────────────────────┐      ┌─────────────────────────┐
   │  zenohd                          │      │  goat-topology serve    │
   │   adminspace REST                │      │   subscribes            │
   │   127.0.0.1:8000  ◄──── curl ──┐ │      │   ops/fabric/topology/**│
   │  /etc/<stack>/config.json5 ◄─┐ │ │      │                         │
   │   (gateway block only)       │ │ │      │   in-memory store       │
   │  goat-topology publish ──────┴─┘ │      │        │                │
   │        │                         │      │        │                │
   └────────┼─────────────────────────┘      │        ▼                │
            │  publish RouterTopologyV1      │   findings engine       │
            │  ops/fabric/topology/          │        │                │
            └──────────► <site>/<node> ──────┼──►     ▼                │
                        (the fabric)         │   HTTP + graph UI       │
                                             │   topology.<site>.<zone>│
                                             └─────────────────────────┘
```

**Publish mode** runs on every host that runs a router. It reads **its own
router only**, on a timer, and publishes what it saw. It knows nothing about any
other router and never asks about one.

**Serve mode** runs on the mgmt-host. It subscribes, keeps the last snapshot per
router in memory, and serves the UI from that memory. **A browser request never
touches the fabric.**

### 2.1 The publisher reads host loopback, not a network namespace

The router's adminspace REST listener is published to host loopback —
`8000/tcp -> 127.0.0.1:8000`, verified on zagros and zagros-prod on 2026-09-04. The
publisher is therefore an ordinary container with `network_mode: host` or a
loopback port mapping; it needs **no `nsenter`, no `--privileged`, no
`CAP_SYS_ADMIN`, and no zenoh client cert to read the router's own state**.

What *is* invisible from the host namespace is the router's zenoh **sessions**
as seen by `ss` — they live in the container netns. That is a debugging trap and
is recorded as such in `ops/scripts/zenoh-region-census.sh`; it is not a
constraint on this design. Getting that distinction wrong would have designed a
privileged container for no reason.

The publisher **does** need an mTLS client cert — but only to *publish* onto the
fabric, exactly like every other meter.

### 2.2 Why self-publish rather than have the surface ask

Three independent reasons, any one of which would be sufficient:

1. **[ADR 1123](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1123-client-demand-must-not-amplify.md).**
   Client demand must not amplify into fabric load. A surface that queried each
   router per page view would be the exact shape that ADR forbids.
2. **Reachability.** The adminspace is bound to each router host's *loopback*. A
   central reader cannot reach it at all without opening a new network path,
   which would be a worse design and a new attack surface.
3. **Brokerless safety.** [ADR 1036](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1036-environment-health-surface.md)'s
   pattern — each node self-publishes, a passive reader rolls up — survives
   partition. A site cut off from the mgmt-host keeps publishing; the surface
   shows its last snapshot with an honest age, and recovers on reconnect with no
   central coordination.

### 2.3 Routers with no publisher are drawn from their neighbors' maps

The page draws every router that any reporting router's link-state lists, not
only the routers that publish. A router with no publisher of its own — a
partner's router dialling in, or one of ours the publisher was never deployed
to — is drawn smaller, with a dashed outline and its display name (§0.3), and
marked **not reporting**. Its links are real; its settings,
load and route table are unknown and are shown as blank, never as zero.

How it works. Every router floods its region's link-state graph to the other
routers in that region, so one reporting router's snapshot already names every
router in its `north` group and every link between them, including routers two
hops away that it holds no session with. The publisher carries that graph as
ZID pairs (`RegionV1.links`, §3). `serve` takes the union of all reporters'
router regions (`north` and `south:<id>:router`; a `:peer` or `:client` region
lists things that are not routers), subtracts the routers that report, and
draws the rest. No router is asked anything: the join runs on the write path
over snapshots already in memory, so §5 still holds.

| Seen by the surface | Drawn as |
|---|---|
| A router that publishes | Full node: name, mode, client arc, findings |
| A router in a reporter's `north` or `south:<id>:router` graph that does not publish | Not reporting: dashed, display name, its links, finding `T-012` (info) |
| A router listed only by reporters that are themselves stale | Not reporting **and** stale |
| A pod below a gateway that does not publish | **Nothing.** A `south` graph is held only by its gateway, so pods behind a silent gateway are invisible to every other router. Deploy the publisher on the gateway. |

Edges come from the same pairs, so a link between two silent routers is drawn
too. A pair seen as `north` from one end and `south:<id>:router` from the other
— a pod and its gateway — is drawn as terminated, because the gateway's view is
the true one. A publisher older than `links` sends none; the surface then draws
that router's own router sessions only.

Measured on a rig of eleven `zenohd` 1.9.0 routers on one host (fleet router
image), six with publishers: the page drew nine routers (six reporting, three
not reporting, one of them two hops from any publisher), nine routed links and
two terminated links. The two pods behind the silent partner gateway were not
drawn, as the table predicts.

### 2.4 Publishing from a router that refuses clients

A router whose `gateway.south` is a Custom list refuses every default client
unless one of its subregions can match a client — and that includes its own
publisher. The publisher therefore takes an ordered list of endpoints and
publishes through the first that admits it; what it **reads** is always its own
router's adminspace on loopback.

The mechanism, from zenoh 1.9.0 `net/runtime/region.rs:46-61`: a Custom
gateway puts any peer that matches no subregion in `north` and tells it so; a
client may never sit north of a router, so the client drops the session with
`Remote's custom configuration conflicts with auto preset`. The client's own
error is only `Unable to connect`. Because the publisher already reads the
declared `gateway` block, it says before connecting that its router will refuse
it, and reports the fact as `client_admission: refused`, which raises the
critical finding `T-013` — every application on that router is being refused,
not just the meter.

The router-side fix is one subregion appended to the list:
`{ filters: [ { modes: ["client"] } ] }`. Until it lands, set
`goat_topology_publish_fallback_endpoints` on that host to a north neighbor's
endpoint. Measured on the same rig: with a ZID-only list, the gateway's
publisher was refused by its own router and published through a core router;
after appending the client subregion and restarting the router, it published
through its own router on the first attempt and `T-013` cleared.

### 2.5 Who operates a router that sends no report

A router that sends no report is labelled with the name of the mesh peer its
session comes from, and its detail says who operates it: the peer name,
hostname and mesh groups, and the organisation, the organisation's nation and
the individual that the identity registry records. Nothing new is asked of the
fabric or of the mesh controller: `serve` joins two files that other roles
already write on the management host, and it reads them only when they change.

**The address.** A reporting router's snapshot already carries, for every
session it holds, the address the session comes from (`SessionV1.link_dst`,
for example `tls/100.127.0.21:50508`). On a goatnet that is the peer's mesh
address. `serve` takes the address a fresh reporter sees; a stale reporter's
view is used only when no fresh one has the session.

**The peer directory** (`--peer-directory`, role variable
`goat_topology_peer_directory_path`). A file mapping a mesh address to peer
name, hostname and groups. Two existing writers produce one, and both shapes
are read:

| File on the management host | Written by | Every | Credential it uses (the surface needs none) |
|---|---|---|---|
| `<relay_health_www_dir>/ingest/peer-names.json` | relay-health `relay-peer-names` timer, on by default | 5 min | none: it reads the mesh controller's database on the host |
| `/opt/mesh-health/state/peer-registry.json` | mesh-health snapshot timer, when `mesh_health_registry_enabled` | 20 s | the host's existing infra token, kept on the host |

**The roster** (`--roster`, role variable `goat_topology_roster_path`). The
identity registry's public-safe projection that the peerview role installs at
`/opt/peerview/roster.json`. It carries handles, organisations and their
nations, and each member's own nationality; it carries no names and no email
addresses.

**From peer to roster.** A peer name is evidence about ownership, not the
record of it, and the page says which it used. The name is used only to find a
roster handle, by the onboarding grammar the peerview surface already applies:

| Peer name | Handle | Individual shown |
|---|---|---|
| `peer-<handle>[-<goatnet>]-<yyyy>-<mm>-<dd>-<host>` | everything before the date, minus the goatnet token (`--goatnet`) | the person, `<handle>` |
| `machine-<handle>[-<host>]` | the longest roster handle the rest starts with | the custodian who enrolled the machine (roster `custody`) |
| anything else | none | none; organisation `not recorded` |

The handle is never decomposed further. Organisation, organisation nation and
nationality are then read from the roster and never derived from the name, a
group or an address. Every owner carries `attribution: peer-name`, and the
detail says "the peer name led to roster handle …; a lead, not the recorded
enrolment link". The recorded link (the portal's slot registry) needs the
portal operator key and is not read.

**Two nationality facts, kept apart.** An organisation's nation (roster
`orgs[].nation`) and a person's nationality (roster `members[].nationality`)
are different facts, and neither fills in for the other (ADR 1171). A machine
has no nationality: for a machine the detail shows the organisation's nation
and, beside the custodian, the custodian's own nationality, labelled as theirs.
A fact the roster does not hold is shown as `not recorded`.

| Where | Shows |
|---|---|
| Graph label, below the name | `<organisation> · <organisation nation>`, clipped in the organisation's name so the nation is always visible |
| Routers table | an Organisation · nation column |
| Detail, on click | mesh peer, hostname, groups, the address it connects from, organisation, organisation nation, the individual (or the custodian, for a machine), that person's nationality, and how the owner was found |

The individual and their nationality are in the detail only. A goatnet that
must not show them sets `goat_topology_show_individual: false`; the payload
then carries neither.

**What stays unnamed, and the page says why.** The detail's "Operated by" line
gives the reason:

| Case | Name shown |
|---|---|
| Known only through link state: no reporter holds a session with it | the display name derived from the ZID (§0.3); "known only through link state" |
| Its address is not in the peer directory | derived name; "the address it connects from is not in the peer directory" |
| No peer directory configured, or the file unreadable | derived name; the reason is said once above the routers table |

**How old the two files are.** The payload carries when each file was last
written (`directory.peers_modified_ms`, `directory.roster_modified_ms`), and the
note above the routers table names a file that is old: a roster written seven
or more days ago ("anyone enrolled since has no entry, so their routers show a
name and no owner"), and a peer directory not rewritten for a day. A router
whose name leads to no roster handle gives the roster's date in its detail.
The roster is not on a timer: the peerview role copies the goatnet's own
`docs/operators/<goatnet>/roster.json` from the assembled tree each time
`peerview-deploy.yml` runs, so a host's roster is as old as its last peerview
deploy.

Measured on the rig of §2.3, extended to twenty-two routers (twenty drawn),
each on its own loopback address, with a synthetic peer-names file and roster:
`rig.sh check` matched every not-reporting router's label to its directory
entry and its organisation, organisation nation, individual and nationality to
the roster (0 mismatches). It also showed the link-state-only router with its
derived name and reason, and a router absent from the directory with its reason.
The page loaded in headless Chromium had no label outside the picture and no
two labels overlapping.

### 2.6 What each router carries

The page shows, for every router, how many subscriptions and publishers it
holds and how many distinct key expressions (topics) it carries. The counts
are declarations read from each reporting router's own tables. They are not
traffic: this router build counts no messages or bytes. Each publisher reads
its own router over loopback on a cycle of its own, so a page view still costs
nothing (§5).

**What zenoh 1.9.0 attributes, and to whom.** Each entry of
`@/<zid>/router/subscriber/**` is one key expression and the sources that
declared it, as `{clients:[], peers:[], routers:[]}`
(`net/runtime/adminspace.rs`, `resources_data`). Measured on the rig
(`rig-client` declaring known keys on seven routers) and matched against a
live router's 539-entry table:

| List | Holds | Attributable to |
|---|---|---|
| `clients`, `peers` | the ZIDs of this router's own sessions that declared the key; the router's own process appears under its own ZID (`hat/broker/pubsub.rs:195`) | each of this router's clients and peers |
| `routers` | every other router in the north routing group that holds the key (`hat/router/pubsub.rs:278`), plus this router's own ZID for anything declared south of it (`dispatcher/tables.rs:339`) | the router whose application declared it, including a router two hops away; a declaration below another gateway is attributed to that gateway |

The publisher table (`router/publisher/**`) lists only this router's own
sessions. The router hat keeps no publishers (`hat/router/pubsub.rs:301`). So
the number of publishers that **another** router holds cannot be known from
here, and the page says "publishers not visible" for every router that sends no
report.

**What is published.** `RouterTopologyV1.declarations` (`DeclarationsV1`):

- This router's own totals: subscriptions and publishers declared by its
  sessions (per session, so two clients on one key count two), the keys those
  cover, keys held only by the router's own process (its adminspace and
  plugins, reported apart so they do not read as application load), and
  `topics`, the distinct key expressions across both tables.
- For each other router the subscriber table names, the number of subscribed
  keys attributed to it.
- At most `--declarations-top-keys` (default 5) key expressions per router and
  for the router itself, most-declared first, each clipped at 160 characters.
  This is a sample for recognition, not a ranking by traffic. Counts plus a
  capped sample, never the full list (cardinality budget, M-091).
- `read_unix_ms`, `read_ms`, `interval_secs`, `capped` (the
  `--declarations-max-entries` cap of 20 000 entries per table was hit, so
  counts are lower bounds) and `read_error`.

**Route entries per neighbour.** `next_hops` counts, from the successor table
the snapshot already reads, how many `src/<a>/dst/<b>` entries name each
neighbour as the next hop. That is how many destinations this router reaches
through each link. The detail lists it beside each neighbour.

**The surface.** A reporting router shows its own census. A router that sends
no report shows what the freshest readable census of a reporter in its routing
group attributes to it, and says which reporter. A reporter outside its group
sees none of its declarations, so it says nothing rather than zero. A census
that failed shows `not read`. A router no census covers shows `—`.

**Two cycles.** The 30 s snapshot used to read `@/local/**`, which in zenoh
1.9.0 also returns every subscriber, publisher, queryable, querier, token and
transport entry. It now reads only the four keys it uses: `@/local/router`,
`@/local/router/linkstate/*`, `@/local/router/route/successor/**` and
`@/local/router/status/plugins/rest/version`. The subscriber and publisher
tables are read on their own cycle:

| Setting | Flag / role variable | Default | Why |
|---|---|---|---|
| Interval | `--declarations-interval-secs` / `goat_topology_declarations_interval_secs` | 300 s | These are the large tables (hundreds of entries on a busy router), and what they count changes as applications start and stop. Ten snapshots per census. The page shows the read time. `0` turns the census off. |
| Time budget | `--declarations-timeout-secs` / `goat_topology_declarations_timeout_secs` | 10 s | Both reads together. A read that runs out publishes `not read`, never zeros. |
| Entry cap | `--declarations-max-entries` / `goat_topology_declarations_max_entries` | 20 000 | Past it the census is marked capped. A body over 32 MiB is refused, not truncated. |
| Sample | `--declarations-top-keys` / `goat_topology_declarations_top_keys` | 5 | Key expressions per router; `0` publishes counts only. |

Measured on the rig with one router holding 541 subscriptions and 50
publishers, 22 routers in its group, 100 reads each:

| Read | Bytes | Median | p95 |
|---|---|---|---|
| `@/local/**` (the old 30 s snapshot) | 177 764 | 5.1 ms | 6.4 ms |
| the four snapshot keys (new 30 s snapshot) | 46 447 | 3.3 ms | 4.6 ms |
| subscriber + publisher tables (census, every 300 s) | 123 222 | 4.4 ms | 5.3 ms |

Over 300 s that is 10 × 177 764 bytes read before, against 10 × 46 447 +
123 222 bytes after: 1.78 MB down to 0.59 MB.

Also measured on the rig: `rig.sh check` matched every router's count with
what its rig client declared, 0 mismatches. That covered a gateway's own
declarations plus those of its silent pod, a router two hops out, and each
reporter's own subscriptions and publishers.

**Not built.**

- **Messages and bytes per router, and traffic between two routers.** These
  need a router built with zenoh's `stats` feature. The fleet build's
  `router/metrics` holds only `zenoh_build_info`. Tracked in
  `docs/project/backlog.md`.
- **Traffic between two routers that both send no report.** This is not
  knowable from any reporter, whatever the build. The page says so once,
  above the routers table.
- **Publishers held by another router.** Not exposed by zenoh 1.9.0's router
  hat (above).

## 3. The envelope — `RouterTopologyV1`

Published to `ops/fabric/topology/<site>/<node>`, protobuf, canonical schema at
`ops/goat-topology/proto/router_topology_v1.proto`. One message per router per
interval. Shape:

| Field | Meaning |
|---|---|
| `zid`, `site`, `node` | Who is speaking. `node` is the segment in the key. |
| `ts_unix_ms`, `interval_secs` | When, and how often to expect the next one. |
| `zenoh_version` | From the adminspace, so a version skew is visible. |
| `gateway_mode` | `auto` \| `custom` \| `unreadable` — see §3.1. |
| `custom_subregions[]` | For `custom`: per subregion, its index and its filters, each filter carrying which of `modes` / `interfaces` / `zids` / `region_names` it uses and whether it is `negated`. |
| `regions[]` | Per region: name, link-state `V` and `E`, member ZIDs, the session count by `whatami`, and `links[]` — every edge of the region's link-state graph as a ZID pair, capped at 1024 with `links_truncated` set when hit. |
| `route_successor_count` | Observed, for the Σ V(V−1) cross-check. |
| `sessions[]` | Per session: peer ZID, `whatami`, assigned region, link endpoints. |
| `declarations` | The declarations census, from its own slower cycle (§2.6). Absent from an older publisher or one with the census off. |
| `next_hops[]` | Route-table entries per next-hop neighbour (§2.6). |
| `client_admission` | `admitted` \| `refused` \| `unspecified`. From the declared `gateway` block: whether any Custom subregion can match a default client (§2.4). `unspecified` under `auto`, `unreadable`, or from an older publisher, and then no finding fires. |

### 3.1 Two sources, because neither one is enough

Measured on zagros, 2026-09-04:

| Source | Gives | Verified |
|---|---|---|
| **Adminspace** `127.0.0.1:8000/@/local/**` | Observed regions, link-state graphs, route table, sessions | 737 keys returned, **none of them config** — it does not expose `gateway` at all |
| **Declared config** `/etc/<stack>/config.json5` on the router host | The `gateway` block as deployed | Readable, mode 0644; on zagros it contains **no `gateway` key**, independently confirming M-111's finding from a third angle |

So `gateway_mode` and `custom_subregions` come from the **file**, and everything
else from the **adminspace**. The publisher runs on the router host, so it has
both.

**The publisher extracts the `gateway` block and nothing else.** That config file
is ~900 KB and is overwhelmingly `access_control` — the goatnet's live ACL,
including slot anchors and subject CNs. None of it is read into the envelope,
logged, or published. A meter that shipped ACL content onto the bus would be an
identity leak dressed as observability. The parser targets one top-level key.

**Where a source is missing, say so.** If the config file is unreadable,
`gateway_mode` is **`unreadable`** — never inferred as `auto` from the absence
of a `south:` region. The asymmetry that makes this necessary: `Auto` and a
`Custom` list that happens to match nothing produce **the same observable
regions**, and the second is a real misconfiguration. Inferring from observation
alone would report a fault as healthy. This is [ADR 1036](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1036-environment-health-surface.md)'s
honesty rule: never a false green, and `unknown` is a first-class state.

## 4. The findings engine

Each finding is `{id, severity, subject, what, why, remediation}`. Severity is
`critical | warning | advisory | info`. Every finding names the router or pair
it is about — never "the fabric".

| ID | Fires when | Severity | Remediation offered |
|---|---|---|---|
| `T-001` region-filter-authz | A `custom_subregion` filter uses `zids` or `region_names` | **critical** | These are values the peer asserts about itself. They route; they do not authorize. Scope by certificate identity in the ACL instead. Never widen a region to grant access. |
| `T-002` empty-filter-list | A subregion has `filters: []` | **critical** | An empty list matches nothing, so the subregion is inert and every peer silently falls north — the opposite of the intent. Either populate it or remove the entry, remembering the region id is the list index. |
| `T-003` mode-drift | Routers in one goatnet disagree on `gateway_mode` | warning | Region config is per-router and a split is legitimate during a rollout. If it is not a rollout, converge them; state which routers differ. |
| `T-004` asymmetric-view | A's north graph contains B, and none of B's router regions (`north`, `south:<id>:router`) contains A | warning | One side has not settled, or a session dropped one-way. Re-check after one interval; if it persists, inspect that pair's transport. |
| `T-005` route-table-mismatch | `route_successor_count` ≠ Σ V(V−1) over that router's **edge-bearing** regions | warning | A persistent mismatch means the router is mid-settle, or the model is wrong for this topology — capture it, it is worth a finding. See §4.1 for why the edge condition is in the rule. |
| `T-006` stale-publisher | No snapshot within 3 intervals | warning | The router's view is **unknown**, not healthy. Check the publisher, not the router. |
| `T-007` version-skew | Routers report different `zenoh_version` | warning | The version-of-record is `=1.9.0` fleet-wide (`zenoh-version-pinning.md`); a skew opens the 1.9.0-vs-1.0.4 handshake-failure class. |
| `T-008` isolated-router | `V=1` and no non-client sessions | info | A single-router goatnet is a legitimate shape, not a fault. Stated so an empty graph is not misread. |
| `T-009` domain-growth | Σ V(V−1) across the goatnet, and what one more router costs | advisory | Not a problem below the thresholds M-112 bounds. Shown so the growth curve is visible before it matters, with the honesty that M-112 measured `V ≤ 3` and everything beyond is projection. |
| `T-011` unreadable-config | The declared config could not be read | warning | The mode is reported `unreadable`, never assumed `auto` — see §3.1. Give the publisher read access, or fix the path it was pointed at. |
| `T-012` not-reporting | A router appears in a reporter's router regions and publishes no snapshot | info | One finding for all of them, naming the reporter that sees most. Deploy the publisher if the router is ours; nothing to do for a partner's. A ZID identifies nothing — match a router to a host by its session's source address. |
| `T-013` clients-refused | Mode is `custom` and no subregion can match a client (`client_admission: refused`) | **critical** | Every default client of this router is refused. Append `{ filters: [ { modes: ["client"] } ] }` to the gateway list and restart; meanwhile give the publisher a fallback endpoint (§2.4). |
| `T-010` declared-vs-running | Mode is `custom` **and** the declared subregion indices are not the running ones | **critical** | The file on disk is not what the router is doing — almost always a config change without a restart. Merged is not deployed. Restart the router, or find out who edited the file out of band. Only decidable under `custom`; see §4.2. |

### 4.1 The successor model needed a correction that only live data could give

M-112 established `route/successor = Σ V(V−1)` over a router's regions, exact at
every point in the rig. Running this component's `inspect` against the **live
zagros router** on 2026-09-04 produced a route table of **2** where that formula
predicts **8** — so `T-005` would have fired a warning on a healthy production
router on every cycle.

The cause is the anomaly M-111
recorded and could not explain: zagros's `south:0:peer` graph carries **V=3 with
E=0**, nodes known through gossip that the router holds no link to. With no
links there are no paths, so that region contributes no successor entries
however many nodes it lists.

**The rule is therefore `V(V−1)` for regions with edges, and 0 for regions
without.** That fits every observation held — three live goatnets and both rig
arms:

| Source | Regions | Predicted | Observed |
|---|---|---|---|
| zagros (live) | north V2 E1; `south:0:peer` V3 E0 | 2 | 2 |
| zagros-prod (live) | north V3 E2; `south:0:peer` V1 E0 | 6 | 6 |
| nova (live) | north V1 E0; `south:0:peer` V1 E0 | 0 | 0 |
| M-112 gateway | north V3 E2; `south:0:router` V3 E2 | 12 | 12 |
| M-112 member | north V3 E2; `south:0:peer` V1 E0 | 6 | 6 |

M-112 is not wrong — every region in its rig was well-formed, so the rig could
not have surfaced this. It is the kind of refinement that only arrives when a
model meets production, and it is recorded here rather than left in the code
because the next person to reason about route-table size needs it.

### 4.2 `T-010` could not have been stated the way it first was

The first cut of `T-010` looked for a divergence by asking whether the mode was
`auto` while a `south:` region held more than one member — reading a populated
southern region as evidence someone had configured a `Custom` list the file no
longer showed.

That premise is wrong. **`Preset(Auto)` computes south regions itself**, at
runtime, from the mode pairs of the sessions it holds. Pointed at the live zagros
router on 2026-09-06 the rule fired **critical** on `south:0:peer` V=3 E=0
produced by an entirely empty `gateway` block — a healthy router, in the
fleet's only configuration. Every goat router runs `Preset(Auto)`
(M-111),
so this was a permanent false critical on all of them.

Under `Auto` there is nothing declared for the running state to diverge *from*,
so the only honest answer is silence. The check is decidable only under
`Custom`, where the file names subregions by list position and zenoh assigns
region ids from that same position (`region.rs:124-136`): the declared index set
and the running index set must match, and they part when the file is edited
without a restart — which is the fault the finding was written for.

This is the same shape as §4.1: a rule that held across every arm of the rig,
because every region in the rig was well-formed and every arm that had southern
peers was a `Custom` arm. Both corrections came from one router in production.
`auto_with_a_populated_south_region_is_not_a_divergence` in `findings.rs` pins
the live case.

**`T-001` is the reason this surface has a findings engine at all.** The
`gateway.south` filter list has the exact shape of an access control and two of
its five axes are peer-asserted. A surface that rendered the config without
saying so would make the hazard *easier* to reach. The engine therefore names it
as critical and offers the correct alternative in the same breath.

**What the engine will never do:** recommend `region_names` or `zids` as a way
to scope anything, or recommend a `Custom` layout as though it were tested at
scale. M-112 measured two regions of three routers on one host. The `T-009`
advisory says so on its face.

## 5. What a page view costs

Nothing on the fabric. The surface answers from memory. The only fabric traffic
this component generates is one small publish per router per interval — a
constant, independent of how many people have the page open. The router reads
behind it run on each publisher's own cycles (§2.6), never on a request.

This is the property to protect in review. Any future change that makes the
surface fetch on request converts it into the shape ADR 1123 exists to forbid.

## 6. Honesty rules

Inherited from [ADR 1036](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1036-environment-health-surface.md), restated
because they are load-bearing here:

- A router with no recent snapshot is **`unknown`**, rendered as such, never
  omitted and never green. An omitted router reads as "no problem".
- A router whose `gateway` config could not be read is **`unreadable`**, not
  `auto`.
- A router known only from another router's map is drawn, marked **not
  reporting**, and its unknown numbers are blank, not zero. It is counted in the
  router total, and the total is split into reporting and not reporting.
- Every rendered number carries its snapshot age.
- A finding states what was *observed*. Where a finding rests on a projection —
  only `T-009` does — it says so in the finding text, not in a footnote.

## 6a. The surface is born in the design system

It carries no palette of its own. `src/serve.rs` `include_str!`s
[`docs/design-system/colors_and_type.css`](../design-system/colors_and_type.css)
and `components.css` and splices them ahead of the page's own rules — the same
way flashlight consumes them, so there is one source of truth for the bytes and
a design-system change reaches this surface on the next image build rather than
drifting in a vendored copy ([`design-system.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/design/design-system.md)
§"How consumers receive the CSS bytes"). The Dockerfile therefore builds from
the **repo root**, not from the crate directory.

Two properties are asserted by tests rather than left to review:

- **The tokens actually land.** A failed splice would fall back to browser
  defaults and read as a CSS mistake rather than a build one, so the test
  checks the token bytes are present *and* that they precede the page's own
  rules.
- **The page makes no external request.** No CDN, no webfont, no analytics. A
  mesh-only admin surface must not attempt egress on page load — the design
  system states that as canon, having found a Google Fonts `@import` that could
  never have worked on a mesh-only surface. Type hierarchy here comes from size,
  weight and spacing, which work in any face.

What the visualization shows, and why each choice is information rather than
decoration:

| Element | Encodes |
|---|---|
| Node ring arc | Clients terminating at that router — weight at a glance, and they cost routing nothing |
| Edge colour | The region the session was assigned: north routes, south terminates |
| Animated edge flow | **North edges only** — the routed domain is the part carrying state |
| Node tag | The declared gateway mode, or `stale` when the snapshot is old |
| Region row `V/E` | An edgeless region is labelled "carries no routes" — the §4.1 correction, on the face of the table |

The graph layout is **deterministic, not force-simulated**. A goatnet has a
handful of routers, and a layout that settles differently on each refresh makes
an operator re-find the same node every time. Same input, same picture.

### 0.3 Names, and one click to everything about a router

A router is labeled so that two routers are told apart at a glance, and one
click on it gathers what the page holds about it.

**Labels.** Each router in the graph carries its lines outside the ring: its
node name and `<site> · <role>`, then, when the roster knows its owner,
`<organisation> · <organisation nation>` (§2.5). A router that sends no report
is labelled with its mesh peer name when the peer directory has it (§2.5). The role is read from what the
router reports: a router that holds other routers in a south group, or that
declares a custom gateway block, is a `gateway`; every other router is a
`member`. A router that is known only by its ZID, with no peer name, gets a display name derived
from the ZID, two words and the first six characters of the ZID, for example
`coral-pier (e1e1e1)` for the ZID `e1e1e1…`. The derivation is a hash of the ZID and nothing else,
so the name is the same on every load and for every viewer. It is display
only: nothing matches on it, and the ZID is always shown beside it.

**Router detail.** Selecting a router in the graph, or its row in the routers
table, opens one panel below the graph. Selecting it again closes the panel.

| Part | Content | Read from |
|---|---|---|
| Identity and state | site, node, ZID, Zenoh version, declared mode, time since the last report, connections held, route entries beside the predicted count, routing groups | the router's row in `routers` |
| Operated by | mesh peer, hostname, groups, address, organisation, organisation nation, individual and their nationality, or why none is known (§2.5) | the row's `peer`, `owner`, `address`, `name_note` |
| Connected to | every router it has a link with, whether the link is routed or terminated, and which end sits below the other; each entry selects that router | `edges` |
| Findings that name this router | severity, id and headline | `findings`, by subject |
| Changes it made | time and change | `changes`, by node |

A neighbor that has sent no report is listed with the words `not reporting`,
and selecting it shows its ZID, its derived display name, who operates it
(§2.5), the reporting routers whose routing map lists it (the payload's
`known_from`) and its links.

Past eight routers the ring becomes an ellipse half again as wide as it is
tall, every other label near its top and bottom sits one row further out with a
leader line to its node, and a label line is clipped at thirty characters (the
full text is in the tooltip, the table and the detail). The picture widens to
fit the longest label rather than letting it run off the edge.

The panel is a second reading of the payload the page already holds. It makes
no request, so a drill-down costs the fabric nothing
([ADR 1123](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1123-client-demand-must-not-amplify.md)). Adding a router to
the carve plan is a button in the panel; the planner's own selection chips work
as before.

## 6b. How the component is put together

One concern per module, and the dependency direction is a DAG — nothing points
back up:

```
wire      protobuf types                        (no deps)
assets    compiled-in static files              (no deps)
findings  the engine                            -> wire
store     what is known, and what changed       -> wire
view      the API projection                    -> findings, store, wire
serve     HTTP, routes, subscription            -> assets, store, view, wire
```

`store` deliberately does **not** know about `view`. An earlier cut had the
store building its own projection, which made state depend on presentation and
had the two importing each other. `serve` is the only orchestrator: it ingests,
asks whether anything changed, and installs the rebuilt payload.

The page is split the same way — `index.html` is markup, `app.css` is style,
and behaviour is ES modules (`app.js` plus one renderer per section, none of
which knows about any other). A test walks the import graph and fails if a
module is imported but not served, because that is a 404 at runtime and nothing
at build time.

### 6b.1 A request does no work

This is the property to protect in review, and it is stronger than the ADR 1123
minimum:

| | |
|---|---|
| **Write path** (a router publishes, ~every 30s) | Diff the fingerprint, run the findings engine, build the projection, serialize once, bump the ETag. |
| **Read path** (a browser polls, every 15s, × viewers) | Clone one `Arc`, or return `304`. No engine run, no serialization, no allocation of consequence. |

Three decisions make the read path that cheap, and each would be easy to undo
by accident:

- **The projection is built on write.** Building per request would redo
  identical work for every viewer, inside a lock every viewer contends for.
- **The payload carries no ticking values.** There is no `generated_ms` and no
  `age_secs` — routers carry an absolute `received_ms` and the browser
  subtracts. A value that moved every tick would change the bytes, change the
  ETag, and defeat the `304` while nothing had actually happened.
- **The staleness tick is edge-triggered.** It rebuilds only when the set of
  stale routers actually changes, not every ten seconds. A router going quiet
  still surfaces immediately; a settled fabric produces byte-identical payloads
  indefinitely.

Together: on a fabric where nothing is happening — the normal case — every poll
answers `304`, the browser parses nothing, and the page re-renders nothing.
Tests assert each leg, including that a quiet fabric keeps the same ETag across
repeated refreshes.

### 6b.2 The payload carries what the page reads, and nothing else

The wire snapshot holds every session a router has — one per
client at any scale. The page uses the count. Region member lists are likewise
resolved into edges server-side and never sent.

That is not micro-optimisation: session lists are the one part of this payload
that grows with the fabric, so shipping them would make the page heavier exactly
as a goatnet gets busier, for data nothing renders. A test asserts the payload
stays flat as client count goes from 1 to 500.

## 7. Deployment

Registered in [`ops/serve/services.yaml`](https://github.com/usareur-af/goatcore/blob/main/ops/serve/services.yaml) as a
`container`, so it inherits the Caddy/Traefik + mesh-DNS + isitup path every
hosted service uses. Mesh-internal and admin-gated like the other operator
surfaces; **never publicly exposed**.

Everything below is declared, not hand-run — the point of the wiring is that a
rebuild, a reboot or a rotation does not need anyone to remember this component
exists.

| Piece | Where |
|---|---|
| Image | `ghcr.io/<org>/goat-topology`, built by `.github/workflows/goat-topology-image.yml` **from the repo-root context** (the design-system `include_str!` needs it), digest-pinned in `ops/pins/fleet-pins.yaml` |
| Role | `roles/goat-topology`, one role with two modes; `goat_topology_modes` per group_vars. **Publish runs on every host whose router serves the adminspace REST plugin** — core, standby and pod routers alike; a router host without it shows as not reporting (§2.3). A peering edge (`goat-zenoh-edge`) loads no REST plugin, by design: its in-process session presents no certificate and matches no ACL subject. Edges are therefore always drawn not reporting, from the core router's map. `goat_topology_publish_fallback_endpoints` for a router that refuses clients (§2.4). Serve mode: `goat_topology_peer_directory_path`, `goat_topology_roster_path`, `goat_topology_goatnet`, `goat_topology_show_individual` (§2.5), mounted read-only and only when the file exists, passed as environment so an older pinned image ignores them |
| Playbook | `ops/ansible/topology-deploy.yml -e target=<group>` |
| Zenoh ACL | rule `infra-w-topology` + subject `infra-topology` in `roles/goat-zenoh/templates/config.json5.j2` |
| Certificate | a `goat-topology` row in the site cert manifest, minted by `ops/scripts/mint-site-certs.sh` |
| DNS + catalogue | an `isitup_services` row per goatnet; the mesh zone is **generated** from it |

### 7.1 Two prerequisites that fail silently if skipped

**The ACL grant.** The publisher writes `ops/fabric/topology/**`, which nothing
is permitted to do by default. Without the grant every publish is denied and
Zenoh denial is **silent** — the publisher runs happily, the surface
comes up healthy and empty, and the operator debugs the wrong component. The
grant is inert until a cert with the matching CN connects, so it costs nothing
on a goatnet not running the surface.

**The certificate CN.** The ACL matches `CN=goat-topology-trial-peer`. A leaf
with any other CN produces exactly the same silent nothing. The role therefore
runs `openssl x509 -subject` and **refuses to deploy** on a mismatch rather than
starting something that cannot work.

### 7.2 Registration happens in the same change as the deploy

Per F-335: a surface declared ahead of its deploy grades `unknown` and stays
invisible; one registered afterwards is a service nobody can find. So the
`isitup_services` row, the generated mesh-DNS zone and the re-signed name
snapshot all land together. Regenerating a zone invalidates that snapshot's
signature, so it is re-signed with the goatnet's own CA in the same change — an
unverifiable snapshot is worse than a stale one.

### 7.3 Rollout order

**ZAGROS sandbox → verify → zagros-prod.** Not simultaneous: this is a new
component, and the release ladder ([`release-graduation.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/release-graduation.md))
exists because "it worked on the first goatnet" is the cheapest evidence
available before production. The ACL change reaches each goatnet through an
ordinary `zenoh-deploy.yml` re-render; on a goatnet whose ACL is hybrid the
F-258 guard applies, so read it before re-rendering one.

**nova is deliberately not a target** (operator, 2026-09-05). Its
measurements below are retained because they are a record of what was observed
on 2026-09-02, and a measurement does not stop being true when its subject is
retired.

## 8. What this is not

- **Not a control plane.** It reads and renders. It changes no router config, and
  has no write path to any router. Remediation is text an operator acts on.
- **Not a mesh tool.** See §1's scope boundary.
- **Not an alerting system.** Findings are rendered, not paged. If a finding
  class earns paging it belongs in the existing Prometheus/Alertmanager path
  (`mgmt-stack-resilience-and-observability.md`),
  not in a web surface.
- **Not evidence about scale.** It shows what is running. The scaling claims it
  can make are bounded by M-111 and M-112 and nothing more.

## 9. Reuse outside goat: who runs a router, neutral wording, facts and policy

**Designed, not built** (2026-11-16). The surface is meant to run on any Zenoh
fabric, inside goat and outside it, as one public build. Three parts of it
still assume goat. This section is the design for each; the build lands on top
of the owner-from-record change (§2.5), which edits the same code.

### 9.1 One neutral, optional router directory

Today the names and owners of routers come from goat's own files: the
relay-health peer-names file, a goat roster, and goat's peer-name grammar
(`peer-<handle>-…`, `machine-<handle>-…`) with a `TOPOLOGY_GOATNET` token. No
one outside goat has those files or that grammar.

**Design.** One optional file, given by `--directory <path>`
(`TOPOLOGY_DIRECTORY`), JSON, that anyone can write by hand or generate:

```json
{
  "version": 1,
  "routers": [
    { "match": { "zid": "a1a1a1a1a1a1a1a1" },
      "name": "core-1", "operator": "Network operations",
      "organization": "Example Org", "labels": { "rack": "r2" } },
    { "match": { "address": "192.0.2.10" }, "name": "edge-3" },
    { "match": { "address": "198.51.100.0/24" }, "operator": "Partner B" }
  ]
}
```

| Field | Meaning |
|---|---|
| `match.zid` | The router's ZID, in full. A ZID is chosen by the router itself and proves nothing; the detail says so when a match was by ZID. |
| `match.address` | An IP address or CIDR block. Matched against the address a reporting router sees the router's session come from (the `link_dst` already in each snapshot), which is observed by our own router, not claimed by the other. |
| `name` | Shown in place of the derived name. A reporting router keeps its `--node` name; `name` is then shown beside it. |
| `operator`, `organization` | Free text, shown in the detail and, when any entry sets them, in one table column. |
| `labels` | Free-form key/value pairs, shown verbatim in the detail. |

Precedence: a `zid` match, then an exact address, then the longest CIDR. The
file is re-read when its modification time changes; an unreadable file or a
bad entry is listed on the page and in the log, and the rest of the file still
applies.

**With no directory configured, the page shows no owner column, no owner
fields in the detail, no owner state and no owner finding.** Routers are named
by their `--node` name, or by the name derived from their ZID for one that
sends no report. Today the page shows an empty "organization · nation" column
and "not known" in that case; that is the defect this removes.

**goat becomes an adapter.** The goat inputs (`--peer-directory`, `--roster`,
`TOPOLOGY_GOATNET`, and the owner block written from the record) are read by
one module that produces the same in-memory entries as the neutral file. Their
flag and variable names stay accepted unchanged, so a goatnet's role keeps
working with no edit. The owner findings that encode goat's rule that every
router has a recorded owner (T-014, T-015, T-016) run only when
`--require-owner` is set; the goat role sets it, so a goatnet sees the same
findings as before.

### 9.2 Words a reader outside goat can follow

Page and finding texts cite records only goat can open (ADR numbers,
`docs/design/zenoh-version-pinning.md`, `ops/pins/fleet-pins.yaml`, "the
goat-topology role") and say "goatnet". **Design:** every such text states its
reason in the text itself and says "fabric"; a link, where one helps, goes to a
public page. The texts to change: T-006 (steps name the publisher's default
REST address as if fixed), T-007 (steps name goat's pin file and pinning
procedure), T-008 ("a goatnet with one router"), T-012 (step names the goat
role), the page header comment, the CLI `about` line, and the planner caveat
("every goatnet today").

Where the page's words differ from upstream Zenoh's (region, subregion,
gateway, client), the README's "Words on the page" table states the mapping
today. Whether the page itself should switch to upstream's words is waiting on
the Zenoh team's answer in the public discussion.

### 9.3 Findings that are Zenoh facts, and findings that are a policy

| Finding | Kind | Design |
|---|---|---|
| T-001, T-002, T-010, T-011, T-013 | Fact about a router's `gateway` configuration | Always on. |
| T-004, T-006, T-008, T-012 | Fact about what the reports show | Always on. |
| T-005, T-009 | Our measured model of the route table, not an upstream statement | Always on; the text says the model is ours and measured. |
| T-007 | Fact: routers report different Zenoh versions | Always on, with neutral steps. **New, optional:** `--expect-zenoh-version <v>`; when set, a router on another version is also named, even when every router agrees. Unset by default and in the goat role. |
| T-003 | **Policy:** "every router uses the same gateway mode". A gateway with a custom `gateway.south` above routers on `auto` is a normal layout, and the generic-name rig raises T-003 for exactly that. | Runs only with `--expect-uniform-gateway-mode`; the goat role sets it, so a goatnet sees the same findings. |
| T-014, T-015, T-016 | **Policy:** every router has a recorded owner | Runs only with `--require-owner` (§9.1); the goat role sets it. |

No finding today compares a router against a pinned version; the exact pin
appears only in T-007's steps, which §9.2 rewrites.
