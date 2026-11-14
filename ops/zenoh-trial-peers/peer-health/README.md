# goat-peer-health

**Status:** built, not deployed.

A reader that runs beside a Zenoh router, follows the router's log and reports two faults on peers: a clock that runs ahead of the router's own, and a client that refuses the router's certificate. It is stage 1 (measure) of the peer-fault design in goatcore, [`docs/design/peer-fault-handling.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/peer-fault-handling.md) (ADR 1232). It carries two detectors: the clock (built, not deployed) and trust refusal (built, not deployed).

It enforces nothing and stores nothing. If it stops, the fabric behaves exactly as before.

## What it reads

When a Zenoh router receives a sample whose timestamp is more than its limit (500 ms by default) ahead of its own clock, it logs two lines:

```
<ts>  WARN  rx-1 ThreadId(5) uhlc: incoming timestamp from <zid> exceeding delta 500ms is rejected: <peer time> vs. now: <router time>
<ts> ERROR  rx-1 ThreadId(5) zenoh::net::routing::dispatcher::pubsub: Error treating timestamp for received Data (incoming timestamp from <zid> ...). Replace timestamp: ...
```

The reader counts the `uhlc` WARN line only, so each rejected sample is counted once. From it the reader takes the Zenoh ID (1 to 32 lowercase hex characters), the limit, and the offset (peer time minus router time). ANSI color codes are stripped. A uhlc line it cannot parse is counted as unparsed, and the reading then never grades green. The reader also picks up the router's own Zenoh ID from its `Using ZID:` start-up line, when that line is in the stream.

The Zenoh ID in a rejection line belongs to the router that stamped the sample. That router may sit behind a router that holds the session with ours. `--sessions-file` records which IDs hold a session directly, and `--peering` attributes every rejection to one partner.

### The trust-refusal detector

A peer that trusts a root certificate the router no longer presents refuses the router's certificate, ends the TLS handshake and holds no session. Its own container stays healthy, so its owner does not notice.

For each handshake a client ends this way, the router logs one line. Two shapes were measured on 14 Nov with Zenoh 1.9.0, a TLS router and a client that trusts another root:

```
(A) stock Zenoh 1.9.0, WARN at the default info level:
2026-11-14T16:45:52.337979Z  WARN acc-0 ThreadId(05) zenoh_link_tls::unicast: received fatal alert: UnknownCA. Hint: increase the system open file limit.

(B) a router build with a patched accept loop, DEBUG only:
2026-11-14T16:45:52.337979Z DEBUG acc-0 ThreadId(05) zenoh_link_tls::unicast: TLS handshake failed on accept: received fatal alert: UnknownCA
```

On build (B) at the default info level the router logs **nothing** for a refused handshake. Shape (B) appears only with `RUST_LOG=info,zenoh_link_tls::unicast=debug`. The router needs the alert line at its log level, or this detector sees nothing.

The reader counts any line from the `zenoh_link_tls::unicast` module that carries `received fatal alert: <Name>`, whatever the level word, the thread tag or the text between the module name and that phrase, with or without the `Hint:` tail, ANSI-colored or not. At debug level the same module logs one `Accepted TLS connection on <address>: <address>. Common Name: <name>.` line per accepted connection. That line, and every other line from the module, is an ordinary line: it is not counted and not unparsed. It takes the alert name (CamelCase, or `Unknown(<n>)`) and the line's leading timestamp; a line without a parsable timestamp is placed at its arrival. Each alert name is counted in the same sliding window as the clock detector. A line with the marker and no readable name is counted as unparsed, and the reading then never grades green.

**Logged or not.** The reader cannot tell "no client refuses our root" from "this router does not log refused handshakes". The deployer sets `--tls-alerts-logged` (`PEER_HEALTH_TLS_ALERTS_LOGGED=true`) when the router logs the alert line at its level: shape (A) on a stock router, or shape (B) with the debug filter above. While the flag is off and no alert line has been read since the reader started, the trust reading is `unmeasured`, never clean or green. The first alert line read proves the router logs them; the reading is then measured until the reader restarts.

**What it can say.** That some client refuses the router's certificate, with which alert, and how often.

**What it cannot say.** Which client. The line carries no client address and no Zenoh ID, also with `zenoh_link_tls=trace,zenoh_link_commons=trace,zenoh_transport=debug`. The reader therefore never blames a peer for this fault: it names **candidates**. It also cannot tell an accepted connection from one our router opened, since `zenoh_link_tls::unicast` logs both; the measured line came from an accept task (`acc-0`), and the reader does not rely on the thread tag.

**The trust-refusal set** is `UnknownCA`, `BadCertificate` and `CertificateUnknown`: the alerts rustls and OpenSSL clients send when they do not accept the certificate chain the server presents (rustls sends `UnknownCA` for an unknown issuer). Every other name, for example `CertificateExpired`, `CertificateRevoked` or `DecryptError`, is counted and reported with `trust_refusal = false`, and grades nothing: an expired certificate can be the client's clock or our renewal, which is a different fault.

**Candidates.** A declared peer is *absent* when it is in the peers file and holds no session per the sessions file. A peers-file entry keyed by Zenoh ID is judged against the Zenoh IDs in the sessions file. An entry keyed by source address is judged only when the sessions file also lists source addresses (entries that are not Zenoh IDs); otherwise its presence is unknown and it is never a candidate.

| Trust-refusal alerts in the window | Absent declared peers | Rows | `trust_verdict` |
|---|---|---|---|
| yes | one or more | one `TRUST_ROOT_REFUSED_CANDIDATE` row per absent declared peer, grade `candidate`, color `amber` | `candidates` |
| yes | none, or no peers file, or no sessions file | none; site-level counts only. The reason says a client is refusing the router's certificate and cannot be named from the router's log | `unattributed` |
| no | any | none. Absence alone is not this fault | `clean` |
| (reading unmeasured, or the router not known to log alerts) | any | none; the counts read so far are still reported | `unmeasured` |

A candidate row carries the Zenoh ID, or `zid = ""` and `source_address` for an entry keyed by address; the name and owner from the peers file; `direct_session = false`; and `absent_since_unix_ms`, the first reading at which this reader saw the peer hold no session. That time is measured since the reader started, or since it could last judge the peer (the sessions file was readable). It is not persisted, so the peer may have been absent longer.

**No verdict document for a candidate.** The reader publishes nothing on `ops/peer/verdict/<zid>` for a candidate. A verdict tells a peer that it is at fault; the router's log cannot show which client refused, so a candidate is not a verdict. A candidate row disappears when the alerts leave the window or the peer holds a session again, with no `cleared` row.

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

A deployment does not pass the router's whole log through. It feeds a file that holds three kinds of line:

- the uhlc rejection lines (`uhlc: incoming timestamp`);
- the TLS fatal-alert lines: any line from `zenoh_link_tls::unicast` that carries `received fatal alert`, in either shape (the extended pattern `zenoh_link_tls::unicast.*received fatal alert` keeps both, also when ANSI codes sit after the module name);
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
| `--sessions-file <path>` | `PEER_HEALTH_SESSIONS_FILE` | | JSON list of the Zenoh IDs that hold a session with our router; re-read every tick. Entries that are not Zenoh IDs are read as session source addresses (`10.0.0.9` or `10.0.0.9:7447`) |
| `--peering <name>` | `PEER_HEALTH_PEERING` | | The one peering this reader serves (beside a per-partner edge router). 2 to 16 characters of `a-z`, `0-9` and `-`; the reader exits on any other name |
| `--observed-by <zid>` | `PEER_HEALTH_OBSERVED_BY` | from the log | Our router's Zenoh ID |
| `--tls-alerts-logged` | `PEER_HEALTH_TLS_ALERTS_LOGGED` | off | The router logs refused TLS handshakes at its log level. Off and no alert line read yet: the trust reading is `unmeasured` |
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

For the clock detector, entries keyed by source address match nothing yet: joining a Zenoh ID in a rejection line to its source address needs the router's admin space, which is a later increment. For the trust detector, an entry keyed by address is judged present or absent when the sessions file lists source addresses.

## How a reading is graded

The reader grades a reading before it blames any peer. The clock verdicts are checked in this order:

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

### How the two detectors combine

`verdict` and `overall_color` combine both detectors. Worst grounded wins:

1. `unmeasured` overrides everything (`unmetered`), as before. The trust detector adds no candidate rows while the reading is unmeasured.
2. Otherwise the worse color wins: red > amber > green. Trust refusal is never red, because no peer is confirmed.
3. When the trust reading is `candidates` or `unattributed` and the clock verdict is `clean`, the verdict becomes `trust_refused_candidates` or `trust_refused_unattributed` (amber).
4. When the trust reading is not measured (router not known to log alerts) and the clock verdict is `clean`, the verdict becomes `trust_unmeasured`. The color is `unmetered` when the clock alone would read green, and stays amber when the clock reads amber. Never green.
5. On any other clock verdict (`peer_at_fault`, `local_clock_suspect`, `recovering`) the clock word and color are kept and the reason adds `; also: ` and the trust reason. `trust_verdict` always carries the trust detector's own word.

Alerts with no candidate therefore read amber, `unattributed`. The clock's unmeasured rules are unchanged.

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
- candidate rows only: the kind `TRUST_ROOT_REFUSED_CANDIDATE`, `source_address` for an entry keyed by address, and `absent_since_unix_ms`. `first_seen_unix_ms` repeats the absent-since time and `last_seen_unix_ms` is the reading's time; the offset and rejection fields are zero

The detail also carries, for the trust detector:

- `tls_alerts`: one entry per alert name received in the window, sorted by name, with `count_in_window`, `per_minute`, `trust_refusal` and `last_seen_unix_ms`. It is a fact about the router, not about a named peer
- `trust_verdict`: `clean`, `unattributed`, `candidates` or `unmeasured`
- `declared_peers_absent`: how many declared peers hold no session, whether or not alerts arrive; absent when no declared peer can be judged
- `unparsed_lines_window` counts unparsed uhlc and TLS-alert lines together

`ops/peer/verdict/<zid>` carries one JSON document per peer at fault for a clock (never for a trust-refusal candidate), published every interval while the fault lasts, and once more when it clears:

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
- The remaining detectors in the design (certificate close to expiry, connect-and-drop loops, publishing above the envelope).
- Naming the client that refuses the router's certificate. The router's log cannot; it would need the network (source addresses of new connections to the router port compared with the session list) or a Zenoh change that logs the peer address with the alert.
- Writing source addresses into the sessions file (the feed's job).
- Probation entries and owner notices.
- An Ansible role and a compose template.

## Build and test

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

`tests/fixture_replay.rs` sends two recorded router logs through the file follower, the detector and a capturing publisher, and decodes what would have been published: `tests/fixtures/router.log` (clock only) and `tests/fixtures/router-mixed.log` (clock rejections and TLS alerts, with made-up IDs). The clock-only log never shows an alert line and the router is not declared to log one, so its clean reading ends as `trust_unmeasured`.
