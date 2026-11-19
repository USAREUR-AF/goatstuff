# Backlog

Work that is decided or needed and not built. One entry per item: what, why,
what it needs, and the design document that owns it.

| Item | Why | Needs | Design |
|---|---|---|---|
| Messages and bytes per router, and between two routers, on the topology page | The page shows declarations (subscriptions, publishers, topics) but no traffic; an operator cannot tell a busy router from an idle one with the same declarations | A zenoh router built with the `stats` feature, so `@/<zid>/router/metrics` carries per-transport message and byte counters; then a publisher field and a page column. The fleet build's metrics hold only `zenoh_build_info` | [zenoh-topology-surface.md §2.6](../design/zenoh-topology-surface.md) |
| Deploy `transform-shim`: an Ansible role, an image workflow and a goatnet that runs it | The shim is built and tested and runs nowhere | A goatnet with a format to convert; a registered system and read grants for the shim's certificate; a decision record in goatcore for adopting the goat-transformers library | [format-transformers.md §3.7](../design/format-transformers.md) |
| goat-map's library path with no completion step in Python | With `GOATMAP_COT_ENGINE=library`, three steps in `cot_library.py` complete `affiliation`, `dimension`, an error estimate above `9999999`, and null members | In goat-transformers: a derive function that takes a token of a string, a value-table entry for an unlisted value, a `field` option for absent at or above a value (or goat-map taking the library's exact-`9999999` rule), a `json` directive that writes null for an absent member | [format-transformers.md §4.1](../design/format-transformers.md) |
| The portal inspector decodes pack-described formats with the Go engine | A topic in a format the library describes needs an uploaded descriptor set or XSD today | A change to `ops/goat-zenoh-portal/internal/decoder/` in goatcore, a pack bundle in the portal image, a `pack:<format>` topic format | [format-transformers.md §7](../design/format-transformers.md) |
