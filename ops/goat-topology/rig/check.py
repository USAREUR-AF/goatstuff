#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Hold the rig page to what the rig declared.

usage: check.py <declared.txt> <peer-names.json> <api-url>

Checks, and prints one line per router:
  - a router that sends no report and holds a session with a reporter is
    labeled with the peer name the directory gives its address;
  - one known only through link state has no peer and says why;
  - the organization, its nation and the individual are the roster's;
  - the subscribed keys a reporter attributes to it equal the subscriptions
    the rig's clients declared on it (and, for a gateway, on its pods);
  - each reporter's own census counts its own clients' declarations.
Exits 1 on any mismatch.
"""
import json
import sys
import urllib.request

declared_path, names_path, url = sys.argv[1:4]
rows = [l.split() for l in open(declared_path) if l.strip()]
# name idx zid publish subs pubs peername
R = {r[0]: {"zid": r[2], "publish": r[3], "subs": int(r[4]), "pubs": int(r[5]), "peer": r[6]} for r in rows}
PODS = {"charlie-gateway": ["charlie-pod-1", "charlie-pod-2"], "bravo-gateway": ["bravo-pod-1", "bravo-pod-2"]}
# What the synthetic roster records, by router (rig.sh `identity`).
OWNER = {
    "charlie-gateway": ("org-cedar", None, "rivera-j", "XCC"),
    "delta-1": ("org-birch", "XBB", "okafor-n", None),
    "echo-1": ("org-anvil", "XAA", "rivera-j", "XCC"),
    "echo-2": ("org-cedar", None, None, None),
    "echo-3": ("org-anvil", "XAA", "rivera-j", "XCC"),
    "echo-7": ("org-birch", "XBB", "okafor-n", None),
    "echo-9": ("org-anvil", "XAA", None, None),
}
NO_OWNER = {"echo-4", "echo-5"}
# What the record says, as rig.sh `owner_json` writes it (ADR 1249 states).
# Every reporting router states its own address (--self-address), so each is
# looked up directly and is "recorded".
STATE = {"echo-2": "lead", "echo-10": "lead", "echo-4": "no_record", "echo-5": "excepted",
         "delta-2": "not_read", "echo-11": "not_read"}
RECORD_ORG = {"charlie-gateway": ("cedar", ""), "delta-1": ("birch", "XBB"), "echo-1": ("anvil", "XAA"),
              "echo-3": ("anvil", "XAA"), "echo-6": ("anvil", "XAA"), "echo-7": ("birch", "XBB"),
              "echo-8": ("birch", "XBB"), "echo-9": ("anvil", "XAA")}
CAUSE = {"delta-2": "link_state_only", "echo-11": "not_in_directory"}
LINK_STATE_ONLY = {"delta-2"}
NOT_IN_DIRECTORY = {"echo-11"}
# The serve process subscribes once, on alpha-core-1.
EXTRA_SUBS = {"alpha-core-1": 1}

raw = urllib.request.urlopen(url).read().decode()
d = json.loads(raw)
by_zid = {x["zid"]: x for x in d["routers"]}
bad = 0


def ok(cond, msg):
    global bad
    print(("ok   " if cond else "FAIL ") + msg)
    if not cond:
        bad += 1


drawn = [n for n, r in R.items() if r["zid"] in by_zid]
ok(len(d["routers"]) == len(drawn) and len(drawn) >= 20, f"{len(drawn)} routers drawn")
for n, r in R.items():
    x = by_zid.get(r["zid"])
    if x is None:
        ok(n in ("charlie-pod-1", "charlie-pod-2"), f"{n}: not drawn (pod behind a silent gateway)")
        continue
    want_state = STATE.get(n, "recorded")
    ok(x.get("owner_state") == want_state, f"{n}: owner state {x.get('owner_state')!r}, rig declared {want_state!r} ({x.get('owner_reason')})")
    if n in CAUSE:
        ok(x.get("owner_cause") == CAUSE[n], f"{n}: not read because {x.get('owner_cause')!r}, rig {CAUSE[n]!r}")
    if n in RECORD_ORG:
        ro = (x.get("peer") or {}).get("owner") or {}
        ok((ro.get("org_short_tag"), ro.get("org_nation")) == RECORD_ORG[n],
           f"{n}: recorded org {ro.get('org_short_tag')!r} / {ro.get('org_nation')!r}, rig {RECORD_ORG[n]}")
    if r["publish"] != "-":
        c = x.get("census") or {}
        want_s = r["subs"] + EXTRA_SUBS.get(n, 0)
        ok(not c.get("read_error") and c.get("session_subscriptions") == want_s and c.get("session_publishers") == r["pubs"],
           f"{n}: own census {c.get('session_subscriptions')} subs / {c.get('session_publishers')} pubs, declared {want_s} / {r['pubs']}"
           f" (read in {c.get('read_ms')} ms)")
        continue
    peer = (x.get("peer") or {}).get("name")
    if n in LINK_STATE_ONLY:
        ok(peer is None and "link state" in x.get("name_note", ""), f"{n}: no peer name; note: {x.get('name_note')}")
    elif n in NOT_IN_DIRECTORY:
        ok(peer is None and "not in the peer directory" in x.get("name_note", ""), f"{n}: note: {x.get('name_note')}")
    else:
        ok(peer == r["peer"], f"{n}: labeled {peer!r}, directory says {r['peer']!r}")
    o = x.get("owner")
    if n in OWNER:
        want = OWNER[n]
        got = (o or {}).get("org"), (o or {}).get("org_nation"), (o or {}).get("individual"), (o or {}).get("individual_nationality")
        ok(got == want, f"{n}: owner {got}, roster {want}")
    elif n in NO_OWNER:
        ok(o is None, f"{n}: no owner (name leads to no roster handle)")
    want = r["subs"] + sum(R[p]["subs"] for p in PODS.get(n, []))
    h = x.get("held")
    got = h["subscribed_keys"] if h else None
    ok(got == want, f"{n}: {got} subscribed keys as {h and h['by']} sees it, declared {want}")

fs = {f["id"]: f for f in d["findings"]}
t14 = fs.get("T-014")
ok(t14 is not None and t14["severity"] == "critical"
   and R["echo-4"]["peer"] in t14["what"] and R["echo-5"]["peer"] not in t14["what"],
   "T-014 (critical) names the router with no recorded owner, and not the excepted one")
ok(t14 is not None and R["echo-2"]["peer"] in t14["what"],
   "T-014 counts a router named only by a roster lead, as the reconcile does")
ok("identity_id" not in raw and "ory-" not in raw, "no identity id reaches the browser")
t16 = fs.get("T-016")
ok(t16 is not None and t16["severity"] == "info" and R["echo-5"]["peer"] in t16["what"],
   "T-016 (info) lists the router under an owner exception: shown, not hidden")
e1 = by_zid[R["echo-1"]["zid"]]
ok(e1.get("owner_state") == "recorded" and "remove it" in (e1.get("owner_reason") or ""),
   "a stale owner exception stays recorded and says to remove it")
ok("T-015" in fs and all(n not in fs["T-015"]["what"] for n in ("machine-anvil-pod-7-rack2",)),
   "T-015 (warning) lists the routers whose owner could not be read, and no recorded one")

print(f"{bad} mismatch(es)")
sys.exit(1 if bad else 0)
