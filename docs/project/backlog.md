# Backlog

Work that is decided or needed and not built. One entry per item: what, why,
what it needs, and the design document that owns it.

| Item | Why | Needs | Design |
|---|---|---|---|
| Messages and bytes per router, and between two routers, on the topology page | The page shows declarations (subscriptions, publishers, topics) but no traffic; an operator cannot tell a busy router from an idle one with the same declarations | A zenoh router built with the `stats` feature, so `@/<zid>/router/metrics` carries per-transport message and byte counters; then a publisher field and a page column. The fleet build's metrics hold only `zenoh_build_info` | [zenoh-topology-surface.md §2.6](../design/zenoh-topology-surface.md) |
| Every mesh-key mint path records an owner and the key id (goatcore ADR 1249 decision 3) | The owner of a peer is found today by the name in the peer, so the link from peer to owner is recorded only after its first hop; a hand-made key can carry any name | The portal stores the key id on the machine slot; the person mint writes `netbird.setup_key_ids`; the bootstrap and bench key tools take `--owner` and record the key id; the reconcile then matches by key id | [zenoh-topology-surface.md §2.5a](../design/zenoh-topology-surface.md) |
| A declared-owner registry for our own infrastructure peers (ADR 1249 decision 2) | Management, relay, DNS and similar hosts carry no owner a resolver reads, so they show as "no record" or under an owner exception | A per-goatnet registry in the overlay that the portal resolver reads (`owner.source: declared`) | [zenoh-topology-surface.md §2.5a](../design/zenoh-topology-surface.md) |
| The identity record carries each person's handle and nationality | Person devices with no slot link resolve only through the roster, and no identity carries a nationality, so the page shows "not stamped" | The mint writes `metadata_public.handle` and `.nationality`; `goat admin identity resync` backfills them | [zenoh-topology-surface.md §2.5a](../design/zenoh-topology-surface.md) |
| Act on each owner exception | Each exception is a peer on the mesh with no recorded owner, by the operator's decision, until its owner is recorded or its access is decided | Operator action per peer; the reconcile reports stale exceptions for removal | [zenoh-topology-surface.md §2.5a](../design/zenoh-topology-surface.md) |
