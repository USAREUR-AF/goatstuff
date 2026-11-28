# Reticulum bridge — extending goat below the IP line

**Status:** Bridge pattern accepted ([ADR 1050](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1050-reticulum-denied-edge-bridge.md)). The first bridge (Python, on `rns`/`lxmf`) is removed because of the Reticulum License (ADR 1050 Amendment 1). The rebuild in Rust on Reticulum-rs is in progress, not merged; which repository holds it is to be decided. Field RF bearers and goat-identity mapping are designed-not-built.

## Conclusion

goat's edge-resilience story holds through a **management-plane** outage — P2P
tunnels persist, same-LAN peers keep talking — but it assumes **IP reachability
between peers**. WireGuard P2P and relay fallback both need IP. Below the IP line
(RF, LoRa, packet radio, serial, true infrastructure denial) goat has no bearer.
That is the one honest gap in "the tactical edge runs itself": it runs itself
only where IP survives.

[Reticulum](https://reticulum.network) (RNS) is purpose-built for that regime:
coordination-less, cryptographic-identity addressing (no IP, no DNS, no central
authority), with transport interfaces over almost any medium down to a few bits
per second. The two stacks are **complementary, not competing** — goat governs
attributable, audited sharing above IP; Reticulum carries bits where there is no
IP. The bridge is the seam.

The Python reference implementation (`rns`, `lxmf`) is under the Reticulum
License from 0.9.4, whose condition forbids use "in any kind of system which
includes amongst its functions the ability to purposefully do harm to human
beings". goat does not use it in any form. The bridge runs on Reticulum-rs
(MIT), with the pieces it lacks written clean-room. Interop with Python `rns`
networks is not a goal. The capability lives in a sibling repo and is
positioned as the first reference **denied-edge southbound gateway** for
ice-goat.

## The gap, precisely

| goat resilience claim | True when | Breaks when |
|---|---|---|
| Tunnels persist through mgmt outage | always (P2P established) | — |
| Same-LAN peers keep talking offline | IP on the LAN | no IP fabric at all |
| "Tactical edge runs itself" | **IP reachable between peers** | bearer degrades below IP (RF-only, LoRa, denied) |

The first two are real and built. The third has an unstated IP dependency. The
Reticulum bridge is what closes it.

## Design

A single bridge process holds both stacks and translates between them: a
Reticulum identity and one `SINGLE` destination (`goat_reticulum.bearer`) that
edges open an encrypted, forward-secret Link to, and a Zenoh peer on the
egress and ingress key spaces. The Reticulum interface is configuration, not
bridge code, so the same bridge runs over any bearer.

## The rebuild in Rust

One Rust process holds a Zenoh peer (`zenoh` =1.9.0, the fleet pin) and a
Reticulum destination on Reticulum-rs.

| Direction | Zenoh key | On the bearer |
|---|---|---|
| goat -> every edge | `goat/rns/egress/<key>` | the edge receives `<key>` |
| goat -> one edge | `goat/rns/egress/<edge-identity-hash>/<key>` | that edge receives `<key>` |
| edge -> goat | `goat/rns/ingress/<edge-identity-hash>/<key>` | the edge sends `<key>` |

- An edge proves its Reticulum identity on the Link; data from an edge that has
  not is dropped and counted. The ingress key carries the proven identity hash.
- Messages travel over a Reticulum Channel (reliable, ordered, windowed). A
  message larger than one packet is split into chunks sized for the bearer MTU
  and reassembled in order.
- Interfaces: TCP and UDP from Reticulum-rs; serial (HDLC framing), KISS (tty or
  KISS TCP port) and RNode (KISS plus radio setup, every radio parameter
  required) written clean-room.
- The proof runs bridge and edge as separate processes with a goat-side Zenoh
  peer over TCP, lossy UDP, or a serial or KISS null-modem, optionally
  rate-limited and delayed. It asserts both directions for a one-packet and a
  chunked (50 KB) message, sends a run of small messages each way, and fails if
  the bridge's own p99 handling time per message exceeds 1 ms.

## What is not built (the gate to canon)

- **A real RF bearer.** The proof uses simulated bearers. The RNode and KISS
  interfaces are unproven on hardware.
- **goat-identity mapping.** Mapping a proven Reticulum identity to a goat slot
  anchor so edge-sourced data carries full goat attribution (alias/ACL) is
  designed-not-built.
- **Rate-shaping onto a scarce bearer.** Egress onto a contested RF link must be
  a low-rate, prioritized subset of fabric traffic — ties into the QoS work
  ([ADR 1033](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1033-zenoh-qos-prioritization-reserved-mission-slice.md)),
  unbuilt here.

## Relationship to ice-goat

ice-goat's southbound adapter contract
is the published plug-point for transport/RF/MANET gateways. goat-reticulum is
the first reference **denied-edge** gateway against it, and the realized
lower-cost answer to ice-goat's "Zenoh over serial/RF" tracked dependency:
because **Zenoh stays on IP** and the cross-to-RF hop is Reticulum's job behind
the adapter, there is **no zenohd fork and no non-default Zenoh cargo feature** —
none of the build-from-source cost that the `transport_serial` shape would
reopen ([ADR 0858](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0858-zenoh-image-upstream-pivot.md)). Mapping detail in
the sibling repo's
`docs/ice-goat-tie-in.md`.

## Why a sibling repo, not goat core

- It carries a Reticulum stack (Reticulum-rs, vendored and patched) that neither
  goat core nor ice-goat should take as a dependency.
- It is a **reference** bearer — one of potentially many — so it lives beside the
  contract it implements, per the sibling-repo containment pattern
  ([ADR 0217 addendum](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0217-shim-repo-containment.md)) and the
  demand-gated radio tier posture (cf. ice-goat itself).

## Cross-references

- [ADR 1050](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1050-reticulum-denied-edge-bridge.md) — the decision.
- `goat-reticulum` — the capability + proof.
- `ice-goat` `docs/interfaces/southbound.md` — the adapter contract.
- [`offline-enrollment.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/offline-enrollment.md), [`bootstrap-bundle.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/bootstrap-bundle.md), `ha-failover.md` — the above-IP resilience this complements.
- [`zenoh-volatile-class.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/zenoh-volatile-class.md) — the `volatile/blob` chunking model the large-payload roadmap mirrors.
