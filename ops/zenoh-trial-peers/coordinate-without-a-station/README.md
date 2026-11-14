# coordinate: systems share tasks when their station is gone

A station hands a small, fixed set of tasks, such as sectors to watch, to the systems in reach. When the station goes, the systems keep every task covered with nothing in the middle. Each system watches who is alive and runs the same assignment over that set, so they agree without sending a message to agree.

This is the worked example for the goat gym module *Let autonomous systems coordinate when no central station is reachable*.

## What it promises, and what it does not

| Situation | Handled here? | Why |
|---|---|---|
| The station, or the link to it, is gone, and the systems still share their site's router | Yes | Zenoh routers are edge-local, so a site cut off from every other site keeps its local pub/sub (goatcore `docs/design/zenoh-data-fabric.md`, "The topology as deployed"). |
| No router is in reach, and the systems must talk directly | No | Router-less peer continuity is designed, not built (goatcore ADR 1095). With no IP path at all, the bearer is Reticulum (ADR 1050), whose field bearers are not built either. |

## The rule

| Piece | What the example does |
|---|---|
| Who is alive | Each system holds a liveliness token at `<slot>/presence/coord/<group>/systems/<system>`, the presence convention in goatcore `crates/panoscope-core/src/presence.rs`. A token goes when its session closes or crashes, so no clock is involved. |
| Who covers what | Rendezvous hashing: each task goes to the live member with the highest sha256(task, member). Views that agree give the same answer. When a member leaves, only its tasks move; when one joins, only the tasks it now wins move to it. |
| A partition | Each side covers every task itself. A task may be covered twice, never by nobody. When the sides see each other again they recompute, and the duplicates drop. |
| The station | While the station's token is alive and it has published an assignment, the systems follow it. A task it gave to a member that is gone falls back to the rule at once. |
| Trust | Each participant publishes only under its own slot, and the per-slot write rule makes that slot an identity. A system accepts an assignment only from the slot that holds the station token. |
| A silently denied token | A router without the liveliness grant for a slot denies the token without an error. A system takes no task until it sees its own token in its own watch, and otherwise exits saying why. |

Each system publishes what it covers on one stable key, `<slot>/coord/<group>/<system>/covering/v1`. It publishes on every change and every `--heartbeat` seconds. The payload carries `mode`, which is `station` or `self`, along with `covering`, `live`, `seq` and `at`.

## Run it

Offline, to see the rule:

```bash
python3 coordinate.py plan --tasks north,east,south,west \
  --live s1/alpha,s2/bravo,s3/charlie
```

On a goatnet, each participant needs its own slot's bundle: the cert, the key and the CA root, in one directory. Start the systems and the station as clients of the same site router:

```bash
python3 coordinate.py system  --bundle <alpha-bundle> --endpoint tls/<site-router>:7447 \
  --group line-3 --system alpha --tasks north,east,south,west
python3 coordinate.py station --bundle <station-bundle> --endpoint tls/<site-router>:7447 \
  --group line-3 --tasks north,east,south,west
```

Watch from any enrolled machine:

```bash
goat presence --watch --key '*/presence/coord/line-3/**'
goat sub '*/coord/line-3/*/covering/v1'
```

The example needs the Zenoh client library, `pip install eclipse-zenoh==1.9.0`, the version the map surface pins. It uses that library rather than `goat pub` because no `goat` verb declares a liveliness token.

## Tests

```bash
python3 -m pytest -q ops/zenoh-trial-peers/coordinate-without-a-station
```

The rule is tested over every live set drawn from a roster of six systems and nine tasks:
- every task has exactly one owner;
- a leave moves only the leaver's tasks, and a join moves tasks only to the joiner;
- each side of a partition covers every task;
- a merge returns to one owner per task.

The event handling is tested without a fabric. It waits for the system's own token, follows the station, refuses an assignment from another slot, and covers a task the station gave to a member that has gone.

One test starts real Zenoh sessions on loopback when the client library is installed. A router-mode session stands in for the site router, with the station and three systems as clients. It checks that every task is covered under the station and again after the station stops. It uses no certificates and no goatnet, so it shows the code works against the library, not against a goatnet's router, ACL or certificates. The example has not been run on a goatnet.
