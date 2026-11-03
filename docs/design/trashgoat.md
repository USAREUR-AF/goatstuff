# trashgoat — operator-controlled chaos publisher

**Status:** PR #3 of 3 — runner binary + Ansible role + CI workflow (operator-fired live-fire on nova-a pending image build + cert mint)
**Block:** 92
**Triggered by:** a 12.5 MB OpenAPI z_put silent-drop RCA (2026-05-26) — we
have an empirically-derived sizing-threshold table in
[`pubsub-best-practices.md` §5](https://github.com/usareur-af/goatcore/blob/main/docs/design/pubsub-best-practices.md), but no
substrate that lets an operator generate ad-hoc traffic against
known-suspicious shapes without writing a one-shot Python script every
time.

## What trashgoat is

A controlled-chaos publisher: an operator picks a publish "shape"
(topic, payload generator, cadence, lifecycle) from a portal control
panel, fires it onto the fabric under the `trashgoat/<experiment>/...`
namespace, and watches what panoscope + the audit subscriber + storage
+ live subscribers actually do with it. The point is **empirical
observation of fabric behavior under unusual shapes**, not steady-state
demo traffic. Distinct from:

| Surface | Purpose |
|---|---|
| `synth-vendor` (the `notional` vendor) | steady-state 22-peer demo data — what the fabric should look like in normal operation |
| `demogame` Zenoh publish | application-driven publish; tracks real game events |
| `trashgoat` | **operator-fired chaos** — large payloads, malformed shapes, intentional rate spikes, "what happens if…" experiments |

The name is deliberately undignified — anything trashgoat does is
suspect by construction; consumers should never depend on it.

## Namespace + identity

- **Vendor / URI-SAN identity:** `trashgoat` (single cert minted via
  the same `bootstrap-portal-vendors.sh` path; no per-experiment
  identity).
- **Data namespace (wire publish):** `trashgoat/<experiment-slug>/<...>`
  — the experiment slug is part of the spec; runner publishes here.
- **Control namespace (portal → runner):** `ops/trashgoat/control/<spec-id>/v1`
  — distinct from the data namespace so the runner's cert can carry an
  ALLOW write on `trashgoat/**` and an ALLOW read on
  `ops/trashgoat/control/**` without entangling the two.

## Lifecycle

A trashgoat **spec** is the unit of configuration. Specs live in the
portal's SQLite registry (`trashgoat_specs` table) and walk through
named states:

```
                           +-----------+
                           |   draft   |    (created, not running)
                           +-----------+
                                 |
                                 | activate
                                 v
                          +-------------+
                  +-----> |   active    |    (runner is publishing)
                  |       +-------------+
            resume|             |
                  |             | pause
                  |             v
                  |       +-------------+
                  +-------|   paused    |    (spec preserved; no publish)
                          +-------------+
                                 |
                                 | retire
                                 v
                          +-------------+
                          |   retired   |    (kept for audit; no publish; can't resume)
                          +-------------+
```

Plus a **cleanse** action (orthogonal to state — works from any state
except `draft`): issues `z_delete` against every key the spec has ever
published, dropping the data from storage. State after a cleanse stays
where it was (a paused spec stays paused; an active spec stays active
and will start publishing fresh data on its next cycle).

## Spec shape

Each spec carries:

| Field | Type | Notes |
|---|---|---|
| `id` | integer | autoincrement PK |
| `slug` | string | url-safe kebab-case experiment name, unique; segment 1 of the data key (`trashgoat/<slug>/...`) |
| `description` | string | one-line operator-readable purpose |
| `topic_pattern` | string | full key under `trashgoat/<slug>/...`; literal or template (see below) |
| `payload_spec` | JSON | per-generator config — see below |
| `cadence_spec` | JSON | timing config — see below |
| `state` | enum | `draft` / `active` / `paused` / `retired` |
| `created_at`, `created_by` | timestamp + operator | audit |
| `last_activated_at`, `last_paused_at`, `last_cleansed_at`, `retired_at` | nullable timestamps | lifecycle audit trail |
| `notes` | text | operator scratch — what was learned, when, by whom |

### Payload spec (JSON, by generator kind)

```jsonc
// bytes(size_bytes, content="random"|"zeros"|"repeating:<pattern>")
{ "kind": "bytes", "size_bytes": 12500000, "content": "random" }

// chunked(total_size_bytes, chunk_size_bytes, content)
// emits per-chunk publishes + a manifest publish (Pattern E from pubsub-best-practices §5)
{ "kind": "chunked", "total_size_bytes": 12500000, "chunk_size_bytes": 1500000, "content": "random" }

// json_shape(template) — for testing inspector decode failures, schema-evolution, etc.
{ "kind": "json_shape", "template": { "id": "$uuid", "ts": "$now_ms", "payload": "x" } }

// protobuf_shape(message_type, field_overrides) — requires a schema body uploaded already
{ "kind": "protobuf_shape", "message_type": "notional.tracks_demo.v1.Entity", "fields": { "confidence": 0.42 } }
```

### Cadence spec (JSON)

```jsonc
{ "kind": "oneshot" }
{ "kind": "every", "interval_ms": 1000 }
{ "kind": "burst", "count": 10, "spacing_ms": 50, "then_idle": true }
```

## Topic templating

`topic_pattern` is a literal key OR a template with these substitutions
(resolved at publish time by the runner, not the portal):

- `{seq}` — monotonic sequence counter per spec; starts at 0
- `{uuid}` — fresh UUID per publish
- `{ts_ms}` — wall-clock milliseconds at publish

Examples (all valid):

```
trashgoat/large-payload-12mb/v1
trashgoat/cot-per-entity/v1/{seq}
trashgoat/burst/v1/{uuid}
```

## Control wire contract

Portal publishes spec state changes to `ops/trashgoat/control/<spec-id>/v1`
with a JSON payload:

```json
{
  "spec_id": 42,
  "spec": { /* the full spec object as of this moment */ },
  "command": "activate" | "pause" | "retire" | "cleanse" | "update",
  "issued_by": "operator-handle",
  "issued_at": 1750000000
}
```

The portal also keeps a snapshot of the current spec set queryable via
`session.get("ops/trashgoat/control/**")` — runner cold-start reads
the snapshot then subscribes for live updates (the standard pub/sub
late-joiner pattern from `pubsub-best-practices.md` §3).

## Operator UI

`/admin/trashgoat.html` (operator-only, gated by `requireOperator` per
the existing portal admin pattern). Two zones:

- **Spec list** — table of all specs with state badge, last-activated
  timestamp, brief stats (total publishes counted via audit), action
  buttons (activate / pause / cleanse / retire / edit / delete).
- **Spec editor** — form for creating new specs or editing existing
  drafts. Three sub-panels matching the three JSON fields
  (`payload_spec`, `cadence_spec`, `topic_pattern`). v1 ships a thin
  raw-JSON editor for `payload_spec` + `cadence_spec`; v2 can add
  structured form controls when shapes stabilize.

## ACL impact

The portal does NOT write to the control topic on its own cert; instead
it uses the Zenoh REST plugin (loopback-only on nova-a, same path used
for inspector GETs today) which authenticates as the router's REST
identity. The router's existing ACL doesn't gate REST-plugin writes by
client cert; trusting the docker-network boundary as we do for
demogame' wire publish (CLAUDE.md demogame
calls this out).

The runner (PR #2) gets:
- **Write** on `trashgoat/**` (its own data namespace; one new
  per-vendor ACL stanza emitted at cert mint)
- **Read** on `ops/trashgoat/control/**` (one new fleet-wide ALLOW
  stanza, same shape as the audit-subscriber's read-on-`**`)
- **Storage** on `trashgoat/**` (auto-provisioned at mint via the
  existing F-146 path)

## Out of scope for v1

- **Authoring code execution.** Specs are declarative shapes; the
  runner interprets them. No arbitrary code embedded in specs.
- **Cross-vendor publishes.** trashgoat is a single-vendor publisher;
  it doesn't impersonate other vendors.
- **Rate limiting from the portal side.** The runner is free to ignore
  a `cadence_spec.every.interval_ms` of `1` and just refuse to publish
  faster than e.g. 100 Hz; portal does not enforce.
- **Persistence of generated payloads.** trashgoat publishes are
  ephemeral by design — once delivered, the runner doesn't keep them.
  Re-running the same spec generates fresh content per cadence.

## Phased rollout

| PR | Scope |
|---|---|
| **#1 (merged)** | Phase A planning: design doc + tracker + CLAUDE.md pointer. No code. |
| **#2 (merged)** | Portal substrate: `trashgoat_specs` SQLite table; `/api/admin/trashgoat/specs` CRUD + lifecycle endpoints; `/admin/trashgoat.html` UI; portal publishes control-topic messages on state change (no consumer yet); no data traffic generated. |
| **#3 (this PR)** | `goat-trashgoat` Rust binary at [`ops/goat-trashgoat/`](../../ops/goat-trashgoat/): subscribes to `ops/trashgoat/control/**`, runs specs per their cadence, issues `z_delete` on cleanse. Cold-start replay via `session.get`. `protobuf_shape` is a JSON-envelope stub in v1 (full impl when descriptor-set lookup substrate exists). Ansible role at [`ops/ansible/roles/goat-trashgoat/`](../../ops/ansible/roles/goat-trashgoat/) + apply playbook + deploy-history ledger per ADR 0859 + CI workflow at [`.github/workflows/goat-trashgoat-image.yml`](../../.github/workflows/goat-trashgoat-image.yml). Default `goat_trashgoat_enabled: false` keeps the role harmless until operator-fired live-fire. |
| #4 (future) | Optional polish — structured form editors for `payload_spec` + `cadence_spec`; richer per-spec stats (live publish-rate, byte-rate, last-error); cross-link from panoscope side-panel ("show this peer's spec"); real protobuf encoding via shared descriptor cache. |

After PR #2 lands, the operator can fire a 12.5 MB shot via the UI
and watch what panoscope / audit / `session.get` actually do — closes
the loop on the empirical-observability gap that triggered this work.

## Related

- [`pubsub-best-practices.md` §5](https://github.com/usareur-af/goatcore/blob/main/docs/design/pubsub-best-practices.md) — the
  empirical sizing-threshold table this lab will help fill in further
- [`zenoh-trial-fabric-synth-vendors.md`](zenoh-trial-fabric-synth-vendors.md) —
  the steady-state notional fleet; trashgoat is its chaotic sibling
- ADR 0844 — production Zenoh deployment topology + reserved prefixes
  (`ops/**` is the operator/control prefix, so `ops/trashgoat/control/**`
  is in the right neighborhood); ADR 1035 — why the control plane is
  `ops/` and not the deprecated `system/` it originally shipped under
- ADR 0859 — easy-rollback discipline; PR #2's runner adopts at tier-0
