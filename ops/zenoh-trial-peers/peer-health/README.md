# goat-peer-health

**Status:** built, not deployed.

A reader that runs beside a Zenoh router, follows the router's log and reports peers whose clocks run ahead of the router's own. It is stage 1 (measure) of the peer-fault design in goatcore, [`docs/design/peer-fault-handling.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/peer-fault-handling.md) (ADR 1232). This first increment carries one detector: the clock.

It enforces nothing and stores nothing. If it stops, the fabric behaves exactly as before.

## What it reads

When a Zenoh router receives a sample whose timestamp is more than its limit (500 ms by default) ahead of its own clock, it logs two lines:

```
<ts>  WARN  rx-1 ThreadId(5) uhlc: incoming timestamp from <zid> exceeding delta 500ms is rejected: <peer time> vs. now: <router time>
<ts> ERROR  rx-1 ThreadId(5) zenoh::net::routing::dispatcher::pubsub: Error treating timestamp for received Data (incoming timestamp from <zid> ...). Replace timestamp: ...
```

The reader counts the `uhlc` WARN line only, so each rejected sample is counted once. From it the reader takes the Zenoh ID (1 to 32 lowercase hex characters), the limit, and the offset (peer time minus router time). ANSI color codes are stripped. A uhlc line it cannot parse is counted as unparsed, and the reading then never grades green. The reader also picks up the router's own Zenoh ID from its `Using ZID:` start-up line, when that line is in the stream.

The Zenoh ID in a rejection line belongs to the router that stamped the sample. That router may sit behind a router that holds the session with ours. `--sessions-file` records which IDs hold a session directly, and `--peering` attributes every rejection to one partner.

## How to feed it

From a container's log, starting with recent lines only:

```bash
docker logs -f --since 1m <router> 2>&1 | goat-peer-health --stdin --site example-site \
  --endpoint tls/10.0.0.1:7447 --cert leaf.pem --key leaf.key --root-ca ca.pem
```

From a file, which the reader follows the way `tail -F` does. It survives rotation and truncation, and waits for a file that does not exist yet:

```bash
goat-peer-health --log-file /var/log/zenoh/router.log --site example-site --endpoint ...
```

The reader never opens the docker socket. To see what it would publish without a router, add `--dry-run`. It then prints one JSON line per put on stdout and logs to stderr.

When standard input ends, for example because the router container went away and `docker logs -f` exited, the reading turns `unmeasured` and stays that way. Restart the pipe to measure again.

A deployment does not pass the router's whole log through. It feeds a file that holds two kinds of line:

- the uhlc rejection lines only;
- one heartbeat line every 30 s.

Any line that is not a rejection proves the feed is alive. Without the heartbeat, a quiet router would read `unmeasured` once `--silence-after-secs` passes.

The router's `Using ZID:` start-up line is not in such a feed, so the deployment sets `PEER_HEALTH_OBSERVED_BY` to the router's Zenoh ID.

## Flags and environment

| Flag | Environment | Default | Meaning |
|---|---|---|---|
| `--stdin` | | | Read the log from standard input |
| `--log-file <path>` | `PEER_HEALTH_LOG_FILE` | | Follow a log file (one of the two inputs is required) |
| `--from-start` | `PEER_HEALTH_FROM_START` | off | With `--log-file`, read the file from its start instead of its end |
| `--site <name>` | `PEER_HEALTH_SITE` | required | The `<site>` in `ops/peer/health/<site>` |
| `--endpoint` | `PEER_HEALTH_ENDPOINT` | required unless `--dry-run` | Router endpoint, e.g. `tls/10.0.0.1:7447` |
| `--cert`, `--key`, `--root-ca` | `PEER_HEALTH_CERT`, `PEER_HEALTH_KEY_FILE`, `PEER_HEALTH_ROOT_CA` | | mTLS client material; required for a `tls/` endpoint (same as goat-time-health) |
| `--interval-ms` | `PEER_HEALTH_INTERVAL_MS` | 30000 | Publish interval |
| `--window-secs` | `PEER_HEALTH_WINDOW_SECS` | 60 | Sliding window for rate and min/max offset |
| `--clean-after-secs` | `PEER_HEALTH_CLEAN_AFTER_SECS` | 600 | A peer with no rejection for this long is reported once as cleared |
| `--silence-after-secs` | `PEER_HEALTH_SILENCE_AFTER_SECS` | 900 | No log line of any kind for this long makes the reading unmeasured |
| `--peer-population` | `PEER_HEALTH_PEER_POPULATION` | 0 | Declared number of peers this router serves; the denominator of the majority rule |
| `--peers-file <path>` | `PEER_HEALTH_PEERS_FILE` | | JSON map from Zenoh ID (or source address) to `{"name", "owner"}` |
| `--sessions-file <path>` | `PEER_HEALTH_SESSIONS_FILE` | | JSON list of the Zenoh IDs that hold a session with our router; re-read every tick |
| `--peering <name>` | `PEER_HEALTH_PEERING` | | The one peering this reader serves (beside a per-partner edge router) |
| `--observed-by <zid>` | `PEER_HEALTH_OBSERVED_BY` | from the log | Our router's Zenoh ID |
| `--local-clock` | `PEER_HEALTH_LOCAL_CLOCK` | `auto` | `auto` asks the kernel (adjtimex); `synced` or `unsynced` overrides it |
| `--dry-run` | | off | Print instead of publishing; open no session |

`RUST_LOG` sets the log level (default `info`).

If `PEER_HEALTH_ROOT_CA` did not issue the router's certificate, the reader exits with status 1 after logging `Unable to connect to tls/<host>:<port>! Can not create a new TLS link bound to <address>: invalid peer certificate: UnknownIssuer`. The router logs `received fatal alert: UnknownCA`.

Peers file example:

```json
{
  "3c9a51e0d2b7f4a6c18e05d9b3a7f2e1": { "name": "lab-router-b", "owner": "example partner" },
  "10.0.0.7": { "name": "lab-router-c" }
}
```

Entries keyed by source address are accepted but match nothing yet. Joining a Zenoh ID to its source address needs the router's admin space, which is a later increment.

## How a reading is graded

The reader grades a reading before it blames any peer. The verdicts are checked in this order:

| Verdict | When | `overall_color` | Per-peer verdicts published |
|---|---|---|---|
| `unmeasured` | No log line for `--silence-after-secs`, no line yet, or the input ended | `unmetered` | none; no peer is cleared while unmeasured |
| `local_clock_suspect` | Our host clock is not synchronized or could not be checked, or more than half of the known peers show an offset in the same direction | `amber` | none |
| `peer_at_fault` | One or more peers had a timestamp rejected in the window | `red` | one per peer at fault |
| `recovering` | Peers rejected earlier, none in the window, clean period not yet passed | `amber` | none |
| `clean` | Nothing tracked | `green` (`amber` if uhlc lines did not parse) | a `cleared` verdict once for each peer that had been told |

Rules behind the table:

- **Known peers.** The count of known peers is the number of distinct Zenoh IDs across the peers tracked, the peers file and the sessions file, or `--peer-population` if that is larger. A router with 100 sessions and 2 fast peers therefore blames the 2 peers. Without a sessions file or a declared population, 2 fast peers out of 2 known read as our clock.
- **Majority rule.** The rule needs at least two known peers. With `--peering` it does not apply, because every stamping router then belongs to one partner network.
- **Local clock.** The reader treats its host's clock as the router's clock, because it runs on the same host. The `auto` check reads the kernel's `STA_UNSYNC` flag through `adjtimex`, which needs no privilege. If the call is refused, for example by a seccomp profile, the clock reads `unknown` and nobody is blamed. Set `PEER_HEALTH_LOCAL_CLOCK=synced` only on a host whose sync is verified by other means.
- **Event times.** A rejection is placed in the window at the router's own "now" from the line. A replayed history therefore lands in its real place and is not counted as new.

A router at `RUST_LOG=info` can log nothing for long stretches. On such a router, a peer that stops being rejected is cleared after `--clean-after-secs`. If the log then stays silent past `--silence-after-secs`, the reading turns `unmeasured`.

## Keys published

`ops/peer/health/<site>` carries `EnvHealthV1{domain:"peer"}` with `PeerFaultDetailV1` (arm 17), protobuf. The canonical schema is [`../time-health/proto/env_health_v1.proto`](../time-health/proto/env_health_v1.proto). Each peer row carries:

- the Zenoh ID, the fault kind (`CLOCK_AHEAD`) and the limit
- the latest, minimum and maximum offset in the window
- rejections per minute, and the count in the window
- when the peer was first and last seen
- `observed_by`: our router's Zenoh ID
- the attribution name and owner, when known
- `direct_session`, when known
- the row's grade and color

`ops/peer/verdict/<zid>` carries one JSON document per peer at fault, published every interval while the fault lasts, and once more when it clears:

```json
{"schema":"goat.peer.verdict.v1","fault":"clock_ahead","state":"at_fault","offset_seconds":17.70,
 "limit_seconds":0.5,"rejections_per_minute":12.0,"observed_at":"2026-11-13T13:19:10.000Z",
 "observed_by":"5be08d2f41a97c36e0d1f8b2a4c6e9d7",
 "sentence":"the router sees your clock 17.7 s ahead of its own; the limit is 0.5 s"}
```

With `--peering <name>`, the reader also publishes one document on `ops/peer/verdict/peering/<name>`. It carries the largest offset in the partner network and the list of stamping Zenoh IDs. Making that key readable across a boundary node, as a released key, is deployment configuration and is not part of this crate.

## Not in this increment

- The neighbor-observed row in the `time` domain (`ops/time/health/<site>`).
- Joining a Zenoh ID to its source address through the router admin space, and writing the sessions file.
- Persisting first-seen across restarts. The value is reset when the reader restarts.
- The other detectors in the design.
- Probation entries and owner notices.
- An Ansible role and a compose template.

## Build and test

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

`tests/fixture_replay.rs` sends a recorded router log (`tests/fixtures/router.log`, made-up IDs) through the file follower, the detector and a capturing publisher. It then decodes the documents that would have been published.
