# Backlog

Work that is decided or needed and not built. One entry per item: what, why,
what it needs, and the design document that owns it.

| Item | Why | Needs | Design |
|---|---|---|---|
| Messages and bytes per router, and between two routers, on the topology page | The page shows declarations (subscriptions, publishers, topics) but no traffic; an operator cannot tell a busy router from an idle one with the same declarations | A zenoh router built with the `stats` feature, so `@/<zid>/router/metrics` carries per-transport message and byte counters; then a publisher field and a page column. The fleet build's metrics hold only `zenoh_build_info` | [zenoh-topology-surface.md §2.6](../design/zenoh-topology-surface.md) |
