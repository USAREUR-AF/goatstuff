# Backlog

Work that is decided or needed and not built. One entry per item: what, why,
what it needs, and the design document that owns it.

| Item | Why | Needs | Design |
|---|---|---|---|
| Messages and bytes per router, and between two routers, on the topology page | The page shows declarations (subscriptions, publishers, topics) but no traffic; an operator cannot tell a busy router from an idle one with the same declarations | A zenoh router built with the `stats` feature, so `@/<zid>/router/metrics` carries per-transport message and byte counters; then a publisher field and a page column. The fleet build's metrics hold only `zenoh_build_info` | [zenoh-topology-surface.md §2.6](../design/zenoh-topology-surface.md) |
| A roster on the management host that is current with enrollments | The topology page finds a router's organisation, the organisation's nation and the individual in the roster; the roster on a host is only as current as the committed `docs/operators/<goatnet>/roster.json`, which is generated from operator manifests, and people enrolled through the identity console have none, so their routers show a peer name and "org not recorded" | A writer that reads the identity registry and writes the roster's public-safe shape to the management host on a cycle; no change to the surface | [zenoh-topology-surface.md §2.5](../design/zenoh-topology-surface.md) |
