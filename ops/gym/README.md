# ops/gym — goat gym

The training surface. Design: [`docs/design/goat-gym.md`](../../docs/design/goat-gym.md)
([ADR 1132](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1132-goat-gym-training-surface.md)). Deploy runbook:
[`docs/operations/gym-deploy.md`](../../docs/operations/gym-deploy.md).

```
gym.yaml      the registry — SINGLE SOURCE OF TRUTH (lanes, modules, drills, quirks)
content/      prose for the few modules nothing else covers
check.py      the gate + the coverage report; needs the full doc tree and goatnet declarations, so it is not a check in this repository
generate.py   renders the static site + gym.json
site/         render output — gitignored, produced at deploy time
```

```bash
python3 ops/gym/generate.py     # renders into ops/gym/site/
```

## Planned and frontier modules

A module with `status: planned` has no page. Its lane lists it as a named gap. Add
`frontier: true` to a planned module when the tooling it would teach is not built yet:
the lane then marks it "frontier" and says so, so a reader can tell a lesson that is
only unwritten from one whose subject does not exist yet. `check.py` refuses `frontier`
on a written module.

## Lane headings and the next lane

A lane may carry `headings:`, a map from a module id in the lane to a heading shown before
it, and `next:`, the id of the lane to go on to. The learner ladder is kept to its ten rungs
and points at "Beyond the ladder" this way, so a newcomer sees the ladder and not a longer list.

## Adding a module

**Point first.** If any doc in the tree explains the thing, the module is a
pointer and carries no prose of its own:

```yaml
  - id: your-module
    title: What the reader will be able to do
    level: basics | working | mastery
    minutes: 20
    mode: reference
    source:
      - path: docs/design/the-doc-that-owns-it.md
        section: §"the part that matters"      # optional
        gives: One line — what the reader gets there.
    surface: [isitup-service-id]                # optional, drives coverage
    quirks:
      - claim: >-
          The behaviour that is true and invisible on first look.
        evidence: F-123 | docs/adr/1129-….md | ops/warrant/generate.py
    drill:
      do: One command, or one thing to find.
      pass: What you should see. Not "you understand it".
      safety: read-only | writes | admin
      needs: mesh                               # optional
```

Then add the id to at least one lane's `modules:` list, in the order someone
should walk it.

**Write in place only when nothing owns it** — and say so. `no_sot:` is a
falsifiable claim, so name what you searched:

```yaml
    mode: in-place
    no_sot: >-
      Nothing documents X. Searched: docs/design/*x*, docs/operations/*x*, CLAUDE.md.
    content: ops/gym/content/your-module.md
```

If a reviewer knows the doc exists, the module is wrong and becomes a pointer.

## Rules the gate enforces, and why

- **A `source:` path must exist.** A renamed doc turns a curated pointer into a
  dead end.
- **A quirk must carry evidence.** Folklore accumulating in a surface people
  trust is worse than no surface.
- **Every module needs a drill with a pass condition.** An exercise with no pass
  condition is a suggestion.
- **`quirks: []` is required when there are none.** Silence and "none recorded"
  are different statements, and only one of them is honest.
- **A `surface:` id must be declared by some goatnet.** A typo drops the module
  out of the coverage denominator silently.

Coverage is printed, not enforced. `--strict-coverage` fails on any live isitup
service with no module, for the day the repo decides to hold that line.
