# Tipper — preplanned-specialist proof case for the agent-class peer pattern

| Field | Value |
|---|---|
| Status | Draft (Phase 2D-late / Phase 3 commitment) |
| Role | Proof case 1 of 2 for the rapid-adaptation-with-AI bones |
| ADR anchor | [`../adr/0512-rapid-adaptation-with-ai.md`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0512-rapid-adaptation-with-ai.md) |
| Companion proof case | Scout — see [`scout.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/design/scout.md) |
| Bones doc | [`ai-rapid-adaptation.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/design/ai-rapid-adaptation.md) |
| Implementation tracker | `../project/implementation-plan.md` Block 23 |

## What Tipper proves

Tipper is **proof case 1 of 2** for the rapid-adaptation-with-AI substrate. It exists to demonstrate that the agent-class peer pattern, queryable-by-default fabric, citation discipline, and mandatory data-dictionary declaration all hold under the **preplanned long-running specialist** shape. Scout (proof case 2 — see [`scout.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/design/scout.md)) demonstrates the same bones under the **human-defined ad-hoc** shape. Both share the substrate convention.

Tipper is deliberately small: statistics-only, no LLM, no tool-use planning, no conversational interface. The point is the substrate, not the agent science. The sophistication of the agent is *not* what's being proven; what's being proven is that future preplanned specialists (flightplan-deviation tipper, fusion correlator) reuse the convention without re-deciding it.

## Concrete design

### What Tipper does

Subscribes to `data/track/**`. Maintains per-source rolling statistics for three dimensions of normal radar-track behavior:

- `speed` (m/s)
- `altitude` (meters)
- `trajectory_delta` (heading change between consecutive samples, degrees)

When a track sample exceeds *K* standard deviations on any tracked dimension, Tipper emits an **anomaly** to `agents/tipper/anomaly/<track-id>` and adds the track to its **inspection queue** at `agents/tipper/queue/<track-id>`. Downstream consumers (a human operator dashboard, Scout fielding a related ad-hoc question, a future fusion agent) subscribe to `agents/tipper/queue/**` to pick up tracks for further inspection.

That's the entire scope. Statistics, not LLMs.

### Topic schema

**Input — `data/track/<source-id>/<track-id>`** conforms to the event-telemetry schema (`event-telemetry-schema.md`, ADR [`0507`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0507-event-telemetry-schema.md)). Tipper-relevant fields:

```yaml
track_id: string
source_id: string
timestamp: rfc3339
position: {lat: float, lon: float, alt_m: float}
velocity: {speed_mps: float, heading_deg: float}
classification: string             # advisory only
```

**Output — `agents/tipper/anomaly/<track-id>`** carries the standard agent-emission envelope (citation discipline per ADR [`0512`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0512-rapid-adaptation-with-ai.md)):

```yaml
track_id: string
source_id: string
detected_at: rfc3339
window: {source_id: string, sample_count: int}
dimensions:
  - name: speed | altitude | trajectory_delta
    value: float
    mean: float
    stddev: float
    z_score: float
    unit: string                   # m/s, meters, degrees — required per data-dictionary
threshold_k: float
agent_identity: string             # SPIFFE / Ory subject of emitting Tipper instance
agent_version: string
sources:                           # citation array — required, non-empty
  - topic: string                  # e.g. data/track/<source-id>/<track-id>
    timestamp: rfc3339
    digest: string                 # payload hash
```

**Output — `agents/tipper/queue/<track-id>`** (inspection-queue entry):

```yaml
track_id: string
queued_at: rfc3339
anomaly_ref: string                # zenoh keyexpr of the corresponding anomaly
status: pending | inspected | dismissed
ttl_until: rfc3339                 # default queue retention 24h
sources: [...]                     # same citation array
```

**Inbound `cmd/agents/tipper/clear/<track-id>`** allows downstream consumers (a human operator who has inspected a track, or Scout closing out a follow-up request) to clear an entry. Tipper handles this single inbound command; it issues no outbound `cmd/`.

### Queryable

Per the queryable-by-default bone, Tipper exposes `cmd/agents/tipper/query` so Scout (and dashboards, and CLI users) can introspect Tipper's state. Query schema published at `attest/queryable/agents/tipper`:

```yaml
input:
  filter:
    track_id: string?
    source_id: string?
    z_score_min: float?
    since: rfc3339?
    status: pending | inspected | dismissed | all?
output:
  matches:
    - <queue entry, with citations>
  total: int
  query_time_ms: float
```

### Statistical model

Per-source online mean and standard deviation via Welford's algorithm. Window size *N* (default 1000 samples) per source. Anomaly threshold *K* (default 3.0 standard deviations). Bootstrap: a source's first *N_min* samples (default 50) accumulate without emitting anomalies — establishes baseline before flagging.

Configuration is declarative, per-source override possible:

```yaml
tipper:
  defaults: {window_size: 1000, threshold_k: 3.0, bootstrap_min: 50}
  per_source:
    radar-01: {threshold_k: 2.5}      # known-quiet airspace
    radar-02: {window_size: 5000}     # high-traffic
```

Online learning, drift-aware baselines, seasonality, classifier-conditioned baselines — all out of scope for v1.

### Identity, audit, observability, schema

- **Identity:** Phase 2 — Ory service-account `agents/tipper/<instance>`. Phase 3+ — SPIFFE SVID `spiffe://goat/agents/tipper/<instance>` per ADR `0412`.
- **Audit:** every emission lands in the audit-logger interceptor (ADR [`0508`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0508-audit-logger-interceptor.md)) keyed by Tipper's identity. Citation array carried into the integrity tier ([`0811`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0811-audit-log-integrity.md)).
- **Observability:** `/metrics` endpoint — track-ingestion rate, anomaly emission rate, per-source baseline stability, queue depth. Health at `health/agents/tipper/<instance>` per ADR [`0815`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0815-observability-stack.md).
- **Schema declaration (mandatory).** Tipper publishes schemas for both anomaly and queue topics at `attest/schema/agents/tipper/anomaly/**` and `attest/schema/agents/tipper/queue/**`. Without these declarations the schema-enforcer interceptor rejects publication. See [`data-dictionary.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/data-dictionary.md).
- **Lineage.** Tipper's emissions feed the lineage materializer automatically via the audit-logger output topic. No agent-side lineage code.
- **Policy.** Tipper v1 issues no outbound `cmd/`, so no policy-engine ([`0805`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0805-policy-engine-active-passive.md)) interaction. Inbound `cmd/agents/tipper/clear/**` is gated by the *issuer's* identity, not Tipper's.

### Implementation surface

| Path | Purpose |
|---|---|
| `ops/agents/tipper/` | Rust crate — agent peer binary |
| `ops/agents/tipper/Cargo.toml` | Pinned dependencies, built under the same supply-chain pipeline as the audit-logger interceptor (per ADR [`0708`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0708-build-zenoh-from-source.md)) |
| `ops/agents/tipper/Dockerfile` | Distroless / Chainguard-shaped final stage |
| `ops/agents/tipper/schema/` | `attest/schema/...` source-of-truth for Tipper's published schemas |
| `ops/ansible/roles/tipper/` | Deploy role |
| `docs/operations/tipper-runbook.md` | Operator runbook (created when Tipper deploys) |

Built under the existing multi-arch pipeline alongside the Zenoh image and audit-logger plugin.

## Why Rust

Aligns with the project's tech-strategy preference — Rust for data-services, performance-critical daemons, anything pairing naturally with the Rust-native Zenoh ecosystem. Existing audit-logger interceptor is Rust; build pipeline is Rust. Tipper inherits the toolchain.

## Out of scope for Tipper v1

- LLM-backed inference. No model serving, no embeddings, no RAG.
- Tool-use planning loops. No outbound `cmd/`.
- Multi-agent orchestration.
- Trajectory anomaly detection beyond simple delta (Kalman / IMM / particle filters).
- Per-track-class baselines (commercial vs military vs UAS profiles).
- Online drift-aware baselines (seasonality, time-of-day coupling).
- Federated baselining across sites.
- Adversarial robustness against poisoned input topics — defers to publisher-trust-model (ADR [`0511`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0511-publisher-trust-model.md)) and mesh microsegmentation (ADR [`0807`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0807-mesh-microsegmentation.md)).

## How we know Tipper works (test plan)

The substrate is what we are proving; Tipper's tests are a vehicle.

- **Unit tests.** Welford correctness; threshold logic; bootstrap behavior.
- **Smoke test (CI).** Run Tipper against a synthetic track stream; assert anomaly emission for a planted outlier, queue entry created, audit-logger received emission with citation array.
- **Integration soak (integdev).** 24h soak with `ops/zenoh/loadgen/` producing a steady-state track stream + 1% anomaly injection. Pass: anomalies emitted within 5s, queue depth bounded, no missed audit entries, identity correctly recorded, citation array non-empty on every emission.
- **Substrate-proof criterion.** A second preplanned specialist (e.g., flightplan-deviation tipper) ships by reusing the agent-class convention without changing it. Until that ships, the substrate proof for the preplanned shape is *provisional*.

## References

- ADR [`0512`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0512-rapid-adaptation-with-ai.md) — bones + Tipper as proof case 1 + Scout as proof case 2
- ADR [`0208`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0208-phoenix-native-peer-discipline.md) — phoenix-native peer discipline
- ADR [`0508`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0508-audit-logger-interceptor.md) — audit-logger interceptor
- ADR [`0511`](https://github.com/usareur-af/goatcore/blob/main/docs/adr/0511-publisher-trust-model.md) — publisher trust model
- [`ai-rapid-adaptation.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/design/ai-rapid-adaptation.md) — bones / strategic frame
- [`scout.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/design/scout.md) — proof case 2 (ad-hoc, human-defined)
- [`data-dictionary.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/data-dictionary.md) — schema declaration discipline
- [`lineage.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/lineage.md) — lineage materializer (Tipper emissions feed it)
- [`entity-vocabulary.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/design/entity-vocabulary.md) — entity glossary referenced from Tipper schemas
- `zenoh-data-fabric.md` — keyexpr conventions
- `tech-strategy.md` — Rust default for data-services
- `../project/implementation-plan.md` Block 23
