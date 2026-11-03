# Pub/sub hygiene diagnostic

**Status:** design-of-record, v1 landing.
**Companion:** [`pubsub-best-practices.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/pubsub-best-practices.md) (the vendor-facing explainer this diagnostic mechanizes), `panoscope.md` (the surface it renders on), [`data-access-cop.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/data-access-cop.md) (the access/retrievability COP it complements).

## Problem

panoscope shows *what* a publisher is emitting (its topic fan-out) but never judged *whether the way it publishes is sound*. A publisher emitting `<anchor>/aircraft/<entity-id>/v1` (one topic per aircraft, version-after-entity) sprayed ~270 nodes across the COP, and nothing on the surface told the operator — or the publisher — that the key shape was the problem, whether the data was actually retrievable later, or what the fix was. The [pub/sub best-practices doc](https://github.com/usareur-af/goatcore/blob/main/docs/design/pubsub-best-practices.md) already encodes the right answers (the four topic patterns, the three footguns, "storage isn't a message bus"); it lived only as prose a vendor had to find and read.

This diagnostic turns that prose into a per-publisher verdict the operator sees in-surface, answering two questions:

1. **Key-shape hygiene** — is this publisher's topic space one of the sanctioned patterns, or an anti-pattern (entity/version inversion, unbounded per-*message* key minting, high fan-out with no declared schema)? Derivable from the topology alone — cheap, passive, computed every snapshot.
2. **Retrievability** — if a consumer wants this data *later*, can they actually `get` it? That depends on a **storage** being subscribed to the prefix, which is not visible from the topology. Confirmed with a bounded one-shot `get` probe (reusing panoscope's stale-recall plumbing), lazily, when the operator inspects a publisher.

## The retrieval model the diagnostic teaches

Restating the load-bearing facts from [pubsub-best-practices.md](https://github.com/usareur-af/goatcore/blob/main/docs/design/pubsub-best-practices.md) §2–4, because the verdict encodes them:

- A topic is a **channel**, not an S3 key. Retrieval (`get`) is a separate operation that queries a **storage**, which may or may not cover the key.
- You do **not** need one topic per message. The idiomatic shape for "retrieve a specific entity later" is **Pattern B**: `<vendor>/<system>/<version>/<entity-id>` — entity id **last**, version in the stable prefix. Retrieve one with `get .../<entity-id>`, all with `get .../*`. A rocksdb storage keeps the latest value **per key**, so storage scales with entity count.
- For full history of every message, **Pattern C** (unique key per event) on a persisted prefix; `get .../**` replays within the storage TTL. The selective-retrieval lever is the **key-expression selector** (`*` one segment, `**` any depth) plus, for time-series storages, a `_time=[start..stop]` range.
- **One topic works** (Pattern A) only for latest-value (`get` returns one message regardless of how many were published to it). To pull *specific* messages later you must put the discriminator in a **key segment** (so you can select it) or on a **time-series storage** (select by time). Entity-in-payload-only is the anti-pattern — you lose key selection.
- Retrievability is **not automatic**: `get` returns nothing unless a storage is subscribed to the prefix. That is exactly what the probe confirms.

## Architecture

Three pieces, smallest-blast-radius first.

### 1. Pure classifier — `panoscope_core::pubsub_hygiene`

`classify_publisher(stats: &[TopicStat]) -> PublisherHygiene`, pure (no Zenoh), unit-tested, reused by the panoscope backend and `goat`.

- **Input** `TopicStat { key, total_events, has_schema }` — one per topic the publisher emits (the backend builds these from its raw edges; `goat` from the portal/registry).
- **Families.** Topics are bucketed by segment count and, within a bucket, the highest-cardinality segment position is the entity axis (same varying-segment analysis as the fold). Each over-threshold family is analyzed for shape.
- **Findings** (each carries severity + plain message + fix):
  - `version_after_entity` (advisory) — the varying segment is non-terminal and a constant version-like leaf trails it (`…/<entity>/v1`). Retrievable, but non-idiomatic and it blinds last-segment tooling. Fix: `…/v1/<entity>`.
  - `per_message_keys` (advisory) — a high-fan-out family where ~every key carries ≤1 event and the varying segment is high-entropy (uuid-like): looks like per-event archive (Pattern C). Fine for replay; if the intent is latest-state-per-entity, reuse a stable entity id (Pattern B).
  - `no_declared_schema` (warn) — a fan-out family with no declared format/schema on any member: subscribers can't decode it.
  - `flat_single_channel` / `per_entity_state` (info) — recognized-good patterns, stated positively.
- **Verdict** = worst severity present: `poor` (any warn) / `review` (any advisory) / `good`.
- **probe_selectors** — the fold templates (e.g. `<anchor>/aircraft/*/v1`) the backend should `get`-probe for storage coverage.

### 2. Backend — verdict in the snapshot + a lazy probe endpoint

- The cheap key-shape `PublisherHygiene` is attached per-publisher in the snapshot (a `NodeView.hygiene` summary: verdict + finding codes), so the side panel renders it with no extra round-trip.
- `GET /api/publisher/hygiene?source=<peer>` runs the full classify **and** a bounded one-shot `get` per probe selector (reusing the stale-recall `get`): any reply ⇒ `retrievable: confirmed`; none ⇒ `retrievable: unconfirmed` (**never** "no storage" — we can prove presence, not absence; honesty discipline, cf. the env-health `idle`/`unmetered` states). Returns the full finding list + fix text + the retrievability result.

### 3. Frontend — publisher side-panel badge

The publisher side panel gets a hygiene badge (`GOOD` / `REVIEW` / `POOR`) and, on expand, the finding list with one-line fixes and the retrievability line. Opening the panel fires the lazy probe. Display-only; no wire/ACL/identity impact (ADR 1039 discipline).

## Honesty discipline

- Key-shape findings are **advisory by default** — most "anti-patterns" are still retrievable; the diagnostic teaches, it does not gate.
- Retrievability is **confirm-only**: a positive `get` reply proves a storage answers; silence is reported as `unconfirmed`, never as "not retrievable," because a passive/one-shot angle can't distinguish "no storage" from "empty storage" or "ACL-blocked probe."
- No confabulated scores — the verdict is the worst observed finding severity, nothing weighted or invented.

## Reuse

The classifier is pure `panoscope-core`, so `goat` can expose the same verdict (e.g. `goat access` / `goat doctor` over the registry's declared topics) without re-implementing it. v1 wires the panoscope surface; the `goat` path is a fast-follow.

## v1 limitations

- One varying axis per (publisher, segment-count) bucket (inherits the fold's limitation).
- `per_message_keys` vs `per_entity_state` is a heuristic (event-count + entropy), surfaced as advisory, not asserted.
- The storage probe is per-publisher on-demand, not a fabric-wide retrievability sweep.
