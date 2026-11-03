# Reticulum bridge — extending goat below the IP line

**Status:** Bridge pattern **proven** (bidirectional, no-hardware) in the sibling repo
`goat-reticulum`; field RF
bearers + goat-identity mapping designed-not-built. [ADR 1050](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1050-reticulum-denied-edge-bridge.md).

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
IP. The bridge is the seam, and goat does **not** reimplement Reticulum.

The capability lives in a sibling repo (it pulls a Python RF stack neither goat
core nor ice-goat should depend on) and is positioned as the first reference
**denied-edge southbound gateway** for ice-goat.

## The gap, precisely

| goat resilience claim | True when | Breaks when |
|---|---|---|
| Tunnels persist through mgmt outage | always (P2P established) | — |
| Same-LAN peers keep talking offline | IP on the LAN | no IP fabric at all |
| "Tactical edge runs itself" | **IP reachable between peers** | bearer degrades below IP (RF-only, LoRa, denied) |

The first two are real and built. The third has an unstated IP dependency. The
Reticulum bridge is what closes it.

## Design

A single bridge process holds both stacks and translates between them.

- **Reticulum side:** a persistent `RNS.Identity`, one `IN`/`SINGLE`
  `Destination` (`goat_reticulum.bearer`) that edge peers establish an encrypted,
  forward-secret, bidirectional `Link` to. Inbound packets are republished onto
  Zenoh; outbound Zenoh samples are sent as packets over the active Link(s).
- **goat (Zenoh) side:** a Zenoh peer that subscribes an **egress** key space
  (`goat/rns/egress/**`, data bound for the denied edge) and publishes
  edge-sourced data into an **ingress** key space
  (`goat/rns/ingress/<edge-identity-hash>`), so a proven Reticulum identity
  becomes a first-class, attributable key on the fabric.
- **The bearer is opaque.** The Reticulum *interface* (TCP loopback in the proof;
  `RNode`/LoRa, KISS/packet-radio, or serial in the field) is config, not bridge
  code — the same bridge runs over real RF unchanged.

Architecture detail, key spaces, and field-deployment shape live in the sibling
repo's `docs/architecture.md`.

## What is proven

A self-contained proof (`make proof`, no RF hardware, no live fabric) spins up a
bridge, a Reticulum-only edge node, and a goat-side Zenoh peer on loopback and
asserts data crosses the boundary **in both directions**, against current `rns`
1.3.5 and `eclipse-zenoh` 1.9.0 (the goat fleet pin):

- **edge → goat** (Reticulum → Zenoh): the edge's RNS packet is republished onto
  the ingress key, labeled by the edge's proven Reticulum identity hash.
- **goat → edge** (Zenoh → Reticulum): a fabric sample is forwarded over the
  Link and received by the Reticulum-only node.

The proof now covers **both** a single small packet **and** a large payload that
must travel as a chunked `RNS.Resource` (a 50 KB ≈ 108-part transfer), each way —
four crossings, three consecutive runs all PASS; the edge identity propagates
end-to-end onto the Zenoh key, not just the bytes. Detail:
`docs/proof.md`.

### Large payloads — built (`RNS.Resource`)

Payloads above the ~383-byte single-packet ceiling (`ENCRYPTED_MDU`) now travel as
a chunked, reassembled `RNS.Resource` automatically; small payloads stay
single-packet. The choice is transparent to callers (`goat_reticulum.transport`)
in both directions — the analog of goat's `volatile/blob` chunking. Proven over
loopback; the field test confirms it over the real RF bearer, where chunk timing
and loss bite.

## What is not built (the gate to canon)

- **A real RF bearer.** The proof uses a TCP loopback interface. Field use swaps
  it for a LoRa `RNode` / packet-radio / serial interface — config, not code, but
  unproven on hardware. Hardware tiers + BOM:
  `docs/hardware-proveout.md`.
- **Multi-edge routing.** The proof broadcasts egress to all active Links;
  per-edge routing by egress key suffix is unbuilt.
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

- It pulls a Python RF stack (`rns`, `lxmf`) that neither goat core (Rust/Zenoh)
  nor ice-goat should take as a dependency.
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
