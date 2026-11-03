#!/usr/bin/env python3
"""One-shot: insert the initial `check:` blocks into gym.yaml.

Kept in-tree rather than run and thrown away so the six choices are reviewable
as code — which drills were judged machine-checkable, and the stated reason for
each. Re-running is idempotent: a drill that already has a `check:` is skipped.
"""

import re
import sys
import textwrap

GYM = "ops/gym/gym.yaml"

# Every `run` below was executed against this tree before being written down.
# A drill earns a check only when a command genuinely stands in for the
# exercise — not when one merely relates to the topic.
CHECKS = {
    "warrant-generates-the-acl": dict(
        run="python3 ops/warrant/generate.py --check",
        expect="exit-zero",
        why="This IS the drill — the registry's byte-identical check is the command "
            "the module asks you to run."),
    "ci-gates-you-will-meet": dict(
        run="ops/ci/check-adr-index-complete.sh",
        expect="exit-zero",
        why="Stands in for 'run one repository check against your working tree' with "
            "a check that is fast, offline and read-only. Deliberately an EXISTING "
            "check on main rather than one added alongside: a check pointing at a "
            "script from its own branch passes locally and fails the moment the two "
            "are reviewed separately."),
    "track-governance": dict(
        run="ops/work/work list",
        expect="exit-zero",
        why="The drill's first half is 'list the live tracks'. Deciding whether your "
            "work overlaps one is the half only a person can do."),
    "session-isolation": dict(
        run="git rev-parse --git-dir >/dev/null && git rev-parse --git-common-dir >/dev/null",
        expect="exit-zero",
        why="Verifies the two commands that answer the drill both work here. WHICH "
            "worktree you are in, and whether it is the right one, is the drill and "
            "needs a person. An earlier version ended in `|| echo master`, which made "
            "it exit 0 in every case — a check that cannot fail."),
    # DELIBERATELY NOT GIVEN A CHECK: commit-and-pr-conventions. Its drill is
    # "read the last five commits on the trunk and write a compliant subject
    # for your own change". The only checkable half is whether `git log` exits
    # zero, which tests git rather than the drill — and reaching for
    # `origin/main` to make it meaningful breaks under CI's shallow clone. A
    # check that tests the harness instead of the exercise is the failure mode
    # this schema exists to avoid, so this drill stays `human`.
    "mesh-acl-model": dict(
        run="python3 ops/ci/check-netbird-acl-parity.py",
        expect="exit-zero",
        why="Proves the rules you are about to read are the rules that are deployed. "
            "Reading them for YOUR group is the drill proper."),
    # MESH CHECKS — need a live goatnet, so run.py skips them unless --mesh.
    # Both go through the ACTIVE CONTEXT rather than naming a goatnet, which is
    # what makes them safe to keep in a per-goatnet file: hardcoding a hostname
    # here would make the check true on one goatnet and a lie on every other.
    # Fired live: 7/7.
    "health-surface": dict(
        run="goat health",
        expect="exit-zero",
        why="Reads the live health surface through the active context, so it works on "
            "any goatnet without naming one. FINDING a domain that is not green is the "
            "drill and needs a person."),
    "goat-top": dict(
        run="goat top --once",
        expect="exit-zero",
        why="Takes the snapshot the drill asks for. Identifying which field is "
            "deliberately blank — and why blank is the honest answer there — is the "
            "half a person does."),
}


def indent_of(s: str) -> int:
    return len(s) - len(s.lstrip())


def main() -> int:
    with open(GYM, encoding="utf-8") as source_file:
        lines = source_file.read().split("\n")
    out: list[str] = []
    added = 0
    i = 0
    while i < len(lines):
        m = re.match(r"^(\s*)- id: (\S+)\s*$", lines[i])
        if not (m and m.group(2) in CHECKS):
            out.append(lines[i])
            i += 1
            continue

        mid = m.group(2)
        base = indent_of(lines[i]) + 2

        # Locate this module's drill: block and the line after its last key.
        j, drill_at = i + 1, None
        while j < len(lines):
            if re.match(r"^\s*- id: ", lines[j]):
                break
            if re.match(rf"^\s{{{base}}}drill:\s*$", lines[j]):
                drill_at = j
            elif drill_at is not None and lines[j].strip() and indent_of(lines[j]) <= base:
                break
            j += 1

        if drill_at is None:
            out.append(lines[i])
            i += 1
            continue

        end = drill_at + 1
        while end < len(lines):
            if re.match(r"^\s*- id: ", lines[end]):
                break
            if lines[end].strip() and indent_of(lines[end]) <= base:
                break
            end += 1

        block = lines[i:end]
        if any(re.match(rf"^\s{{{base + 2}}}check:\s*$", ln) for ln in block):
            out.extend(block)          # already present — idempotent
            i = end
            continue

        out.extend(block)
        c = CHECKS[mid]
        pad = " " * (base + 2)
        out.append(f"{pad}check:")
        out.append(f'{pad}  run: "{c["run"].replace(chr(34), chr(92) + chr(34))}"')
        out.append(f"{pad}  expect: {c['expect']}")
        out.append(f"{pad}  why: >-")
        for w in textwrap.wrap(c["why"], 88):
            out.append(f"{pad}    {w}")
        added += 1
        i = end

    with open(GYM, "w", encoding="utf-8") as source_file:
        source_file.write("\n".join(out))
    print(f"added {added} check block(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
