#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
#
# promote-pending.py — flip this play's ledger row from `pending` to `deploy`.
#
# Run only from the rollback-ledger handler, which ansible executes at the END
# of a play and ONLY for hosts that did not fail. A refused deploy never reaches
# here, which is the whole point: its row stays `pending`.
#
# Scans from the end so that several plays touching one component in a day
# promote the row they actually wrote, not an older one.
#
# The row's `ts` is when the play STARTED (this role is a meta-dependency, so
# the row is written before the component's own tasks run). The promote stamps
# `promoted_at`, the host's UTC clock at the end of the play, so a reader can
# tell when the deploy actually finished; `ts` is kept as it was.
#
# --served '{"<payload>": "<file>", ...}' (a serving role: goat-get, goat-docs):
# read each file back now, at the end of the play, and record what the host
# serves as `served` {payload: tag}. A file that is not there is "absent"; one
# that does not parse, or names no tag, is "unreadable". The row then says what
# the host serves, not what the play's vars meant it to serve.

import datetime
import json
import os
import sys
import tempfile


def observed(spec: dict) -> dict:
    out = {}
    for name, f in sorted(spec.items()):
        if not os.path.exists(f):
            out[name] = "absent"
            continue
        try:
            with open(f) as fh:
                tag = json.load(fh).get("tag")
        except (ValueError, OSError, AttributeError):
            tag = None
        out[name] = tag if isinstance(tag, str) and tag else "unreadable"
    return out


def main() -> int:
    args = sys.argv[1:]
    served = None
    if "--served" in args:
        i = args.index("--served")
        if i + 1 >= len(args):
            sys.stderr.write("--served needs a JSON object\n")
            return 2
        served = json.loads(args[i + 1])
        del args[i:i + 2]
    if len(args) != 2:
        sys.stderr.write("usage: promote-pending.py <ledger-path> <ref> [--served JSON]\n")
        return 2
    path, ref = args

    if not os.path.exists(path):
        print("no ledger at %s; nothing to promote" % path)
        return 0

    with open(path) as fh:
        lines = fh.read().splitlines()

    for i in range(len(lines) - 1, -1, -1):
        if not lines[i].strip():
            continue
        try:
            row = json.loads(lines[i])
        except ValueError:
            # A hand-edited or truncated line is not ours to repair.
            continue
        if row.get("ref") == ref and row.get("action") == "pending":
            row["action"] = "deploy"
            row["promoted_at"] = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
            if served:
                row["served"] = observed(served)
            lines[i] = json.dumps(row)
            directory = os.path.dirname(path) or "."
            fd, tmp = tempfile.mkstemp(dir=directory)
            try:
                with os.fdopen(fd, "w") as out:
                    out.write("\n".join(lines) + "\n")
                os.chmod(tmp, 0o644)
                os.replace(tmp, path)
            except Exception:
                if os.path.exists(tmp):
                    os.unlink(tmp)
                raise
            print("promoted pending -> deploy for %s" % ref)
            return 0

    print("no pending row for %s; nothing to promote" % ref)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
