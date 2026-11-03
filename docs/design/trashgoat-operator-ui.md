# trashgoat — operator UI design framing

**Status:** design-framing only — invites a designer to author the visual
design bundle for `/admin/trashgoat.html` under [ADR 1018 Augmented
Design-Driven Development](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1018-augmented-design-driven-development.md).
**Block:** 92 (companion to [`trashgoat.md`](trashgoat.md)).
**Scope of this doc:** *what* the operator UI needs to be, *what* gaps the v1
shipped in PR #506 has, *what* the design conversation should produce. It does
NOT specify visual decisions, layout choices, or interaction beats — those are
deliberately left to the design session. Backend / wire-contract / state
machine are OUT of scope; see [`trashgoat.md`](trashgoat.md).

## Why this doc exists

The trashgoat substrate lifecycle is on a three-PR rollout:

| PR | Scope | Status |
|---|---|---|
| PR #1 | Design doc + tracker + CLAUDE.md pointer | merged |
| PR #2 | Portal substrate — SQLite table + CRUD/lifecycle API + v1 operator UI + control-topic emit | merged |
| PR #3 | `goat-trashgoat` Rust runner + Ansible role | pending |

PR #2 shipped a minimum-viable operator UI at
[`/admin/trashgoat.html`](https://github.com/usareur-af/goatcore/blob/main/ops/goat-zenoh-portal/web/static/admin/trashgoat.html)
to unblock substrate verification — table of specs + raw-JSON editor pane +
lifecycle action buttons. It is functional, not designed. The interaction
shape is correct (spec list ↔ editor pane, lifecycle actions per row, JSON
textareas with template loaders); the visual + interaction polish is not.

This doc invites a designer to take the v1 UI as the input brief and produce
a visual design bundle for the production shape under ADDD's per-product tier.

## What trashgoat is, in one paragraph

Operator-controlled chaos publisher. An operator declares a publish *spec*
(slug + topic_pattern + payload_spec + cadence_spec) through the portal UI;
the spec walks a lifecycle (`draft` → `active` ⇄ `paused` → `retired`, plus
an orthogonal `cleanse` action that `z_delete`s every key the spec has
published); the portal emits a control-topic event on every state change; a
runner (PR #3, pending) subscribes to the control topic and actually
publishes. Distinct in purpose from `synth-vendor` (steady-state demo data)
and `demogame` (application traffic): trashgoat is deliberately
*suspect*, used for stress-testing fabric behavior against known-bad shapes
(12 MB payloads, malformed wire formats, intentional rate spikes). The
operator UI is the entire surface — there is no vendor-facing view of
trashgoat. Full design at [`trashgoat.md`](trashgoat.md); the spec shape, the
five payload-generator kinds, the three cadence kinds, the lifecycle state
graph, and the control-wire contract are all canon there.

## The operator's job-to-be-done

The operator opens `/admin/trashgoat.html` with one of these intents:

1. **"I want to fire a one-shot stress test."** Pick a payload shape (often
   `bytes` or `chunked` at some interesting size threshold), pick `oneshot`
   cadence, hit activate, watch panoscope + audit + storage to see what
   actually happens. Deactivate or retire afterwards.
2. **"I want to keep an experiment running in the background while I work on
   something else."** Set up an `every` cadence at a low rate, leave it
   active for hours or days, occasionally check stats. Pause or cleanse when
   done.
3. **"I want to fire a burst and see how the fabric absorbs it."** `burst`
   cadence with high count + tight spacing, fire, watch the rate spike work
   through the system.
4. **"I'm cleaning up after an experiment."** Find an experiment that's been
   running, either pause it (preserve data + spec for later resumption),
   retire it (preserve audit, no resume), or cleanse it (drop every key it
   ever published from storage; state stays where it was).
5. **"I'm reviewing what experiments have run on this fabric."** Scan the
   spec list to see what's been tried, what's still running, what's retired.
   Click into a spec to see its history (last_activated_at, last_paused_at,
   last_cleansed_at, retired_at, notes operator left).
6. **"I want to know what *this* spec is currently doing."** Drill into a
   single spec, see its current state, recent publish stats (cadence-as-fired
   vs cadence-as-declared, byte rate, last error if any), cross-link out to
   panoscope so the running publisher shows up in the fabric topology view.

## What the v1 UI shipped + where it falls short

The v1 UI is [`ops/goat-zenoh-portal/web/static/admin/trashgoat.html`](https://github.com/usareur-af/goatcore/blob/main/ops/goat-zenoh-portal/web/static/admin/trashgoat.html).
The substrate work is correct; the design is placeholder. Specific gaps the
design pass should address:

**Spec list (left zone).**

- Currently a 6-column HTML table (ID / Slug / State / Topic pattern / Last
  activated / Actions). No grouping, no sort, no filter. With 10+ specs the
  view becomes a wall of rows with no information hierarchy.
- The state pill is a colored chip (DRAFT / ACTIVE / PAUSED / RETIRED) but
  carries no live signal — an `ACTIVE` spec that hasn't published in an hour
  looks identical to one publishing at 100 Hz right now.
- No per-spec stats column. Total publishes, byte-rate, last-error all live
  in the audit log; the operator currently has to leave the page to find any
  of them.
- The action buttons (Activate / Pause / Cleanse / Edit / Retire / Delete)
  appear inline on every row regardless of which are valid for that state.
  Currently we filter client-side based on state, but the resulting button
  density per row is high and the destructive actions (Retire, Delete) sit
  visually identical to the constructive ones (Activate).

**Spec editor (right zone).**

- Three raw-JSON `<textarea>` panels for `payload_spec`, `cadence_spec`, and
  `notes`. There are template-loader `<select>`s above each JSON pane that
  drop a default shape in, but once loaded the operator edits raw JSON.
- The five `payload_spec` kinds (`bytes`, `chunked`, `json_shape`,
  `protobuf_shape`) and three `cadence_spec` kinds (`oneshot`, `every`,
  `burst`) have very different field shapes; a structured editor that
  surfaces the right fields per kind would prevent JSON typos.
- The `topic_pattern` field accepts templating substitutions (`{seq}`,
  `{uuid}`, `{ts_ms}`) but the editor doesn't surface this; the operator
  has to know it from reading the design doc.
- The `notes` field is operator scratch — "what was learned, when, by whom"
  per the spec shape. It should be more affordance-y than a plain textarea
  (timestamp-stamped entries? per-activation notes? unsure — design call).
- No preview of what the spec will look like when fired (a dry-run pane
  showing one rendered payload at a representative size, a rendered topic
  key, a cadence timing diagram, etc.).

**Cross-surface integration.**

- No "show this spec in panoscope" affordance. panoscope already renders the
  fabric topology with per-peer drill-down; an active trashgoat spec should
  be one click from its panoscope edge.
- No "show this spec's publishes in the audit subscriber" affordance. Audit
  events are queryable via MinIO; portal could surface a recent-events panel
  per spec.
- No banner relating the v1 page state to PR #3's runner status. Currently
  the page renders a static "PR #2 substrate only" notice; once the runner
  ships, the notice should auto-update based on detected runner presence
  (presence inferred from the control-topic having a subscriber, or from a
  runner-emitted heartbeat topic — backend call, not design).

**Cross-cutting.**

- The page uses bespoke CSS rules under `<style>` inside the HTML, not the
  design-system tokens from [ADR 1018](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1018-augmented-design-driven-development.md)
  / [Block 91](https://github.com/usareur-af/goatdocs/blob/main/docs/design/design-system.md). The design bundle should consume
  the canon (`colors_and_type.css` + `components.css`) verbatim and produce
  a prototype that already adopts the tokens — same pattern as the
  stethoscope wireframe at `docs/design-assets/stethoscope/`.
- No empty state when zero specs exist (currently shows a static "No specs
  yet" line, no on-ramp).
- No error state when the API is down or returns a 5xx (currently dumps the
  raw error string into the loading span).
- No keyboard affordances (no shortcuts to common actions, no focus
  management when opening the editor for an edit).

## What the design pass should produce

A claude.ai/design bundle landing at `docs/design-assets/trashgoat/`,
following the same shape as `docs/design-assets/stethoscope/`:

- `README.md` — verbatim from claude.ai/design (handoff README; not edited
  after handoff).
- `chats/` — the conversation transcript(s) from the design session.
- `project/` — the HTML/CSS/JS prototype output.
- `INSTRUCTIONS-FOR-IMPLEMENTING-AGENT.md` — authored by the steward to tie
  the bundle back to this doc + the trashgoat design-of-record + the in-flight
  Block 92 gate. Per [ADR 1018 §3](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1018-augmented-design-driven-development.md),
  this file is optional; recommended here because the bundle will sit next to
  a design doc and a substrate that already exists, and the agent
  implementing the production shape will need to know which pieces are
  normative.

Per ADR 1018, the per-product tier does NOT ship a `SKILL.md`. The canon-tier
`docs/design-system/SKILL.md` is the single agentic routing point.

The bundle should cover the states the v1 UI currently glosses over:

1. **Empty.** Zero specs exist. The page should have an obvious on-ramp
   ("create your first spec") that lands the operator in the editor.
2. **Few-spec.** 1-5 specs, mixed states. The default state for early
   exploration.
3. **Many-spec.** 20+ specs, mostly retired with a few active. Tests
   information hierarchy, sort/filter affordances, retired-row dim-down.
4. **One spec running hot.** An `every` spec at 100 Hz. Tests live-signal
   rendering — the operator should be able to tell at a glance that this
   spec is actually firing.
5. **One spec failing.** Runner is online but the spec's most recent attempt
   errored (size budget exceeded, ACL deny, etc.). Tests how errors surface
   in the spec list and in the per-spec drilldown.
6. **Runner offline.** The control topic has no subscriber. Specs can still
   be edited + activated but nothing will actually publish; the page should
   make this obvious without making every row look broken.
7. **Editing a draft spec.** Editor pane open with the draft loaded, all
   fields editable, save active.
8. **Editing a non-draft spec.** Editor open with a non-draft loaded —
   currently this state exists but "save will fail" is in the title bar text;
   the design should either gate the editor behind a "make a copy" affordance
   or render the editor read-only with an explicit "this is read-only" badge.
9. **Confirmation dialogs.** Cleanse + Retire + Delete each pop a
   `confirm()` dialog today; the design pass should replace these with
   inline confirmations that surface the actual consequence (cleanse =
   "z_delete N keys", retire = "spec becomes read-only", delete = "audit row
   gone").
10. **Cross-link out to panoscope.** A click target on an active spec that
    deep-links into panoscope's view of the trashgoat peer's edges.

## What the design pass should NOT do

- **Don't redesign the backend.** The spec shape, the lifecycle state graph,
  the control-wire contract, and the API surface are canon per
  [`trashgoat.md`](trashgoat.md). If the design conversation surfaces a
  backend question, file it as a question in the resulting PR rather than
  baking the answer into the prototype.
- **Don't redesign the design system.** Consume `colors_and_type.css` +
  `components.css` verbatim from [`docs/design-system/`](../design-system/).
  If a missing component is needed, the prototype should call it out as a
  gap (so the canon tier picks it up additively) rather than re-styling from
  scratch.
- **Don't add features.** The v1 UI's functional shape is correct; the
  design pass should make that shape *legible* + *liveable*, not extend it.
  New features (per-spec rate-limiting in the UI, multi-select on rows,
  scheduling, exports, etc.) belong in a follow-on after the runner is live
  and the operator has used the surface enough to know what's missing.
- **Don't gate code velocity.** Per ADR 1018's never-blocking provision, the
  runner build (PR #3) is independent of this design pass and continues on
  the track `trashgoat-pr3` baton regardless. The runner will land against
  the v1 UI; the design pass updates the UI to the production shape when
  the bundle is ready.

## Where the bundle lands

```
docs/design-assets/trashgoat/
├── README.md                                  # verbatim from claude.ai/design
├── chats/                                     # conversation transcript(s)
│   └── chat1.md
├── project/                                   # HTML/CSS/JS prototype
│   ├── Trashgoat.html
│   ├── colors_and_type.css                    # imported verbatim from docs/design-system/
│   ├── components.css                         # imported verbatim from docs/design-system/
│   └── ...
└── INSTRUCTIONS-FOR-IMPLEMENTING-AGENT.md     # authored by the track steward
```

Phase A (this PR) lands an empty
`docs/design-assets/trashgoat/` directory
with a placeholder `INSTRUCTIONS-FOR-IMPLEMENTING-AGENT.md` so the
directory exists in the doc tree before the bundle arrives. The design pass
fills it in.

## Related

- [`trashgoat.md`](trashgoat.md) — the design-of-record. Backend, spec
  shape, lifecycle, control-wire contract, ACL impact, phased rollout. Read
  first for context.
- [ADR 1018](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1018-augmented-design-driven-development.md) — ADDD
  discipline. The per-product tier is what `docs/design-assets/trashgoat/`
  instantiates.
- [`design-system.md`](https://github.com/usareur-af/goatdocs/blob/main/docs/design/design-system.md) — Block 91 design system. The
  tokens + components the bundle should consume verbatim.
- `docs/design-assets/stethoscope/` — the
  first per-product-tier instance; reference for bundle shape.
- `docs/design-assets/stethoscope/INSTRUCTIONS-FOR-IMPLEMENTING-AGENT.md`
  — reference for the steward-authored clarity layer.
- `panoscope.md` — the live fabric-topology view this UI
  should cross-link out to.
- [`pubsub-best-practices.md` §5](https://github.com/usareur-af/goatcore/blob/main/docs/design/pubsub-best-practices.md) — the
  large-payload sizing-threshold table trashgoat is meant to help refine
  empirically; the operator UI is the access point for that experimentation.
