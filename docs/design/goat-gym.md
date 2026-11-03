# goat gym — the training surface

| Field | Value |
|---|---|
| Status | Built + ZAGROS-live. Rebuilt 2026-08-14 after first contact: modules teach in place, project documents are hosted alongside, and nothing reaches a reader as a repository path. |
| Scope | Role-based training lanes over every surface we run, plus the mechanisms behind them. |
| SoT | [`ops/gym/gym.yaml`](../../ops/gym/gym.yaml). Gate [`ops/gym/check.py`](../../ops/gym/check.py); renderer [`ops/gym/generate.py`](../../ops/gym/generate.py). |
| Anchor | [ADR 1132](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1132-goat-gym-training-surface.md) |
| Related | teammate-recommended-reading.md, [onboarding-rails.md](https://github.com/usareur-af/goatcore/blob/main/docs/operations/onboarding-rails.md), [goat-cli-hosted-docs.md](https://github.com/usareur-af/goatcore/blob/main/docs/design/goat-cli-hosted-docs.md), [warrant.md](https://github.com/usareur-af/goatcore/blob/main/docs/design/warrant.md) |

## What it is

A mesh-internal site at `gym.<site>.<zone>` carrying **lanes** — an ordered walk
through the platform for one role — built from **modules**. A module is a
pointer at the doc that owns an explanation, plus the two things a doc does not
give you: a drill, and the quirks.

Today: **9 lanes, 80 modules, all written, 159 evidenced quirks, 29/29 live services
covered**, **64 project documents hosted** beside it, and **19 foundation technologies**
with 49 verified external readings.

| Lane | For |
|---|---|
| Newcomer | Anyone with a welcome package, whatever their role will be |
| Fabric developer | DEVELOPER — publishing or subscribing on the fabric |
| Inspection and triage | MONITOR, and anyone asking "is it actually up?" |
| Platform operator | PLATFORM-ADMIN running deploys and standups |
| Identity and access | IAM-ADMIN and sponsors |
| Network and ACLs | Site network admins, and anyone changing who can reach what |
| Machine and gear custodian | Whoever is responsible for a box, sensor or switch |
| Partner and external | Mission partners and external orgs |
| Repo contributor | Anyone sending a pull request, human or agent |

## What changed after first contact, and why

The first version pointed: a module named the document that owned the
explanation and printed its repository path. That was wrong for the audience.
**Most readers have no repository**, so a path is a dead end wearing the clothes
of a citation — the reader spends a click and gets nothing. The operator's
verdict on the first pages was "sparse", and it was correct.

The rebuild keeps the anti-duplication principle and changes how it is served:

| Then | Now |
|---|---|
| "The explanation lives here" + a repo path | The module **teaches**: summary, numbered steps with real commands, and quirks that say what to do |
| a path only a repo holder can follow | the document itself is **hosted beside the gym**, linked by its own title |
| a quirk stating a fact | a quirk stating a fact **and the instruction that follows from it** |
| surfaces named as text | surfaces **linked**, with who can open them |
| nothing checked what shipped | a **publication gate** refuses names, addresses and places of operation |

**The anti-duplication rule survives intact.** The gym still holds no second
copy of any explanation: a module's own prose is the *use* of a thing — how to
do it here, what to watch for — and the reasoning stays in the project document,
which is now one click away instead of one repository away. F-326 is why that
matters: two pages claiming one job means only one gets maintained.

A module that opens must teach, and [`check.py`](../../ops/gym/check.py)
refuses one that does not:

```yaml
- id: silent-denial
  status: written                 # `planned` modules get NO page — a named gap
  summary: >-
    The single most expensive behaviour on this system: a publish you are not
    permitted to make returns success…
  walkthrough:                    # concrete, in order, with real commands
    - do: Recognise the symptom.
      note: You publish, your code is happy, and the subscriber sees nothing.
    - do: Prove it from a surface rather than from your client.
      run: goat top --once
  quirks:
    - claim: A denied publish reports success to the publisher.
      so: Never treat "my client returned OK" as evidence a message was accepted.
      evidence: F-177                       # never rendered as a path
  deeper:
    - doc: docs/design/zenoh-brokerless-consequences.md   # HOSTED, shown by title
      why: Why every central fix re-creates the broker that was rejected.
  surface: [zenoh-router]         # rendered as a live link on this goatnet
```

**A `planned` module has no page.** It appears in its lane as a named gap. A
thin page is worse than an honest absence, because the reader pays the click
before finding out.

## What a module adds

**Order.** A lane is the sequence for someone who does not yet know why they are
reading. A module may appear in several lanes and does.

**A drill.** One exercise, one stated pass condition, and a `safety:` field:

```yaml
drill:
  do: Run the registry's byte-identical check.
  pass: Exit 0 — the applied artifact matches what the registry generates.
  safety: read-only
  run: goat admin peer ls --env <your-goatnet>   # optional, FOR DISPLAY
```

`safety:` is `read-only` / `writes` / `admin`, and `needs: mesh` marks drills
that require a connected peer. Most drills are read-only on purpose: a training
surface whose exercises mutate production is a training surface people skip.

`do:` is prose addressed to a person, and it stays that way. A sweep on
2026-09-04 found that **all 82 drills have a prose `do:` and none is a command**
— "Open your welcome guide's Repositories page, find the example that matches
what you are about to build, and run its first scenario end to end." That is
correct for a training surface. Most drills teach judgement, and rewriting them
into commands to make them executable would trade a good training surface for a
bad test suite.

`run:` is an optional **display** string — the shape of the command, shown on
the page. It may carry a placeholder (`<your-goatnet>`) precisely because a
person reads it and substitutes.

**`check:` — the executable half (optional).** Where a real command genuinely
stands in for the drill, a module may add one. This is what
[`ops/gym/run.py`](../../ops/gym/run.py) executes:

```yaml
  check:
    run: "python3 ops/warrant/generate.py --check"
    expect: exit-zero          # exit-zero | exit-nonzero | contains
    why: >-
      This IS the drill — the registry's byte-identical check is the command
      the module asks you to run.
```

Three rules, each enforced by [`check.py`](../../ops/gym/check.py):

- **`check.run` may not contain a placeholder.** It is shelled out verbatim, so
  `<your-goatnet>` becomes a literal argument and the drill fails for a reason
  unrelated to what it tests. The two fields look alike, and copying the
  display `run:` into a `check:` is the obvious mistake — so it is the one the
  guard names.
- **`why:` is required.** A check whose relationship to its drill is unstated
  drifts away from it silently.
- **No `check:` is not a gap.** A drill without one is reported as `human`, not
  as missing coverage. Pressuring authors toward full coverage produces checks
  that do not really test their drills.

Seven of 82 carry a check today, two of which need a live goatnet. That number
should grow slowly and only where the relationship is honest.

**Quirks — the part that saves the days.** Behaviour that is true, load-bearing
and invisible on first look, each carrying `evidence:`:

```yaml
quirks:
  - claim: >-
      An ACL-denied publish returns success to the publisher and nothing to the
      subscriber. Neither end sees an error.
    evidence: F-177
```

Evidence is a finding ID (the file must exist), an ADR, or a repo path. **A
quirk with no evidence is folklore and CI refuses it.** This is a deliberate,
narrow exception to one-fact-one-home: the finding holds the analysis, the
module holds the claim and the citation.

## Coverage — the join to isitup

`surface:` on a module names isitup service ids. `check.py` harvests every
`isitup_services` row across `ops/ansible/group_vars/*/` and prints which live
services no module covers.

```
gym: 80 of 80 modules written across 9 lanes, 159 evidenced quirks
gym: isitup coverage 29/29 live service ids
gym: 106 rendered pages checked for repository paths
```

Coverage is **reported, not enforced** — failing a build for content nobody has
written yet trains people to bypass the gate. `--strict-coverage` turns it into
a failure for the day we decide to hold the line.

One exemption mechanism exists, `meta.surfaces_without_rows`, and it needs a
reason per entry. It has exactly one member: **isitup itself carries no row for
itself.** It is the thing rendering the rows, so a self-row would probe the
probe — with the consequence that isitup's own outage is the one outage it
cannot report, and the first sign is every row going stale rather than red.

### Where a surface link points

The row already says. Two of its fields decide the href, and the gym reads both
rather than assuming `https://<fqdn>/`:

| Field | Effect on the link |
|---|---|
| `kind: api` | **No link at all.** A machine door with no web page behind it — the name routes nothing through the ingress and answers 404. The gym renders the service's name as plain text with a *not a web page* chip, which is what the status page has always done with these rows. |
| `link_path` | The row's landing page when it is not `/`. Rows that share a hostname are told apart only by this — on ZAGROS four rows sit on the status page's hostname and without it all four linked to the same index. |

Both were being ignored, and the first one shipped: following *Open it on this
goatnet* from the data-fabric module landed a reader on a 404 at the router's
name (observed 2026-08-17; four rows affected on ZAGROS — zenoh, onboard, oauth2,
mgmt — nine links across the site). The gate below now refuses the shape so it
cannot come back quietly.

## The publication gate — what may be served

The gym is **mesh-only and otherwise unfenced**, so every enrolled peer can read
it — on a sandbox goatnet that includes partner participants and candidates who
are not yet anything. The question is therefore not "is this secret" but "would
I hand this page to every peer on the mesh."

`ops/gym/scrub.py` refuses to write a page carrying:

| Class | Why it is refused |
|---|---|
| a person's name or handle | PII, and it identifies who runs what. The roster is read **at run time** from the operator manifests — never copied into this repo, because a committed roster is the spillage the identity guard already refuses |
| an email address | PII, and a login on several planes |
| a named place of operations | the one fact that turns an engineering page into targeting information |
| a mesh or private address | publishing the map hands a visitor the inventory |
| a token, key or instance id | nothing should carry one; a page that does must not ship |

It deliberately has **no opinion on prose**. A gate that invents findings gets
muted, and a muted gate is worse than none.

One consequence worth stating plainly: because the roster is gitignored, **CI
cannot run the name check** — there is nothing to read there. The deploy runs it
with `--require-roster`, so the check that matters runs where the roster exists,
and refuses rather than passing silently. The first version of that file
resolved only the worktree, found zero names and reported every page publishable
— a guard that could not fail.

## The library — documents hosted beside the gym

Every document a module cites is rendered into the site and linked **by its own
title**. 64 are hosted today; 74 more were refused by the gate above and are
therefore named on the module that cites them, never linked or pathed — the
reader learns the document exists and can ask for it.

Intra-document links are rewritten to hosted siblings and **dropped when the
target is not hosted**, because a link that 404s teaches a reader to stop
trusting the page.

When the repository is open-sourced the whole documentation set can be hosted
the same way; nothing about the mechanism changes except how much passes the
gate.

## Foundations — what we use and do not own

Most of what a person needs to learn here was not written here. WireGuard,
Zenoh, Ory, Postgres, Traefik, Terraform, Docker — each has documentation better
than anything we would write, and none of it is ours to maintain. Re-explaining
them would create the second copy that rots; linking them without saying how we
use them leaves the reader to find our deltas by accident.

[`ops/gym/upstream.yaml`](../../ops/gym/upstream.yaml) carries **14
technologies**, each with exactly three things plus curated links:

| Field | What it holds |
|---|---|
| `what_it_is` | one paragraph — enough to know whether you need it today |
| `how_we_use_it` | the specific shape it takes HERE: versions, pins, choices |
| `differences` | **where our use diverges from what the upstream document assumes** |

The third field is the reason the section exists. No upstream document will ever
tell you that our idempotence claim describes the host and not the running
container, that a denied publish returns success, or that bringing a proxy up
from its base file drops its certificate resolver. That is ours to write, and it
is what turns a reading list into training.

**Links are verified, not trusted.** `upstream_check.py --verify` reaches every
URL and records the date it last answered; the page shows that date. Verification
is deliberately **not** a gate — a check that fails when a third party
reorganises their documentation is a check people learn to ignore. The shape
gate (every technology names its differences, every reading carries a reason,
every technology carries an activity) does run in CI. All 35 links answered on
2026-08-15.

Every link is **off-mesh** and labelled as such: the gym is served where the
internet may not reach, which is the situation the training is for.

## Documents we do not host

74 of the cited documents fail the publication gate, so they are not hosted. The
first version named them and said "ask a platform admin", which rendered as a
greyed-out title with an apology — readers took it for breakage. Each now shows
four things:

| Part | Why |
|---|---|
| the document's **title**, in normal ink | it exists; it is not a broken link |
| **what it gives you** | so the reader can judge whether to chase it |
| a **scrubbed preview** — one contiguous opening paragraph | substance rather than a promise. Held to the same rules as any other page: the publication gate AND the no-repository-paths rule. It walks paragraphs and takes the first clean one rather than editing a quotation to fit; if none qualify, the page says the preview was **withheld** instead of showing a gap |
| **where it lives and what it costs to get in** | a link to the repository, with the caveat that it is private and access is requested from a platform admin |

**Linking the repository reverses a standing rule** that repo URLs stay out of
served pages, because they leak org, repo and file layout to viewers with no
access. Operator's call, 2026-08-15: a link with an honest caveat beats a dead
end. It is scoped to this surface — internal training — and the caveat renders
every time rather than being implied. Where the repository is a goat core
repository, the page also says it is being open-sourced and that the public link
will replace this one; that note appears **once per page**, not under every
entry, because repeated it became noise.

Today 70 entries carry a preview and 6 are withheld.

## The gate

[`ops/gym/check.py`](../../ops/gym/check.py) refuses:

| Refusal | The rot it prevents |
|---|---|
| a written module with fewer than two steps | a pointer wearing a module's clothes — the failure the rebuild exists to prevent |
| a step with neither a note nor a command | it says nothing |
| a quirk with no evidence | folklore accumulating in a surface people trust |
| a quirk with no `so:` | a fact without an instruction leaves the reader where they started |
| a drill with no pass condition | an exercise nobody can grade is a suggestion |
| a `deeper:` document that does not exist | it becomes a missing library page, silently |
| a `surface:` id no goatnet declares | a typo drops the module out of the coverage denominator |
| **a repository path on a rendered page** | most readers have no repository; checked against the BUILT SITE, because that is where the reader meets it |
| **a link to a `kind: api` row on a rendered page** | those names route nothing through the ingress and answer 404; same reason, checked against the built site |

`ops/gym/test_check.py` proves each refusal
fires — 19 cases, run in CI ahead of the gate itself, because a guard that
passes but never fails is worthless.

**The workflow is deliberately not path-filtered.** A module breaks when a doc
it points at is renamed, which touches `docs/` and never touches `ops/gym/`. A
`paths:` filter would run the gate on every PR except the ones that can break
it. (.github/workflows/gym-registry.yml)

## Render and serve

`generate.py` renders static HTML — index, one page per lane, one per module —
plus `gym.json` for anything that wants the registry without scraping. No
external requests, no build toolchain, no fonts or scripts fetched at runtime: a
training surface that cannot render without the internet is useless at the edge,
which is the place the training is for.

Source paths render as **text, not links** — a reader on the mesh cannot open
`docs/design/x.md`, and a link that 404s teaches the reader to stop trusting the
page.

Served by the [`gym`](../../ops/ansible/roles/gym/) role: nginx:alpine behind
Traefik's file provider, mesh-only middleware, `le` resolver — the goat-docs
shape. Two differences worth knowing:

1. **The payload is the repo, not a release asset.** The role runs `check.py`
   (fatal) and `generate.py` on the control node from the checked-out tree, then
   copies the site to the host. There is no version tag: the served site is the
   commit you deployed from. This is why the `ansible-preflight` wrapper matters
   more here than anywhere.
2. **The role applies the lessons it teaches.** The webroot is emptied in place
   rather than removed and recreated (a new inode makes the running nginx serve
   an empty mount); the container's `nginx.conf` is checksummed against the
   host's and the container recreated on mismatch; and the health probe
   runs *inside* the container with no `failed_when: false`, so it can actually
   fail.

## Where it runs, and where it deliberately does not

**ZAGROS hosts it. zagros-prod deliberately does not.** That is a decision, recorded
in three places so it cannot be mistaken for a gap:

| Where | What it says |
|---|---|
| `group_vars/zagros_hosts/gym.yml` | `gym_enabled: true` — a goatnet hosts the gym because its own config says so |
| the role's defaults | `gym_enabled: false` — off unless a goatnet opts in, and the play **refuses** rather than skipping quietly |
| `group_vars/.groupvars-parity.yaml` | the zagros-prod exclusion, with its reason, under the repo's absence-as-data rule |

**Why not production.** The gym is unfenced on the mesh and hosts a library of
internal project documents. On the goatnet where people are onboarded that is a
reasonable trade — the training has to be reachable by the people it is written
for, before they have anything. On a production goatnet it is a poor one: the
same surface adds an unfenced service and an internal-document library to an
environment whose population and posture are narrower, in exchange for training
value that belongs where onboarding happens.

The refusal names the decision, so an operator who points the play at the wrong
goatnet learns why rather than watching a play skip:

```
goat gym is not enabled for <host>. This is a DECLARED DECISION, not an
oversight — a goatnet hosts the training surface by setting gym_enabled in
its own group_vars.
```

Nova is simply not a target today; no decision has been recorded either
way.

## Deploying it

The runbook is [gym-deploy.md](../operations/gym-deploy.md). The order that
matters: **the isitup row and the DNS record land together, then the deploy.**
A row without a deployed service is a red tile; a deployed service without a row
is a name nobody can resolve. Neither is committed ahead of the other.

## What this is not

- **Not a replacement for any doc it points at.** If a module explains rather
  than points, it is a bug — the gate exists to catch it.
- **Not certification.** A drill proves you can do the thing today, to yourself.
  Nothing is recorded, and nothing gates on it.
- **Not a place for secrets or one-time values.** Deliberately unfenced beyond
  the mesh; anything that could not be shown to a candidate does not belong.

## Open

- **Not deployed.** No goatnet has an isitup row yet; the surface is dark by
  construction until an operator fires the play.
- **Quirk coverage is uneven.** 159 quirks across 80 modules, but they cluster
  where the findings cluster — the fabric, deploys, identity. A module with
  `quirks: []` renders "none recorded yet", which is a statement about the gym
  rather than about the surface.
- **No `goat gym` CLI subcommand.** `gym.json` is served so one can be added
  without re-modelling anything; nothing consumes it yet.
  [`ops/gym/run.py`](../../ops/gym/run.py) reads `gym.yaml` directly, so a
  subcommand would be a front end, not a re-model.
- **Only 6 of 82 drills are executed.** `run.py` runs the drills carrying a
  `check:`; the other 76 are `human` and mostly should stay that way. The
  correction to the earlier note here is worth keeping: this was recorded as
  "nothing runs the `do:` commands", which read as though the commands existed
  and merely went unrun. They do not exist — `do:` was always prose. The gap
  was never a missing runner; it was that no drill had ever declared an
  executable form.
- **Nothing runs `run.py` on a schedule.** `--mesh` now works and was fired
  live against ZAGROS on 2026-09-05 (7/7, including `goat health` and
  `goat top --once` against the live fabric), but that was a person running it.
  CI runs only the offline checks, by design — a gate that needs a goatnet is a
  gate that fails when the goatnet is down. Until something runs `--mesh` on a
  cadence, a mesh drill that broke is still invisible between manual runs.
