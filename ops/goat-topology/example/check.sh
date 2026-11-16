#!/bin/bash
# SPDX-License-Identifier: Apache-2.0
#
# Check that the example's page shows what the example runs: 3 routers, all
# reporting, 3 router-to-router links, every snapshot read without error, and
# no critical or warning finding. Run from this directory after
# `docker compose up -d`. CI runs it, so the README's quick start cannot rot.
#
#   ./check.sh [timeout-seconds]      (default 120)
#
# A stock zenohd 1.9.0 router has been seen to stop answering right after start
# (README, "Known issues"). If a router is still unhealthy halfway through the
# wait, this restarts it once and says so; a second failure fails the check.
set -u
cd "$(dirname "$0")"
limit="${1:-120}"
url="http://127.0.0.1:${TOPOLOGY_PORT:-8080}/api/topology"
dc() { docker compose "$@"; }
restarted=""
start=$(date +%s)
verdict() {
  local body
  body="$(curl -fsS --max-time 5 "$url" 2>/dev/null)" || { echo "the page at $url is not answering yet"; return 1; }
  printf '%s' "$body" | python3 -c '
import json, sys
d = json.load(sys.stdin)
c = d["counts"]
errs = [r["node"] + ": " + r["observe_error"] for r in d["routers"] if r.get("observe_error")]
bad = [f["id"] + " " + f["severity"] + " " + f["subject"] for f in d["findings"] if f["severity"] in ("critical", "warning")]
print("routers=%d reporting=%d links=%d errors=%d critical+warning=%d" % (c["routers"], c["reporting"], len(d["edges"]), len(errs), len(bad)))
for e in errs + bad: print("  " + e)
# The routers of this example, so a different page on the same port cannot pass.
names = sorted((r["site"], r["node"]) for r in d["routers"])
want = [("example", "router-" + x) for x in "abc"]
if names != want: print("  routers on the page: %s; expected %s" % (names, want))
ok = names == want and c["routers"] == 3 and c["reporting"] == 3 and len(d["edges"]) == 3 and not errs and not bad
sys.exit(0 if ok else 1)
'
}
while :; do
  now=$(date +%s)
  if out="$(verdict)"; then
    echo "PASS: $out"
    # A router that docker restarted is not a failure of this check, but it is
    # worth seeing: stock zenohd 1.9.0 can panic on a route-table read
    # (README, "Known issues").
    for r in router-a router-b router-c; do
      n="$(docker inspect -f '{{.RestartCount}}' "$(dc ps -q "$r")" 2>/dev/null)"
      if [ -n "$n" ] && [ "$n" != "0" ]; then
        echo "note: $r was restarted $n time(s); its log:"
        dc logs "$r" 2>&1 | grep -E "panicked|index out of bounds" | head -3
      fi
    done
    exit 0
  fi
  if [ $((now - start)) -ge "$limit" ]; then
    echo "FAIL after ${limit}s: ${out:-the page did not answer at $url}"
    dc ps
    for s in router-a router-b router-c publish-a publish-b publish-c serve; do
      echo "--- $s"; dc logs --tail 15 "$s" 2>&1
    done
    exit 1
  fi
  if [ -z "$restarted" ] && [ $((now - start)) -ge $((limit / 2)) ]; then
    for r in router-a router-b router-c; do
      if [ "$(docker inspect -f '{{.State.Health.Status}}' "$(dc ps -q "$r")" 2>/dev/null)" = "unhealthy" ]; then
        echo "note: $r stopped answering its adminspace; restarting it once (README, Known issues)"
        dc restart "$r" >/dev/null 2>&1
        restarted="$restarted $r"
      fi
    done
    [ -n "$restarted" ] || restarted="none"
  fi
  sleep 3
done
