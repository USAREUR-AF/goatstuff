#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
#
# peerview-poll — who is on this goatnet, and what are they asking of it.
#
# Sibling of netview-poll.py (ADR 1128). Same shape, same discipline, different
# question: netview asks "how full is the pipe and what is filling it"; this
# asks "WHO is filling it, what are they running, and what are they costing".
#
# WHY A POLLER AND NOT A BROWSER FETCH: identical reasoning to netview. The
# pages are static files behind oauth2-proxy on the isitup nginx. Letting the
# browser query the NetBird management API would put admin-token demand on a
# path per viewer, and letting it query Prometheus would mean exposing a
# loopback-only service. This runs once per interval on the host, writes one
# document, and every viewer reads the same bytes — the ADR 1123 shape: client
# demand must not amplify into backend work.
#
# FOUR SOURCES, EACH INDEPENDENTLY DEGRADABLE. This is the part that matters,
# because the two goatnets this ships to have DIFFERENT telemetry and neither
# is a subset of the other (measured):
#
#   netbird   NetBird management API. The census. Works on both goatnets.
#   traefik   Traefik's JSON access log, read from the container. Per-peer x
#             per-service demand, joined on the mesh IP. Works on both.
#   prom      Prometheus. Transport class, per-peer bytes, service percentiles.
#             Which of these a goatnet has varies: one can have traefik +
#             goat_net_* attribution + blackbox and NO goat_mesh_* path quality;
#             another can have goat_mesh_* and NO traefik metrics, NO attribution,
#             NO blackbox — and its Prometheus can be on a different host and a
#             different port.
#   ledger    A local JSONL this poller appends to. The ONLY source of
#             day-over-day and month-over-month shape, because Prometheus
#             retention is 15 days (docker-compose.yml
#             --storage.tsdb.retention.time=15d) and a month-to-month
#             trajectory drawn from a fortnight of data would be a lie.
#
# Every source records whether it answered. A source that could not be reached
# blanks its panels and SAYS SO — it never renders as zero.
#
# HONESTY RULES, enforced here rather than in the pages:
#   - a metric with no series is null, never 0. The page renders null as
#     "not measured" and 0 as zero, and they are different facts.
#   - a count derived from a source that did not answer is null, not 0.
#   - a share whose denominator is unattributable is reported WITH the
#     attributable fraction beside it, never silently over the smaller base.
#   - nothing here infers a person's affiliation, location, or organisation
#     from a name. See `handle_of()`.
#
# Written to <out>.tmp then renamed, so nginx never serves a half-written doc.

import argparse
import collections
import fnmatch
import json
import math
import os
import re
import statistics
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request

# The shared peer -> person client. Vendored by path rather than by package
# because the poller is a single file copied onto a mgmt host; the deploy ships
# this module beside it (ops/ansible/roles/peerview).
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
try:
    import peerowner
except ImportError:                                          # pragma: no cover
    peerowner = None

DEFAULT_PROM = "http://127.0.0.1:9192"
RI = "5m"

# ── the census axes ─────────────────────────────────────────────────────────
# Dormancy thresholds in days. Chosen to match dormancy-report's default
# (7,14,30) so the two surfaces cannot disagree about what "dormant" means.
DORMANCY_DAYS = (7, 14, 30)

# Privilege order for picking ONE primary role per peer, most-privileged first.
# A peer commonly holds several groups (DEVELOPER + OBSERVER is the standard
# full-participation pair, docs/design/mesh-and-ory-roles.md), so a
# by-role chart that counted memberships would total more than the population.
# This picks one, and the page says which rule picked it.
ROLE_PRIORITY = [
    "PLATFORM-ADMIN", "INFRA", "BACKEND-INFRA", "EXTERNAL-INFRA",
    "NETWORK", "BREAKGLASS", "MONITOR", "DEVELOPER", "OBSERVER",
    "CONTROL", "FUSION", "SENSOR-ARBITER", "EFFECTOR-ARBITER",
    "SENSOR-TASKING", "EFFECTOR-COMMAND", "EFFECTOR-TRACK", "EFFECTOR-CUE",
    "SENSOR-FEED", "SENSOR-TELEMETRY", "EFFECTOR-TELEMETRY",
    "KID", "EXTERNAL-KID", "EXTERNAL-INTERMEDIARY", "POD", "MOON",
    "SENSOR", "EFFECTOR", "OPSYSTEM", "QUARANTINED",
]
# Capability groups, deliberately NOT roles (mesh-and-ory-roles.md: "P2P is a
# capability, not a role"). Counted separately so graduation is visible without
# polluting the role census.
CAPABILITY_SUFFIXES = {"P2P"}
# Never a role and never a scope: the implicit all-peers group.
UNIVERSAL_GROUPS = {"All"}


# ══════════════════════════════════════════════════════════════════════════
#  Small helpers
# ══════════════════════════════════════════════════════════════════════════

def _get_json(url, headers=None, timeout=20):
    """GET -> parsed JSON, or None. None means COULD NOT ASK."""
    req = urllib.request.Request(url, headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return json.load(r)
    except Exception as e:
        print(f"peerview-poll: GET {url.split('?')[0]} failed: {e}", file=sys.stderr)
        return None


def parse_ts(s):
    """RFC3339 -> unix seconds, or None. NetBird zeroes this for never-seen."""
    if not s or s.startswith("0001-01-01"):
        return None
    s = s.strip().replace("Z", "+00:00")
    # Python's fromisoformat is fussy about sub-second precision > 6 digits,
    # which netbird emits. Trim rather than fail the whole peer.
    m = re.match(r"^(.*\.\d{1,6})\d*([+-]\d{2}:\d{2})$", s)
    if m:
        s = m.group(1) + m.group(2)
    try:
        import datetime
        return datetime.datetime.fromisoformat(s).timestamp()
    except Exception:
        return None


def pctile(values, q):
    """Nearest-rank percentile. Returns None for an empty sample rather than
    raising — an absent percentile is a fact, an exception is a blank page."""
    if not values:
        return None
    vs = sorted(values)
    i = min(len(vs) - 1, max(0, int(round(q * (len(vs) - 1)))))
    return vs[i]


def topn(counter_like, n, key="name"):
    """dict -> sorted [{name, value}], biggest first."""
    return [{key: k, "value": v}
            for k, v in sorted(counter_like.items(), key=lambda kv: -kv[1])[:n]]


# ══════════════════════════════════════════════════════════════════════════
#  Source 1 — the NetBird management API (the census)
# ══════════════════════════════════════════════════════════════════════════

# The mint-name grammar produced by `goat admin onboard`:
#     peer-<handle>-<env>-<YYYY-MM-DD>-<HOSTNAME>
# A peer whose name matches carries a PERSON's mint handle; one that does not
# is a machine enrolled by a setup key with no person in its name.
#
# THIS IS THE ONLY PERSON AXIS AVAILABLE, and the reason is worth writing down
# rather than discovering again: `user_id` is EMPTY on peers
# enrolled with a setup key, and every peer is,
# rather than by a user login.
# So the management API carries no peer->person edge at all, and the name is
# what is left.
#
# WHAT THIS DOES NOT DO: it does not read an organisation, an affiliation, an
# employer or a place out of the handle. Handles frequently look like they
# encode one. They do not — affiliation is an identity-plane attribute and is
# NEVER a mesh group nor a name component you may parse (identity-grammar.md).
# The handle is used ONLY as an opaque key for "these peers belong to the same
# person", and it is rendered verbatim, never decomposed.
# ANCHORED ON THE DATE, GREEDILY. A non-greedy handle followed by a permissive `<env>` pattern lets the
# env swallow part of the handle: `peer-unit-pl-008-galaxy-2030-01-11-BOX` matches
# with handle `unit` and env `pl-008-galaxy`, so `unit-pl-008` and `unit-pl-500` —
# two different machines belonging to two different mint records — merge into
# one "person". A person count that silently merges people
# is worse than no person count.
#
# So: take everything between `peer-` and the date, then remove the goatnet
# token from the end if it is there. When the goatnet is not configured or the
# name carries a different env spelling, the env stays in the key. That is
# deliberate — it is uglier and it is still correct, because grouping only
# needs the key to be stable and unique per person.
MINT_NAME_RE = re.compile(r"^peer-(.+)-\d{4}-\d{2}-\d{2}-", re.I)


def handle_of(peer_name, goatnet=None):
    m = MINT_NAME_RE.match(peer_name or "")
    if not m:
        return None
    h = m.group(1)
    if goatnet:
        suffix = "-" + goatnet
        if h.lower().endswith(suffix.lower()):
            h = h[: -len(suffix)]
    return h or None


# The portal's delegated machine enrollment mints the mesh setup key as
# "machine-"+handle and NetBird appends "-<hostname>" (goat-zenoh-portal
# api/machine_enroll.go). Same accepted forms, and the same longest-handle rule,
# as the portal's access.MatchMachineHandle.
MACHINE_PREFIX = "machine-"


def machine_handle_of(peer_name, handles):
    """The machine a `machine-<handle>[-<hostname>]` peer was minted for, or None."""
    name = peer_name or ""
    if not name.startswith(MACHINE_PREFIX):
        return None
    rest = name[len(MACHINE_PREFIX):]
    best = None
    for h in handles:
        if h and len(h) > len(best or "") and (rest == h or rest.startswith(h + "-")):
            best = h
    return best


def name_shape(peer_name):
    """What a peer name claims to be: `person` (the onboarding grammar),
    `machine` (the portal's machine enrollment) or `none`. Evidence about the
    name only, never about who owns the peer."""
    name = peer_name or ""
    if name.startswith("peer-"):
        return "person"
    if name.startswith(MACHINE_PREFIX):
        return "machine"
    return "none"


# ── the owner join ──────────────────────────────────────────────────────────
# WHO IS RESPONSIBLE FOR THIS PEER. The census above gives a handle and nothing
# else; a handle is a grouping key, not an accountable identity.
#
# THIS SURFACE DOES NOT INVENT ITS OWN ANSWER. peer → person is a solved,
# deployed problem with ONE resolver — the portal's access graph, `POST
# /api/access/peers` (docs/design/peer-identity-resolution.md, Option B) — which
# `goat who`, panoscope /who, netvis and the relay-health card all already
# consume. A fifth re-derivation on this page would be a fifth chance to
# disagree with the other four about who owns a machine. So the primary source
# here is that resolver, through the shared client ops/identity/peerowner.py,
# and this surface adopts its four-state vocabulary verbatim:
#
#   registry   roster handle matched AND a live Ory identity shares its
#              email-hash. The recorded linkage; safe to present as the owner.
#   name       handle matched the roster, no live identity did. Org known,
#              person not. A lead, never an answer.
#   unmatched  the name carries no principal — infra, synth, hand-named hosts.
#              Not a failure, and not "nobody is responsible".
#   unread     we could not ask. Not evidence of anything about the peer.
#
# TWO THINGS THE SHARED RESOLVER DOES NOT ANSWER, and both are supplied here
# rather than by parsing something differently:
#
#   custody    a peer minted for a MACHINE resolves to the machine's own handle,
#              which can never answer for anything. The person who can is its
#              custodian — the person→system custody edge (ADR 1096), carried in
#              docs/operators/roster.json and not yet in the resolver's answer.
#              A machine row therefore names the custodian and says so.
#   declared   substrate — the mgmt host, a relay, a kiosk box — carries no
#              onboarding name and never will, so the resolver correctly returns
#              `unmatched` forever. Config declares who owns it. A FALLBACK
#              only: it never overrides a resolved answer.
#
# Both are gaps in the shared resolver, filed as such in
# docs/design/peer-identity-resolution.md rather than treated as this surface's
# private business.
#
# THE LOCAL ROSTER IS THE OFFLINE FALLBACK, AND IT CAN ONLY EVER CLAIM `name`.
# When the portal cannot be asked, the same roster the resolver reads is on disk
# here, so the handle join still runs — but `registry` requires reconciling
# against a LIVE Ory identity, which only the resolver does. A local answer that
# claimed `registry` would be the exact indistinguishability ADR 1051 exists to
# prevent.
OWNER_KINDS = ("registry", "name", "custody", "team", "declared", "unmatched",
               "ambiguous", "unread")

# Infrastructure answers to a TEAM, and the mesh already says which peers those
# are. A peer holding an infra-class role carries no person in its name and
# never will — but "nobody is answerable for the mgmt host" is false, and
# leaving it blank was the wrong answer to a question the ACL can already
# answer: the platform-admin group of the peer's own scope.
#
# DERIVED, NOT LISTED, and that is the point. A hand-kept glob list of infra
# hostnames goes stale the day someone adds a host; the role assignment is a
# thing an admin must set anyway for the peer to work at all.
#
# ONLY these roles. A peer with no principal in its name but a non-infra role
# stays unmatched: handing it to the platform-admin team would manufacture an
# owner rather than find one.
#
# WHAT A GROUP IS AND IS NOT. It says what a peer is AUTHORIZED for. It does not
# say what the machine is, what it does, or who runs it, and it does NOT expire
# when the machine's job changes — a repurposed host keeps the groups it was
# enrolled with, so its labels describe the job it used to do. That is fine for
# THIS use, because the group is a live authorization edge: whoever administers
# the scope can act on the peer whether or not the label still fits. It is not
# fine as a description. Nothing derived here may be rendered, logged or written
# up as a claim about a machine's purpose — that mistake has been made in this
# repo's own prose about a specific host, and the group looked authoritative
# because it was current in the ACL and stale about the world.
# BREAKGLASS is here for the same accountability reason, not because it is
# "infrastructure": the taxonomy defines it as operator reach-in of last resort
# and explicitly NOT delegable — "the entitlement that lets a site admin enroll
# a switch must not let them enroll their own way into the rescue path"
# (ADR 1114). An operator-enrolled, non-delegable node is the platform admins'
# by construction, so a rescue node must not read as unowned.
#
# NETWORK is deliberately ABSENT. Bearer gear IS delegable — a site-scoped
# network_admin entitlement drives it (ADR 1113) — so its accountable group is
# not the platform admins, and naming them would be the same manufactured
# answer this tier exists to avoid. Gear classes (SENSOR/EFFECTOR/KID/POD/
# CONTROL) are custodian-owned (ADR 1096) and stay out for the same reason.
INFRA_ROLES = ("INFRA", "BACKEND-INFRA", "EXTERNAL-INFRA", "BREAKGLASS")

# The suffix that names a scope's accountable admin group. Paired with the
# peer's own scope, so an EX-DEMO node names that exercise's admins rather
# than the host goatnet's.
PLATFORM_ADMIN_SUFFIX = "PLATFORM-ADMIN"


# How old a declared-registry artifact may be before the surface says so.
#
# WHY A GRADE AND NOT A DATE. This page printed `generated_at` verbatim, and a
# date is not a judgement: a roster a week behind the repository renders as
# "registry file generated <date>" — true, unalarming, and live people read as
# "not in roster". The artifact already carries
# everything needed to grade it (`generated_at` + `source_digest`, stamped by
# gen-roster.py); nothing on this surface looked.
#
# The thresholds are deliberate, not taste. The first pass used
# 10/21 days and graded the live case `fresh`: peerview's copy was seven days
# and 110 people behind while the portal on the SAME host served the current
# artifact. A rule that calls the thing it was written for healthy is not a
# rule, so the bar is where a week-old artifact reads as `ageing`.
#
# `stale` is a prompt to check, never an assertion that the file is wrong —
# `goat access strays` compares the served `source_digest` against a checkout,
# which is the question this surface cannot answer from the host.
ROSTER_AGEING_DAYS = 3
ROSTER_STALE_DAYS = 10


def roster_freshness(generated_at, now=None):
    """(age_days, grade) for a roster stamp: `fresh` | `ageing` | `stale`.

    An unparseable or absent stamp grades `unknown` with no age — an artifact
    that predates the provenance stamp is not old, it is unmeasured, and the two
    must not render the same (the same rule the owner column follows for
    `unread` versus `unmatched`).
    """
    ts = parse_ts(generated_at)
    if not ts:
        return None, "unknown"
    age = max(0.0, ((now if now is not None else time.time()) - ts) / 86400.0)
    if age >= ROSTER_STALE_DAYS:
        return age, "stale"
    if age >= ROSTER_AGEING_DAYS:
        return age, "ageing"
    return age, "fresh"


def load_roster(path):
    """The declared owner registry — docs/operators/roster.json.

    Returns a dict that always answers `ok`. A missing or unparseable file is
    not fatal and not silent: every peer's owner then reads `unknown` and the
    surface says the registry did not load, because "we did not look" and "we
    looked and nobody is declared" are different findings.

    Handles repeat across the file — the same person appears under each org
    they were minted into, and per-env manifests re-declare them. An ACTIVE
    entry wins over a non-active one so a person who is active somewhere is not
    reported as departed; ties keep the first, and the repeat count is carried
    so the surface can say the registry is layered rather than pretending each
    handle appears once.
    """
    out = {"ok": False, "reason": None, "generated_at": None,
           "source_digest": None, "age_days": None, "freshness": "unknown",
           "member_count": 0, "repeated_handles": 0, "by_handle": {}}
    try:
        with open(path) as f:
            raw = json.load(f)
    except FileNotFoundError:
        out["reason"] = f"not on this host at {path}"
        return out
    except (OSError, ValueError) as e:
        out["reason"] = f"{path} did not read or parse ({e})"
        return out

    orgs = raw.get("orgs")
    if not isinstance(orgs, list):
        out["reason"] = f"{path} carries no orgs list — wrong file, or a format change"
        return out

    by_handle = {}
    repeats = 0
    for org in orgs:
        org_id = (org or {}).get("id")
        for m in (org or {}).get("members") or []:
            h = m.get("handle")
            if not h:
                continue
            custody = m.get("custody")
            # gen-roster writes an em dash for "no custody edge recorded". Treat
            # that as absent rather than as a handle named "—".
            if custody in ("—", "-", ""):
                custody = None
            tag = (m.get("vendor_tag") or "").strip()
            entry = {
                "handle": h,
                "type": m.get("type"),
                "state": m.get("state"),
                "org": org_id,
                "custody": custody,
                # EVERY tag this handle was ever declared under, not just the
                # winning entry's. The tag is part of the minted peer name, so a
                # person re-declared under a new org keeps peers carrying the
                # OLD tag; matching only the current one would leave those peers
                # reading as ownerless.
                "tags": {tag} if tag else set(),
            }
            prev = by_handle.get(h)
            if prev is None:
                by_handle[h] = entry
                continue
            repeats += 1
            prev["tags"] |= entry["tags"]
            if prev.get("state") != "active" and entry.get("state") == "active":
                entry["tags"] = prev["tags"]
                by_handle[h] = entry

    generated_at = raw.get("generated_at")
    age_days, freshness = roster_freshness(generated_at)
    out.update(ok=True, generated_at=generated_at,
               # Carried, not interpreted: only a reader with a checkout can say
               # whether this digest is behind (`goat access strays`), and this
               # host has none.
               source_digest=raw.get("source_digest"),
               age_days=age_days, freshness=freshness,
               member_count=len(by_handle), repeated_handles=repeats,
               by_handle=by_handle)
    return out


def load_declared_owners(path):
    """Peers that will never carry an onboarding name, and who owns them anyway.

    The mgmt host, the relays, a kiosk box: enrolled with a setup key, no person
    in the name, and no manifest — so the registry join can never resolve them
    and the surface correctly says nothing records an owner. That is true and it
    is not good enough, because somebody does own the mgmt host.

    THE ONE OTHER PLACE THE ANSWER COULD COME FROM, and why it is not used: the
    NetBird activity log records the enrolment (`peer.setupkey.add`), but for a
    setup-key enrolment the initiator IS THE KEY — `opEvent.InitiatorID = sk.Id`
    (netbird management/server/peer.go). So the log attributes a peer to a setup
    key, and a shared key names nobody. It is a lead, not an owner, which is why
    the owner of substrate is DECLARED here instead of inferred there.

    Format, one entry per line, `<peer-name-or-glob>: <handle>`; `#` comments.
    Deliberately not YAML-parsed — the poller has no yaml dependency on the
    mgmt hosts and this file is two columns.
    """
    out = {"ok": False, "reason": None, "rules": []}
    try:
        with open(path) as f:
            lines = f.readlines()
    except FileNotFoundError:
        out["reason"] = f"no declared-owner file at {path}"
        return out
    except OSError as e:
        out["reason"] = f"{path} did not read ({e})"
        return out
    for raw in lines:
        line = raw.split("#", 1)[0].strip()
        if not line or ":" not in line:
            continue
        pat, handle = line.split(":", 1)
        pat, handle = pat.strip(), handle.strip()
        if pat and handle:
            out["rules"].append((pat, handle))
    # Longest pattern first so a specific host beats the class glob it sits in.
    out["rules"].sort(key=lambda r: len(r[0]), reverse=True)
    out["ok"] = True
    return out


def declared_owner(peer_name, declared):
    for pat, handle in declared.get("rules", ()):
        if fnmatch.fnmatchcase(peer_name or "", pat):
            return handle, pat
    return None, None


def belongs_to(peer_name, handle, org_tag, site):
    """`belongs_to` from ops/identity/peer-name-grammar.yaml (v1.1.0).

    Does this peer name belong to exactly this principal on this goatnet? The
    SoT's own words for why this and not a looser matcher: it is anchored on the
    FULL keyed prefix — handle, then the optional org tag, then the site — so an
    org-mate carrying the same shared tag mid-name is never attributed, and a
    shorter handle never claims a longer one's peer (`maintainer` vs `maintainer-smith`, the
    class that mis-attributes).

    Both the tagged and the untagged form are accepted when the tag EQUALS the
    handle: peers minted before v1.1.0 carry the doubled segment, and refusing
    them would report their owner as undeclared.

    This is a Python reading of a grammar whose generated implementations are Go
    and Rust — there is no Python emitter — so it is pinned against the SoT's
    own conformance vectors in test_peerview.py rather than trusted.
    """
    if not peer_name or not peer_name.startswith("peer-"):
        return False
    rest = peer_name[len("peer-"):]
    tag = (org_tag or "").strip()
    if tag and tag != handle:
        return rest.startswith(f"{handle}-{tag}-{site}-")
    if rest.startswith(f"{handle}-{site}-"):
        return True
    # The pre-v1.1.0 doubled segment, only when the tag was the handle.
    return bool(tag) and rest.startswith(f"{handle}-{handle}-{site}-")


def _owner(kind, handle=None, **kw):
    row = {"kind": kind, "handle": handle, "claimed": None, "source": None,
           "org": None, "state": None, "ory_id": None, "id_console_path": None,
           "custody_of": None, "shape": None, "why": None}
    row.update(kw)
    return row


def _custody_upgrade(row, handle, roster):
    """A machine cannot answer for itself; its custodian can.

    The resolver attributes a machine's peer to the machine's own declared
    handle, which is correct and useless to an operator holding a question. The
    person→system custody edge (ADR 1096) is in the roster, so a `system` row is
    re-pointed at its custodian and SAYS it was — never silently, because "this
    is Ana's machine" and "Ana enrolled this machine" are different claims and
    only the second one is recorded.
    """
    e = roster.get("by_handle", {}).get(handle) if roster.get("ok") else None
    if not e or e.get("type") != "system":
        return row
    if not e.get("custody"):
        return _owner("unmatched", claimed=handle, source=row.get("source"),
                      org=e.get("org"), state=e.get("state"), shape="machine",
                      why=f"{handle} is declared as a system with no custodian "
                          "recorded, so the registry knows the machine but not "
                          "who answers for it")
    return _owner("custody", handle=e["custody"], claimed=handle,
                  source=row.get("source"), org=e.get("org"),
                  state=e.get("state"),
                  why=f"{handle} is a machine; its custodian enrolled it")


def team_owner(role, scope, groups_declared):
    """The team answerable for an infra peer: its scope's platform admins.

    `groups_declared` is the live group list from the management API — the
    group is named only if it EXISTS. A scope whose admin group was never
    created gets no owner rather than a plausible-looking one pointing at
    nothing, which is the same rule the rest of this surface follows.

    Membership is deliberately NOT required. An empty scope-local
    PLATFORM-ADMIN is the normal state at standup (the group is instantiated
    for every scope by template), and it means nobody has been appointed yet —
    which is a real thing to see, not a reason to report the peer as ownerless.
    """
    if role not in INFRA_ROLES or not scope:
        return None
    group = f"{scope}-{PLATFORM_ADMIN_SUFFIX}"
    if groups_declared is not None and group not in groups_declared:
        return None
    return group


def owner_of(peer_name, claimed_handle, roster, site, declared=None, portal_row=None,
             role=None, scope=None, groups_declared=None):
    """Resolve a peer to the identity responsible for it.

    Order, and it is the whole design: the SHARED RESOLVER first, the local
    roster only when the resolver could not be asked, and the declared config
    only for peers no resolver will ever answer for.

    `claimed_handle` is `handle_of(peer_name)` — this surface's loose,
    date-anchored read, used elsewhere for grouping. It is carried through so an
    unresolved row can say WHAT it could not resolve, and is never itself
    treated as an owner.
    """
    declared = declared or {}
    row = None

    # ── 1. the shared resolver ─────────────────────────────────────────────
    if portal_row and portal_row.get("attribution") == "custody":
        # A machine the portal enrolled. The resolver already re-pointed it at
        # the custodian from the registry's custody edge, which this surface's
        # roster file cannot see (portal-enrolled machines are not manifests).
        machine = portal_row.get("custody_of")
        return _owner("custody", handle=portal_row["handle"], claimed=machine,
                      source="portal-resolver", org=portal_row.get("org"),
                      state=portal_row.get("status"),
                      ory_id=portal_row.get("ory_id"),
                      id_console_path=portal_row.get("id_console_path"),
                      custody_of=machine,
                      why=f"{machine} is a machine enrolled through the portal; "
                          "its custodian enrolled it")
    if portal_row and portal_row.get("attribution") in ("registry", "name"):
        row = _owner(portal_row["attribution"], handle=portal_row["handle"],
                     claimed=portal_row["handle"], source="portal-resolver",
                     org=portal_row.get("org"), state=portal_row.get("status"),
                     ory_id=portal_row.get("ory_id"),
                     id_console_path=portal_row.get("id_console_path"),
                     why=None if portal_row["attribution"] == "registry"
                         else "the roster names this handle; no live identity "
                              "matched it, so this is a lead rather than a "
                              "confirmed person")
        return _custody_upgrade(row, portal_row["handle"], roster)

    asked = bool(portal_row) and portal_row.get("attribution") != "unread"

    # ── 2. the local roster, when the resolver could not be asked ──────────
    if not asked and roster.get("ok"):
        matches = [e for e in roster["by_handle"].values()
                   if belongs_to(peer_name, e["handle"], None, site)
                   or any(belongs_to(peer_name, e["handle"], t, site)
                          for t in e["tags"])]
        if not matches:
            # A declared system whose peer carries the portal's machine form.
            mh = machine_handle_of(peer_name, [e["handle"] for e in roster["by_handle"].values()
                                               if e.get("type") == "system"])
            if mh:
                matches = [roster["by_handle"][mh]]
        if len(matches) > 1:
            # Not reachable through the grammar alone (the prefixes are
            # distinct), but a registry can declare overlapping handles. Naming
            # the candidates is honest; picking one is how the wrong person gets
            # asked about somebody else's machine.
            return _owner("ambiguous", claimed=claimed_handle,
                          source="local-roster",
                          why="the declared registry matches this name to more "
                              "than one identity ("
                              + ", ".join(sorted(m["handle"] for m in matches))
                              + "), so none is asserted")
        if matches:
            e = matches[0]
            row = _owner("name", handle=e["handle"], claimed=e["handle"],
                         source="local-roster", org=e.get("org"),
                         state=e.get("state"),
                         why="matched against the registry file on this host; "
                             "the resolver that reconciles against live "
                             "identities could not be asked")
            return _custody_upgrade(row, e["handle"], roster)

    # ── 3. infrastructure answers to a team, per the mesh's own role ───────
    team = team_owner(role, scope, groups_declared)
    if team:
        return _owner("team", handle=team, claimed=claimed_handle, source="mesh-role",
                      org=None, state=None,
                      why=f"infrastructure: this peer holds the {role} role in "
                          f"{scope}, so the scope's platform admins answer for it. "
                          "No one person does.")

    # ── 4. declared substrate, for peers no resolver will ever answer for ──
    dh, pat = declared_owner(peer_name, declared)
    if dh:
        known = dh in roster.get("by_handle", {}) if roster.get("ok") else None
        return _owner("declared", handle=dh, claimed=claimed_handle,
                      source=f"declared ({pat})",
                      why=None if known is not False else
                          f"declared owner {dh} is not itself a declared identity "
                          "in the registry — the line names somebody the registry "
                          "does not know")

    # ── 5. nothing answered ────────────────────────────────────────────────
    if not asked and not roster.get("ok"):
        return _owner("unread", claimed=claimed_handle,
                      why="neither the resolver nor the registry file could be "
                          "read on this host, so nothing is known about this "
                          "peer's owner either way — "
                          + (roster.get("reason") or "reason not recorded"))
    # Three different jobs share this kind, told apart by what the name says.
    # The page labels them from `shape`; `why` names the remedy.
    shape = name_shape(peer_name)
    if shape == "machine":
        # The resolver's reason only when it recognised the machine; a resolver
        # that predates machine support says "no roster handle matched", which
        # is true and names nothing to do.
        resolver_saw_it = asked and (portal_row or {}).get("custody_of")
        why = ((portal_row or {}).get("reason") if resolver_saw_it else None) or (
            "the name says this is a machine enrolled through the portal, but no "
            "enrolled machine or declared system with a custodian matches it — "
            "revoked, re-enrolled under another handle, or a custodian the "
            "roster does not know")
    elif shape == "person":
        why = ("the name follows the onboarding grammar but matches no declared "
               "identity on this goatnet"
               + (f" (it reads as handle {claimed_handle})" if claimed_handle else "")
               + " — for example a person onboarded after the roster was last "
                 "regenerated, whose manifest was never synced; otherwise a "
                 "rename or an enrolment that predates the registry")
    else:
        why = ("the name carries no principal, so this peer never came through "
               "the operator onboarding path — declare an owner for it if "
               "somebody is responsible for it")
    return _owner("unmatched", claimed=claimed_handle, source=row and row.get("source"),
                  shape=shape, why=why)


def load_taxonomy(path):
    """Site prefixes + declared suffixes from netbird_acl_taxonomy.yml.

    Group names are `<SITE-PREFIX>-<SUFFIX>` (ops/ansible/filter_plugins/
    acl_scope.py). Splitting on the FIRST dash is wrong and quietly so:
    `SITE-PROD-PLATFORM-ADMIN` would yield scope `SITE` and role
    `PROD-PLATFORM-ADMIN`, i.e. a scope that does not exist and a role that
    does not exist. So the prefixes
    are read from the taxonomy and matched LONGEST-FIRST.
    """
    try:
        import yaml
        with open(path) as fh:
            doc = yaml.safe_load(fh) or {}
    except Exception as e:
        print(f"peerview-poll: taxonomy unreadable ({e}) — "
              f"roles will read as unresolved", file=sys.stderr)
        return {"prefixes": [], "suffixes": set(), "ok": False}
    prefixes, suffixes = [], set()
    # TWO top-level keys carry prefixes, and both must be read:
    # `netbird_acl_taxonomy` holds the persistent
    # goatnets and `netbird_exercises` holds the activity and forward-site scopes stamped
    # onto a host goatnet's mgmt server. Miss the second and every peer of a
    # forward site reads as having no declared role.
    for key in ("netbird_acl_taxonomy", "netbird_exercises"):
        for _site, cfg in (doc.get(key) or {}).items():
            if not isinstance(cfg, dict):
                continue
            p = cfg.get("site_prefix")
            if p:
                prefixes.append(p)
            for g in cfg.get("groups") or []:
                if isinstance(g, dict) and g.get("suffix"):
                    suffixes.add(g["suffix"])
    # Longest first so SITE-PROD wins over SITE.
    prefixes.sort(key=len, reverse=True)
    return {"prefixes": prefixes, "suffixes": suffixes, "ok": bool(prefixes)}


def split_group(name, tax):
    """'SITE-PROD-PLATFORM-ADMIN' -> ('SITE-PROD', 'PLATFORM-ADMIN').

    Returns (None, None) for the universal group and (None, name) when no
    declared prefix matches — an unscoped group is a real thing (`mesh-dns`)
    and is reported as such rather than forced into a scope.
    """
    if name in UNIVERSAL_GROUPS:
        return None, None
    for p in tax["prefixes"]:
        if name == p:
            return p, None
        if name.startswith(p + "-"):
            return p, name[len(p) + 1:]
    return None, name


def os_family(raw):
    """(os_full, family) for a peer's reported OS. Never raises.

    The management API's `os` is free text a client supplies, so it arrives in
    three shapes and only one of them is the obvious one: a real string
    ("Ubuntu 24.04"), absent/empty, and PRESENT-BUT-BLANK. The third is the trap:
    `" "` is truthy, so it survives `p.get("os") or "unknown"`, and
    `" ".split()[0]` raises IndexError — one field on one peer would freeze both
    reading surfaces.

    Blank and absent mean the same thing here — the peer told us nothing — so
    both render as `unknown` rather than as an empty row nobody can act on.
    """
    os_full = (raw or "").strip() or "unknown"
    if os_full == "unknown":
        return "unknown", "unknown"
    # The family is the first token: "Ubuntu 24.04" -> "Ubuntu". Safe now that
    # os_full is known non-blank.
    return os_full, os_full.split()[0]


def fetch_census(base, token, tax, now, goatnet=None, roster=None, declared=None,
                 portal_owners=None):
    """The whole peer population, sliced every way the API supports."""
    roster = roster if roster is not None else {"ok": False, "reason": "no registry supplied"}
    declared = declared or {"ok": False, "rules": []}
    portal_owners = portal_owners or {}
    hdr = {"Authorization": f"Token {token}"}
    peers = _get_json(base.rstrip("/") + "/api/peers", hdr)
    groups = _get_json(base.rstrip("/") + "/api/groups", hdr)
    if peers is None:
        return {"ok": False, "reason": "management API did not answer"}, None

    # ONE resolver call for the whole population, not one per peer. The batch
    # endpoint exists for exactly this — "one graph assembly, every name
    # resolved against it" — and the caller passes a callable rather than a map
    # because the names are only known here, after the census answered.
    if callable(portal_owners):
        portal_owners = portal_owners(
            [p.get("name") or p.get("hostname") or "?" for p in peers])

    # Group names as the management plane declares them — used to refuse a
    # team attribution that would name a group nobody created.
    group_names = ({g.get("name") for g in groups if g.get("name")}
                   if groups is not None else None)

    per_peer = []
    roles, scopes, caps = collections.Counter(), collections.Counter(), collections.Counter()
    role_memberships = collections.Counter()
    oses, os_fam, vers, uivers = (collections.Counter() for _ in range(4))
    handles = collections.Counter()
    owner_kinds = collections.Counter()
    owning_identities = collections.Counter()
    enrol_month, seen_month = collections.Counter(), collections.Counter()
    dormant = {d: 0 for d in DORMANCY_DAYS}
    connected = never_seen = rosenpass = ssh_on = expired = ephemeral = approval = 0
    unresolved_roles = multi_scope = 0

    for p in peers:
        name = p.get("name") or p.get("hostname") or "?"
        gnames = sorted(g.get("name", "") for g in (p.get("groups") or []))
        prole, pscope, pcaps = None, None, []
        held_scopes = set()
        for gn in gnames:
            scope, suffix = split_group(gn, tax)
            if scope:
                held_scopes.add(scope)
            if not suffix:
                continue
            if suffix in CAPABILITY_SUFFIXES:
                pcaps.append(suffix)
                caps[suffix] += 1
                continue
            role_memberships[suffix] += 1
            if suffix in ROLE_PRIORITY:
                if prole is None or ROLE_PRIORITY.index(suffix) < ROLE_PRIORITY.index(prole):
                    prole = suffix
                    # The primary scope is the scope of the group that gave the
                    # primary role, not "whichever scope the API listed first".
                    # A machine in two scopes (a forward-site node also holding
                    # its host's POD group; an exercise node also holding host groups)
                    # is common, and picking by API order would make the same
                    # peer land in different scopes on different polls.
                    pscope = scope
        if prole is None:
            unresolved_roles += 1
            pscope = sorted(held_scopes)[0] if held_scopes else None
        if len(held_scopes) > 1:
            multi_scope += 1
        roles[prole or "(no declared role)"] += 1
        scopes[pscope or "(unscoped)"] += 1

        # `.strip` before the `or`, because a peer can report an OS that is
        # PRESENT and BLANK. `os: " "` is truthy, survives the `or`, and splits to
        # an empty list — IndexError, service dead, both reading surfaces frozen
        # at the last good render.
        os_full, os_fam_key = os_family(p.get("os"))
        oses[os_full] += 1
        os_fam[os_fam_key] += 1
        vers[p.get("version") or "(unreported)"] += 1
        uivers[(p.get("ui_version") or "").split("/")[-1] or "(headless)"] += 1

        h = handle_of(name, goatnet)
        if h:
            handles[h] += 1
        owner = owner_of(name, h, roster, goatnet or "", declared,
                         portal_owners.get(name), prole, pscope, group_names)
        owner_kinds[owner["kind"]] += 1
        if owner["handle"]:
            owning_identities[owner["handle"]] += 1

        created = parse_ts(p.get("created_at"))
        last = parse_ts(p.get("last_seen"))
        if created:
            enrol_month[time.strftime("%Y-%m", time.gmtime(created))] += 1
        quiet_days = None
        if p.get("connected"):
            connected += 1
            quiet_days = 0.0
        elif last:
            quiet_days = (now - last) / 86400.0
            seen_month[time.strftime("%Y-%m", time.gmtime(last))] += 1
            for d in DORMANCY_DAYS:
                if quiet_days > d:
                    dormant[d] += 1
        else:
            never_seen += 1
            for d in DORMANCY_DAYS:
                dormant[d] += 1

        flags = p.get("local_flags") or {}
        if flags.get("rosenpass_enabled"):
            rosenpass += 1
        if p.get("ssh_enabled"):
            ssh_on += 1
        if p.get("login_expired"):
            expired += 1
        if p.get("ephemeral"):
            ephemeral += 1
        if p.get("approval_required"):
            approval += 1

        per_peer.append({
            "name": name,
            "ip": p.get("ip"),
            "connected": bool(p.get("connected")),
            "os": os_full,
            "version": p.get("version") or None,
            "ui_version": (p.get("ui_version") or "").split("/")[-1] or None,
            "role": prole,
            "scope": pscope,
            "caps": pcaps,
            "handle": h,
            "owner": owner,
            "quiet_days": quiet_days,
            "created_unix": created,
            "rosenpass": bool(flags.get("rosenpass_enabled")),
        })

    # Peers per person, and the multi-device tail.
    per_person = sorted(handles.values(), reverse=True)
    census = {
        "ok": True,
        "peers_total": len(peers),
        "peers_connected": connected,
        "peers_offline": len(peers) - connected,
        "never_connected": never_seen,
        "dormant": {str(d): dormant[d] for d in DORMANCY_DAYS},
        "groups_declared": len(groups) if groups is not None else None,
        "groups_populated": (sum(1 for g in groups if g.get("peers_count"))
                             if groups is not None else None),
        "by_role": topn(roles, 14),
        "by_role_membership": topn(role_memberships, 14),
        "by_scope": topn(scopes, 10),
        "by_capability": topn(caps, 6),
        "roles_unresolved": unresolved_roles,
        "multi_scope_peers": multi_scope,
        "by_os": topn(oses, 12),
        "by_os_family": topn(os_fam, 8),
        "by_version": topn(vers, 12),
        "by_ui_version": topn(uivers, 6),
        "people_total": len(handles),
        "peers_with_handle": sum(handles.values()),
        "peers_without_handle": len(peers) - sum(handles.values()),
        "peers_per_person_median": (statistics.median(per_person) if per_person else None),
        "peers_per_person_max": (per_person[0] if per_person else None),
        "multi_device_people": sum(1 for v in per_person if v > 1),
        "top_holders": topn(handles, 10),
        # PEER NAME -> OWNER, for the whole population. Every table on these
        # pages is keyed by peer name — the demand rows, the slowest-peer rows
        # from Prometheus, anything added later — and each of them wants the
        # same answer to "who do I ask about this one". Emitting the map once
        # means a table gets the owner by lookup instead of each source having
        # to carry the join itself, and it is the census that has the whole
        # population rather than whoever happened to appear in a window.
        "owners": {p["name"]: p["owner"] for p in per_peer},
        # The ownership reading. `resolved` counts peers that reach a named,
        # declared identity — a person directly, or a machine's custodian. The
        # three unresolved kinds are kept apart because the remedies differ:
        # `unregistered` wants a manifest, `none` wants an enrolment that
        # carries a person at all, and `unknown` wants the registry file put on
        # this host. Collapsing them into one "unowned" number would hide which
        # of the three it is, which is the only actionable part.
        "ownership": {
            "roster_ok": bool(roster.get("ok")),
            "roster_reason": roster.get("reason"),
            "roster_generated_at": roster.get("generated_at"),
            # The age is graded, not left as a date for a reader to do
            # arithmetic on, and the digest is published so `goat access strays`
            # can be run against a checkout to answer the question this host
            # cannot: whether the file is merely old or actually behind.
            "roster_age_days": roster.get("age_days"),
            "roster_freshness": roster.get("freshness"),
            "roster_source_digest": roster.get("source_digest"),
            "roster_members": roster.get("member_count"),
            "declared_rules": len(declared.get("rules") or ()),
            # `resolved` counts peers that reach a NAMED identity by any route.
            # `recorded` counts only the ones the resolver reconciled against a
            # live identity — the difference between the two is how much of this
            # column is a record and how much is a lead, which is the number
            # worth watching.
            "resolved": (owner_kinds["registry"] + owner_kinds["name"]
                         + owner_kinds["custody"] + owner_kinds["team"]
                         + owner_kinds["declared"]),
            "recorded": owner_kinds["registry"],
            "by_kind": {k: owner_kinds[k] for k in OWNER_KINDS},
            # `unmatched` split by what the peer name says, because the remedies
            # differ: `person` wants the roster regenerated after a manifest
            # sync, `machine` wants a custodian found, `none` wants an owner
            # declared at all.
            "unmatched_by_shape": dict(collections.Counter(
                p["owner"].get("shape") or "none" for p in per_peer
                if p["owner"]["kind"] == "unmatched")),
            "owning_identities": len(owning_identities),
            "top_owners": topn(owning_identities, 10),
        },
        "enrolments_by_month": [{"name": k, "value": enrol_month[k]}
                                for k in sorted(enrol_month)],
        "last_seen_by_month": [{"name": k, "value": seen_month[k]}
                               for k in sorted(seen_month)],
        "posture": {
            "rosenpass_enabled": rosenpass,
            "ssh_enabled": ssh_on,
            "login_expired": expired,
            "ephemeral": ephemeral,
            "approval_required": approval,
        },
        "taxonomy_ok": tax["ok"],
    }
    return census, per_peer


# ══════════════════════════════════════════════════════════════════════════
#  Source 2 — the Traefik access log (per-peer x per-service demand)
# ══════════════════════════════════════════════════════════════════════════
#
# THE JOIN THAT MAKES THESE PAGES POSSIBLE. Traefik's Prometheus metrics carry
# a `service` label and no client identity whatsoever, so from metrics alone
# "which peer is leaning on panoscope" is unanswerable. The JSON access log
# carries `ClientHost`, and on the mesh entrypoint that is the peer's own
# 100.64/10 address, which is the `ip` field on the management API's peer
# record. One join, and per-peer service demand exists.
#
# WHAT IT CANNOT SEE, said here so nobody has to rediscover it: traffic that
# arrives through the relay ingress tunnel (ADR 1085, haproxy passthrough)
# reaches Traefik from the docker bridge gateway, so every such request shares
# ONE source address and is unattributable at this vantage. That is most of
# the signal/enrollment plane: nearly all logged requests in a window can come
# from the gateway address. So the attributable slice is the BROWSING plane,
# which is exactly the population these pages are about — but the fraction is
# reported on the page rather than quietly divided away.

LOG_SVC_STRIP = re.compile(r"^(?:nova-)?(.*?)@file$")


def norm_service(s):
    """`nova-portal@file` -> `portal`.

    COUPLED to the ^nova- metric_relabel_configs block in
    ops/compose/goat-obs/prometheus/prometheus.yml, which strips the same
    prefix from the `service` label at ingest. The names on these pages have
    to match the names on netview's pages and in Grafana, or the same service
    appears twice under two spellings. If that relabel is renamed, rename
    this with it.
    """
    if not s:
        return None
    m = LOG_SVC_STRIP.match(s)
    return (m.group(1) if m else s.replace("@file", "")) or None


def read_traefik_log(container, window_s, max_bytes):
    """docker logs -> list of parsed access-log records, newest window only.

    Bounded on purpose. ZAGROS logs ~13 MB per five minutes; an unbounded read
    on a busy goatnet would make the poller itself the load. If the cap trips
    the result says so and the page reports a truncated window rather than
    presenting a partial count as a total.
    """
    cmd = ["docker", "logs", "--since", f"{int(window_s)}s", container]
    try:
        proc = subprocess.run(cmd, capture_output=True, timeout=60)
    except Exception as e:
        return None, f"could not read the {container} log: {e}", False
    if proc.returncode != 0:
        err = (proc.stderr or b"").decode("utf-8", "replace").strip()[:160]
        return None, f"docker logs exited {proc.returncode}: {err}", False
    raw = proc.stdout
    truncated = len(raw) > max_bytes
    if truncated:
        # Keep the TAIL: the most recent requests are the ones the rest of the
        # document is contemporaneous with.
        raw = raw[-max_bytes:]
        nl = raw.find(b"\n")
        raw = raw[nl + 1:] if nl >= 0 else raw
    rows = []
    for line in raw.split(b"\n"):
        if not line.startswith(b"{"):
            continue
        try:
            d = json.loads(line)
        except Exception:
            continue
        if "ClientHost" not in d:
            continue
        rows.append(d)
    return rows, None, truncated



# Retention for the contention accumulator. Six hours is long enough to hold
# both a busy and a quiet period on any goatnet and short enough that a
# yesterday-shaped pattern cannot masquerade as today's.
MINUTE_HISTORY_S = 6 * 3600
# Cap per minute so one hot minute cannot dominate the file or the percentile.
MINUTE_SAMPLE_CAP = 240


def _merge_minute_history(path, browse, core_ms):
    """Union this window's per-minute buckets with the retained ones.

    Returns (browse, core_ms) covering up to MINUTE_HISTORY_S. Failure to read
    or write NEVER costs the caller its own window — a missing accumulator
    degrades the test to single-window behaviour rather than breaking the poll.
    """
    if not path:
        return browse, core_ms
    hist = {}
    try:
        with open(path) as fh:
            hist = json.load(fh)
    except FileNotFoundError:
        pass  # no history file yet: start empty
    except Exception as e:                                   # noqa: BLE001
        print(f"peerview-poll: minute history unreadable ({e}) — starting fresh",
              file=sys.stderr)

    for m, v in list(hist.items()):
        b, c = v.get("b", 0), v.get("c", [])
        if m not in browse and m not in core_ms:
            browse[m] = b
            core_ms[m] = list(c)

    now_min = int(time.time() // 60) * 60
    out = {}
    for m in set(list(browse) + list(core_ms)):
        try:
            minute_ts = int(m)             # retain numeric epoch-second history
        except (TypeError, ValueError):
            # build_demand keys buckets with StartUTC[:16], without the Z.
            # Restore explicit UTC so retention never depends on host timezone.
            minute_ts = (parse_ts(m + ":00Z") if isinstance(m, str) and
                         re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}", m) else None)
        if minute_ts is None:
            print("peerview-poll: discarding invalid minute history key", file=sys.stderr)
        if minute_ts is None or now_min - minute_ts > MINUTE_HISTORY_S:
            browse.pop(m, None)
            core_ms.pop(m, None)
            continue
        out[str(m)] = {"b": browse.get(m, 0),
                       "c": list(core_ms.get(m, ()))[:MINUTE_SAMPLE_CAP]}
    try:
        tmp = path + ".tmp"
        with open(tmp, "w") as fh:
            json.dump(out, fh)
        os.replace(tmp, path)             # rename, never a half-written file
    except Exception as e:                                   # noqa: BLE001
        print(f"peerview-poll: could not persist minute history ({e})",
              file=sys.stderr)
    return browse, core_ms


def build_demand(rows, per_peer, classes, window_s, truncated, minutes_path=None):
    """Per-peer and per-service demand from one window of the access log."""
    by_ip = {p["ip"]: p for p in (per_peer or []) if p.get("ip")}

    peer_req = collections.Counter()
    peer_bytes = collections.Counter()
    peer_time = collections.defaultdict(list)
    peer_services = collections.defaultdict(collections.Counter)
    peer_class = collections.defaultdict(collections.Counter)
    svc_req = collections.Counter()
    svc_bytes = collections.Counter()
    svc_time = collections.defaultdict(list)
    svc_clients = collections.defaultdict(set)
    svc_err = collections.Counter()
    class_req = collections.Counter()
    class_bytes = collections.Counter()
    class_time = collections.defaultdict(list)
    unclassified = collections.Counter()
    # Per-minute series, used for the only within-window contention test that
    # is defensible: compare core-service latency in the busiest browse
    # minutes against the quietest.
    minute_browse = collections.Counter()
    minute_core_ms = collections.defaultdict(list)

    attributable = anonymous = probes = 0
    for d in rows:
        ip = d.get("ClientHost")
        svc = norm_service(d.get("ServiceName"))
        dur_ms = (d.get("Duration") or 0) / 1e6          # ns -> ms
        down = d.get("DownstreamContentSize") or 0
        up = d.get("RequestContentSize") or 0
        nbytes = down + up
        status = d.get("DownstreamStatus") or 0
        minute = (d.get("StartUTC") or "")[:16]

        # Synthetic monitoring is counted and then set aside. It never reaches
        # a demand figure, a peer row, a service row or the contention test.
        if is_probe(d.get("RequestPath"), classes):
            probes += 1
            continue

        # A record with no ServiceName is a connection Traefik handled without
        # routing to a backend (a TLS-level or entrypoint-level event). It is
        # real traffic and it is not a service, so it gets its own bucket
        # rather than being pooled with services whose class is simply not
        # declared yet — those two need different reactions.
        cls = "no-backend" if not svc else classify(svc, classes)
        if cls == "unclassified":
            unclassified[svc] += 1
        # Counts and bytes always. Durations only from services whose duration
        # IS a response time — see is_stream().
        timed = not is_stream(svc, classes)
        class_req[cls] += 1
        class_bytes[cls] += nbytes
        if timed:
            class_time[cls].append(dur_ms)
        if cls == "browse":
            minute_browse[minute] += 1
        elif cls == "core" and timed:
            minute_core_ms[minute].append(dur_ms)

        if svc:
            svc_req[svc] += 1
            svc_bytes[svc] += nbytes
            if timed:
                svc_time[svc].append(dur_ms)
            if status >= 500:
                svc_err[svc] += 1

        peer = by_ip.get(ip)
        if peer:
            attributable += 1
            key = peer["name"]
            peer_req[key] += 1
            peer_bytes[key] += nbytes
            if timed:
                peer_time[key].append(dur_ms)
            peer_class[key][cls] += 1
            if svc:
                peer_services[key][svc] += 1
                svc_clients[svc].add(key)
        else:
            anonymous += 1

    total = attributable + anonymous
    peers = []
    for name, n in peer_req.most_common(200):
        p = next((x for x in per_peer if x["name"] == name), {})
        peers.append({
            "name": name,
            "requests": n,
            "rps": n / window_s,
            "bytes": peer_bytes[name],
            "bps": peer_bytes[name] * 8 / window_s,
            "p95_ms": pctile(peer_time[name], 0.95),
            "services": len(peer_services[name]),
            # This PEER's busiest service, not the goatnet's. Getting that
            # backwards would have labelled every row with whatever was
            # globally loudest and told an operator nothing about the row.
            "top_service": (peer_services[name].most_common(1)[0][0]
                            if peer_services[name] else None),
            "top_class": (peer_class[name].most_common(1)[0][0]
                          if peer_class[name] else None),
            "role": p.get("role"),
            "scope": p.get("scope"),
            "os": p.get("os"),
            "version": p.get("version"),
            "handle": p.get("handle"),
            # Carried onto the demand row so the heaviest peer in the window is
            # one click from the person answerable for it, rather than a name to
            # go and look up somewhere else.
            "owner": p.get("owner"),
        })

    services = []
    for svc, n in svc_req.most_common(30):
        services.append({
            "name": svc,
            "class": classify(svc, classes),
            "requests": n,
            "rps": n / window_s,
            "bytes": svc_bytes[svc],
            "bps": svc_bytes[svc] * 8 / window_s,
            "p95_ms": pctile(svc_time[svc], 0.95),
            "mean_ms": (statistics.fmean(svc_time[svc]) if svc_time[svc] else None),
            "clients": len(svc_clients[svc]),
            "err_5xx": svc_err[svc],
        })

    # ── the contention test ────────────────────────────────────────────────
    # Split the window's minutes into the busiest browse third and the
    # quietest third, and compare core-service p95 across the two. This is a
    # WITHIN-WINDOW ASSOCIATION and nothing more: it does not randomise, it
    # does not control for anything, and a difference does not prove browsing
    # slowed the core. It is reported because the honest alternative — two
    # numbers side by side and a shrug — is what leaves the question open
    # forever. Needs enough minutes to have thirds at all.
    # Both sides need enough timed core requests to have a percentile at all.
    # Without this a window comparing hundreds of requests against a handful
    # prints the result as if it meant something.
    MIN_SIDE = 30
    # A minute must carry enough CORE traffic of its own to be worth ranking.
    #
    # Without this the test selected for the wrong thing entirely. Ranking every
    # minute by browse volume and taking the bottom third picks the minutes when
    # the WHOLE SYSTEM is idle — which have almost no core requests either — so
    # the comparison became "813 core requests against 4" and refused itself
    # forever. Measured; the page had been blank since it
    # shipped and the stated remedy, a longer window, would not have fixed it.
    # The asymmetry is structural, not a sample size.
    #
    # Requiring a floor per minute makes the two sides comparable BY
    # CONSTRUCTION: every minute on both sides has a core sample, so the only
    # thing that differs between the thirds is how much browsing was happening,
    # which is the variable under test.
    MIN_CORE_PER_MINUTE = 3

    # ACCUMULATE MINUTES ACROSS POLLS instead of widening the log read.
    #
    # The obvious fix for "not enough minutes" is a longer --window-s. It does
    # not work here and the numbers say why: the Traefik access log runs 37 MB
    # per 5 minutes against a 48 MB read cap, so the CURRENT window is already
    # close to truncating and 30 minutes would be 217 MB re-parsed on every
    # poll. Widening the window buys minutes by making every poll heavier and
    # eventually silently lossy.
    #
    # Each poll already derives exactly what the test needs — per-minute browse
    # counts and per-minute core latencies. Keeping them lets the comparison
    # span hours while every poll still reads only its own five minutes. This is
    # the same reasoning as the ledger: history belongs in a file, not in a
    # bigger read.
    minute_browse, minute_core_ms = _merge_minute_history(
        minutes_path, minute_browse, minute_core_ms)

    contention = {"testable": False, "reason": None}
    all_minutes = sorted(set(list(minute_browse) + list(minute_core_ms)))
    minutes = [m for m in all_minutes
               if len(minute_core_ms.get(m, ())) >= MIN_CORE_PER_MINUTE]
    dropped = len(all_minutes) - len(minutes)
    ranked = sorted(minutes, key=lambda m: minute_browse.get(m, 0))
    k = max(1, len(ranked) // 3)
    quiet, busy = ranked[:k], ranked[-k:]
    q = [v for m in quiet for v in minute_core_ms[m]]
    b = [v for m in busy for v in minute_core_ms[m]]
    if len(minutes) < 6:
        contention["reason"] = (
            f"only {len(minutes)} minute(s) in this window carry at least "
            f"{MIN_CORE_PER_MINUTE} timed core requests ({dropped} minute(s) "
            f"were too quiet to rank) — too few to split into busy and quiet "
            f"thirds. A longer --window-s is the fix; the window is the limit "
            f"here, not the traffic")
    elif min(len(q), len(b)) < MIN_SIDE:
        contention["reason"] = (
            f"the two thirds hold {len(q)} and {len(b)} timed core requests; "
            f"at least {MIN_SIDE} on each side are needed before a percentile "
            f"is worth comparing")
    else:
        qb = sum(minute_browse.get(m, 0) for m in quiet) / len(quiet)
        bb = sum(minute_browse.get(m, 0) for m in busy) / len(busy)
        contention = {
            "testable": True,
            "reason": None,
            "minutes": len(minutes),
            "quiet_browse_rpm": qb,
            "busy_browse_rpm": bb,
            "quiet_core_p95_ms": pctile(q, 0.95),
            "busy_core_p95_ms": pctile(b, 0.95),
            "quiet_core_n": len(q),
            "busy_core_n": len(b),
            "minutes_dropped_too_quiet": dropped,
            "min_core_per_minute": MIN_CORE_PER_MINUTE,
            # A separation this small is inside the noise of a five-minute
            # sample; the page uses it to refuse a verdict rather than to
            # announce one.
            "browse_separation": (bb - qb),
        }

    return {
        "window_s": window_s,
        "truncated": truncated,
        "requests_total": total,
        "requests_probe": probes,
        "requests_logged": total + probes,
        "requests_attributable": attributable,
        "requests_anonymous": anonymous,
        "attributable_share": (attributable / total) if total else None,
        "rps_total": (total / window_s) if window_s else None,
        "peers_seen": len(peer_req),
        "peers": peers,
        "services": services,
        "by_class": [
            {"name": c,
             "requests": class_req[c],
             "rps": class_req[c] / window_s,
             "bytes": class_bytes[c],
             "bps": class_bytes[c] * 8 / window_s,
             "p95_ms": pctile(class_time[c], 0.95)}
            for c in sorted(class_req, key=lambda c: -class_req[c])
        ],
        "unclassified_services": topn(unclassified, 10),
        "contention": contention,
    }


def classify(service, classes):
    """Service -> declared class. Unmapped is `unclassified`, never guessed.

    The map is a shipped config file, not a heuristic here, because "is
    panoscope a browsing surface or a core service" is a policy question with
    an owner and it should be answerable by reading one file.
    """
    if not service:
        return "unclassified"
    for cls, pats in classes.get("classes", {}).items():
        for pat in pats:
            if re.fullmatch(pat, service):
                return cls
    return "unclassified"


def is_probe(path, classes):
    """A synthetic health check, not usage.

    Checked on the PATH and before every class, because a probe and a page
    load hit the same service and only the path separates them. On ZAGROS this
    removes 4 requests/minute/surface of blackbox `/healthz` traffic that
    would otherwise make every unused surface read as evenly popular.
    """
    if not path:
        return False
    p = path.split("?", 1)[0]
    for pat in classes.get("probe_paths", []):
        if re.fullmatch(pat, p):
            return True
    return False


def load_classes(path):
    try:
        import yaml
        with open(path) as fh:
            doc = yaml.safe_load(fh) or {}
    except Exception as e:
        print(f"peerview-poll: service classes unreadable ({e}) — "
              f"every service will read as unclassified and NO probe traffic "
              f"will be filtered", file=sys.stderr)
        return {"classes": {}, "probe_paths": []}, {}
    out, why = {}, {}
    for cls, cfg in (doc.get("classes") or {}).items():
        out[cls] = list(cfg.get("match") or [])
        why[cls] = cfg.get("why", "")
    return {"classes": out,
            "probe_paths": list(doc.get("probe_paths") or []),
            "stream_services": set(doc.get("stream_services") or [])}, why


def is_stream(service, classes):
    """A service whose Traefik duration is a connection lifetime.

    Its requests and bytes are real and are counted. Its DURATION is not a
    response time and is excluded from every percentile, mean and comparison.
    Measured on zagros-prod before this existed: a core-service p95 of 675,687 ms
    — eleven minutes — which was one held-open signal stream and not something
    anybody waited for.
    """
    return service in classes.get("stream_services", ())


# ══════════════════════════════════════════════════════════════════════════
#  Source 3 — Prometheus (transport class, per-peer bytes, service shape)
# ══════════════════════════════════════════════════════════════════════════

def q(prom, expr, timeout=10):
    url = f"{prom}/api/v1/query?" + urllib.parse.urlencode({"query": expr})
    try:
        with urllib.request.urlopen(url, timeout=timeout) as r:
            body = json.load(r)
    except Exception as e:
        print(f"peerview-poll: prom query failed: {e}", file=sys.stderr)
        return None
    if body.get("status") != "success":
        return None
    out = []
    for s in body["data"]["result"]:
        try:
            v = float(s["value"][1])
        except (ValueError, KeyError):
            continue
        if math.isnan(v):  # drop NaN (empty metric)
            continue
        out.append(({k: v2 for k, v2 in s["metric"].items() if k != "__name__"}, v))
    return out


def scalar(prom, expr):
    r = q(prom, expr)
    return None if (r is None or not r) else r[0][1]


def series(prom, expr, key):
    r = q(prom, expr)
    if r is None:
        return None
    return sorted(({"name": m.get(key, "?"), "value": v, **m} for m, v in r),
                  key=lambda d: -d["value"])


def build_transport(prom):
    """Direct vs relayed, per peer, and what each path costs.

    Every reading here comes from ONE vantage — the host running this poller.
    netbird reports a peer's connection type as seen from here, so "29 relayed"
    means 29 relayed FROM THIS HOST, not 29 relayed in the fabric. Two hosts
    would legitimately disagree. Said on the page, not buried here.
    """
    d = {}
    d["peers_measured"] = scalar(prom, "count(goat_net_peer_latency_measured)")
    d["peers_p2p"] = scalar(prom, 'count(goat_net_peer_latency_measured{class="mesh-p2p"})')
    d["peers_relayed"] = scalar(prom, 'count(goat_net_peer_latency_measured{class="mesh-relayed"})')
    d["coverage"] = scalar(
        prom, "sum(goat_net_peer_latency_measured) / "
              "clamp_min(count(goat_net_peer_latency_measured), 1)")
    M = "and on (peer, class) (goat_net_peer_latency_measured == 1)"
    d["rtt_p2p_ms"] = scalar(prom, f'avg(goat_net_peer_latency_ms{{class="mesh-p2p"}} {M})')
    d["rtt_relayed_ms"] = scalar(prom, f'avg(goat_net_peer_latency_ms{{class="mesh-relayed"}} {M})')
    d["bytes_by_class"] = series(
        prom, f'sum by (class) (rate(goat_net_peer_bytes_total[{RI}])) * 8', "class")
    d["top_peers_bps"] = series(
        prom, f'topk(15, sum by (peer, class) (rate(goat_net_peer_bytes_total[{RI}])) * 8)',
        "peer")
    d["slowest_peers"] = series(prom, f"topk(10, goat_net_peer_latency_ms {M})", "peer")
    # One goatnet's path-quality prober publishes these and another's does not; the
    # reverse is true of everything above. Asked for regardless, reported
    # absent where absent.
    d["jitter_ms"] = scalar(prom, "avg(goat_mesh_rtt_jitter_ms)")
    d["loss_ratio"] = scalar(prom, "avg(goat_mesh_packet_loss_ratio)")
    d["mesh_rtt_ms"] = scalar(prom, "avg(goat_mesh_rtt_ms)")

    # TWO DIFFERENT FACTS, and collapsing them was a real defect. `reachable`
    # means Prometheus answered the query; `has_data` means it answered with a
    # series. Prometheus can be up and healthy and carry NONE of the goat_net_*
    # attribution metrics this section needs, which would report the source
    # green while every panel under it is null.
    #
    # The list-valued queries return [] for "asked, nothing matched", which is
    # not None — so `has_data` is computed over the SCALARS only. The section
    # header on the page uses has_data to choose between rendering the cards
    # and rendering the sentence that explains what is missing and why.
    scalars = ("peers_measured", "peers_p2p", "peers_relayed", "coverage",
               "rtt_p2p_ms", "rtt_relayed_ms", "jitter_ms", "loss_ratio",
               "mesh_rtt_ms")
    d["has_data"] = any(d.get(k) is not None for k in scalars)
    d["reachable"] = any(d.get(k) is not None
                         for k in list(scalars) + ["bytes_by_class",
                                                   "top_peers_bps",
                                                   "slowest_peers"])
    return d


# ══════════════════════════════════════════════════════════════════════════
#  Source 4 — the ledger (the only source of day-over-day shape)
# ══════════════════════════════════════════════════════════════════════════
#
# Prometheus retains 15 days. A month-to-month trajectory drawn from it would
# be an extrapolation wearing a chart's clothes, so it is not drawn from it.
# This appends one compact row per poll and folds old rows down so the file
# stays bounded: full resolution for 8 days, hourly to 40, daily beyond.
#
# Population growth is the exception and does NOT need the ledger: every peer
# carries `created_at`, so enrolments-by-month is available in full on the
# first run. The ledger covers what has no such record — how many were
# CONNECTED, and how much they were asking for, at a given hour.

LEDGER_RAW_DAYS = 8
LEDGER_HOURLY_DAYS = 40
LEDGER_MAX_ROWS = 4000


def ledger_row(now, census, demand):
    return {
        "t": int(now),
        "n": census.get("peers_total") if census.get("ok") else None,
        "c": census.get("peers_connected") if census.get("ok") else None,
        "rq": (demand or {}).get("rps_total"),
        "at": (demand or {}).get("requests_attributable"),
        "ps": (demand or {}).get("peers_seen"),
        "res": "raw",
    }


# Resolution rank. A fold must never make a row FINER than it already is, and
# must never re-tag a coarser row as its own resolution. Without the rank the
# day rows written by the first fold were swept into the hour fold on the next
# pass and re-labelled `hour` — harmless to the averages, and it destroyed the
# ability to tell how much real history the ledger holds. Caught by the test.
_RES_RANK = {"raw": 0, "hour": 1, "day": 2}


def _fold(rows, older_than, bucket, tag):
    """Collapse rows older than `older_than` into one row per `bucket`.

    Rows already at this resolution or coarser are passed through untouched.
    """
    want = _RES_RANK[tag]
    keep, fold = [], collections.defaultdict(list)
    for r in rows:
        if r["t"] >= older_than or _RES_RANK.get(r.get("res", "raw"), 0) >= want:
            keep.append(r)
        else:
            fold[r["t"] // bucket].append(r)
    for b, group in fold.items():
        def avg(k):
            vs = [g[k] for g in group if g.get(k) is not None]
            return (sum(vs) / len(vs)) if vs else None
        keep.append({"t": b * bucket, "n": avg("n"), "c": avg("c"),
                     "rq": avg("rq"), "at": avg("at"), "ps": avg("ps"),
                     "res": tag})
    keep.sort(key=lambda r: r["t"])
    return keep


def update_ledger(path, row, now):
    rows = []
    try:
        with open(path) as f:
            for line in f:
                line = line.strip()
                if line:
                    try:
                        rows.append(json.loads(line))
                    except Exception:
                        continue
    except FileNotFoundError:
        pass  # file absent: nothing to load
    except Exception as e:
        print(f"peerview-poll: ledger unreadable ({e}) — starting a new one",
              file=sys.stderr)
        rows = []
    rows.append(row)
    rows = _fold(rows, now - LEDGER_HOURLY_DAYS * 86400, 86400, "day")
    rows = _fold(rows, now - LEDGER_RAW_DAYS * 86400, 3600, "hour")
    if len(rows) > LEDGER_MAX_ROWS:
        # Over the cap even after folding — an unusually fast poll interval.
        # Fold the whole middle band to daily before dropping anything, because
        # the alternative (trimming the head) throws away exactly the old
        # history the month-over-month panel exists to show. Only if that still
        # does not fit does the oldest go, and by then each dropped row is a
        # whole day.
        rows = _fold(rows, now - LEDGER_RAW_DAYS * 86400, 86400, "day")
        if len(rows) > LEDGER_MAX_ROWS:
            rows = rows[-LEDGER_MAX_ROWS:]
    d = os.path.dirname(path) or "."
    os.makedirs(d, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=d, prefix=".pvledger.")
    with os.fdopen(fd, "w") as f:
        for r in rows:
            f.write(json.dumps(r, separators=(",", ":")) + "\n")
    os.chmod(tmp, 0o600)
    os.replace(tmp, path)
    return rows


def build_cycle(rows, now):
    """Hour-of-day shape and month-over-month trajectory, from the ledger.

    Both carry their own sample size. A 24-hour profile built from six hours
    of history is not a daily cycle, and the page needs to be able to say so
    instead of drawing a confident line through one morning.
    """
    if not rows:
        return {"days": 0, "hours": 0, "hour_of_day": [], "by_month": [],
                "span_start": None}
    span = now - min(r["t"] for r in rows)
    hod_c = collections.defaultdict(list)
    hod_r = collections.defaultdict(list)
    mon_c, mon_r = collections.defaultdict(list), collections.defaultdict(list)
    for r in rows:
        h = int(time.strftime("%H", time.gmtime(r["t"])))
        m = time.strftime("%Y-%m", time.gmtime(r["t"]))
        if r.get("c") is not None:
            hod_c[h].append(r["c"])
            mon_c[m].append(r["c"])
        if r.get("rq") is not None:
            hod_r[h].append(r["rq"])
            mon_r[m].append(r["rq"])
    return {
        "days": span / 86400.0,
        "hours": span / 3600.0,
        "rows": len(rows),
        "span_start": int(min(r["t"] for r in rows)),
        "hour_of_day": [
            {"name": f"{h:02d}",
             "connected": (statistics.fmean(hod_c[h]) if hod_c.get(h) else None),
             "rps": (statistics.fmean(hod_r[h]) if hod_r.get(h) else None),
             "samples": len(hod_c.get(h, []))}
            for h in range(24)
        ],
        "by_month": [
            {"name": m,
             "connected": (statistics.fmean(mon_c[m]) if mon_c.get(m) else None),
             "rps": (statistics.fmean(mon_r[m]) if mon_r.get(m) else None),
             "samples": len(mon_c.get(m, []))}
            for m in sorted(set(list(mon_c) + list(mon_r)))
        ],
    }


# ══════════════════════════════════════════════════════════════════════════
#  Derived — what a peer costs before it does anything
# ══════════════════════════════════════════════════════════════════════════

def build_cost(census, demand, transport, classes_why):
    """Baseline (being enrolled) vs active (being used).

    THE DISTINCTION, because it is the one an operator plans capacity with:
    an enrolled peer costs something even when its owner is asleep — it holds
    a signal stream, it re-registers, it answers management. That is BASELINE
    and it scales with the number of enrolled-and-connected peers. What its
    owner does when awake is ACTIVE and scales with usage, not population.
    They are separated here by ENTRYPOINT CLASS, not by guesswork: the
    `enrollment` plane (signal, management) is baseline; the browsing plane is
    active.
    """
    by_class = {c["name"]: c for c in (demand or {}).get("by_class", [])}
    base = by_class.get("core")
    active = by_class.get("browse")
    connected = census.get("peers_connected") if census.get("ok") else None

    out = {
        "connected": connected,
        "baseline_rps": base["rps"] if base else None,
        "baseline_bps": base["bps"] if base else None,
        "active_rps": active["rps"] if active else None,
        "active_bps": active["bps"] if active else None,
    }
    if base and connected:
        out["baseline_rps_per_peer"] = base["rps"] / connected
        out["baseline_bps_per_peer"] = base["bps"] / connected
    else:
        out["baseline_rps_per_peer"] = None
        out["baseline_bps_per_peer"] = None
    # Active demand divided across the peers that were actually SEEN making
    # requests, not across the population — dividing by the population would
    # report a busy operator's session as a fleet-wide average.
    seen = (demand or {}).get("peers_seen") or 0
    out["active_bps_per_active_peer"] = (active["bps"] / seen) if (active and seen) else None
    out["active_peers"] = seen or None
    tot = (out["baseline_bps"] or 0) + (out["active_bps"] or 0)
    out["baseline_share"] = ((out["baseline_bps"] / tot)
                             if (tot and out["baseline_bps"] is not None) else None)
    # A projection, labelled as one on the page. Linear in connected peers,
    # which is exactly the assumption the baseline definition licenses and no
    # more: it says nothing about relay fan-out or signal federation, both of
    # which are super-linear.
    out["projection_basis"] = "linear in connected peers; ignores signal federation fan-out"
    out["classes_why"] = classes_why
    return out


# ══════════════════════════════════════════════════════════════════════════
#  Build
# ══════════════════════════════════════════════════════════════════════════

def build(a):
    now = time.time()
    tax = load_taxonomy(a.taxonomy)
    classes, classes_why = load_classes(a.service_classes)

    doc = {
        "generated_unix": int(now),
        "generated_iso": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(now)),
        "goatnet": a.goatnet,
        "vantage": a.vantage,
        "sources": {},
    }

    # ── census ─────────────────────────────────────────────────────────────
    roster = load_roster(a.roster)
    declared = load_declared_owners(a.declared_owners)

    # The shared peer -> person resolver. Held in a box so the sources table can
    # report what it actually answered; a surface that silently degrades to its
    # local fallback is a surface that disagrees with every other one about who
    # owns a machine and never says so.
    resolver = {"asked": 0, "rows": {}, "reason": None}

    def resolve_owners(names):
        if peerowner is None:
            resolver["reason"] = "shared client ops/identity/peerowner.py is not on this host"
            return {}
        base = peerowner.portal_base(a.portal_url, a.portal_container,
                                     a.portal_network, a.portal_port)
        rows = peerowner.resolve(base, a.portal_key, names)
        unread = [r for r in rows.values() if r["attribution"] == peerowner.UNREAD]
        resolver["asked"] = len(rows)
        resolver["rows"] = rows
        if unread:
            resolver["reason"] = unread[0].get("reason") or "resolver did not answer"
        return rows
    census, per_peer = {"ok": False, "reason": "no management URL configured"}, None
    if a.netbird_url and a.netbird_token:
        census, per_peer = fetch_census(a.netbird_url, a.netbird_token, tax, now,
                                       a.goatnet, roster, declared, resolve_owners)
    doc["census"] = census
    doc["sources"]["portal-resolver"] = {
        "ok": bool(resolver["asked"]) and not resolver["reason"],
        "detail": (resolver["reason"] or
                   f"{resolver['asked']} peer name(s) resolved against "
                   f"POST /api/access/peers"),
        "dark": ("the RECORDED owner of every peer — the live Ory identity behind "
                 "a handle, and the console link to it. The registry file below "
                 "still answers who a handle belongs to, but nothing here can "
                 "confirm a live person"),
    }
    doc["sources"]["roster"] = {
        "ok": bool(roster.get("ok")),
        "detail": (f"{roster['member_count']} declared identities, generated "
                   f"{roster.get('generated_at') or 'at an unrecorded time'}"
                   if roster.get("ok") else (roster.get("reason") or "did not load")),
        "dark": ("every peer's owning identity — which person answers for a peer, "
                 "and which custodian answers for a machine"),
    }
    doc["sources"]["netbird"] = {
        "ok": bool(census.get("ok")),
        "detail": (a.netbird_url or "not configured"),
        "dark": ("the entire population census — totals, roles, versions, "
                 "operating systems, enrolment history"),
    }

    # ── demand ─────────────────────────────────────────────────────────────
    rows, err, truncated = read_traefik_log(
        a.traefik_container, a.window_s, a.max_log_bytes)
    demand = None
    if rows is not None:
        demand = build_demand(rows, per_peer, classes, a.window_s, truncated,
                              minutes_path=a.minute_history)
    doc["demand"] = demand
    doc["sources"]["traefik-log"] = {
        "ok": rows is not None,
        "detail": (err or f"{len(rows)} records over {a.window_s}s"
                   f"{' (window truncated at the byte cap)' if truncated else ''}"),
        "dark": ("per-peer service demand, heavy-user outliers, the "
                 "browse-vs-core comparison, and the baseline/active split"),
    }

    # ── transport ──────────────────────────────────────────────────────────
    transport = build_transport(a.prom)
    doc["transport"] = transport
    doc["sources"]["prometheus"] = {
        "ok": bool(transport.get("reachable")),
        "detail": (a.prom + ("" if transport.get("has_data")
                             else " — answered, but carries none of the "
                                  "goat_net_* / goat_mesh_* series this needs")),
        "dark": "direct-vs-relayed path mix, per-peer wire bytes, path quality",
    }

    # ── ledger + cycle ─────────────────────────────────────────────────────
    led = update_ledger(a.ledger, ledger_row(now, census, demand), now)
    doc["cycle"] = build_cycle(led, now)
    doc["sources"]["ledger"] = {
        "ok": bool(led),
        "detail": f"{len(led)} rows, {doc['cycle']['days']:.2f} days",
        "dark": "hour-of-day shape and month-over-month trajectory",
    }

    # ── sibling surfaces, detected rather than assumed ─────────────────────
    # netview ships to some goatnets and not others. Hard-coding a link to
    # netview.html would put a dead link on every goatnet that does not have
    # it, and a dead link on a status surface teaches people that the links do
    # not work. Presence is checked on disk, in the same directory these pages
    # are served from, so the link appears exactly where the target exists.
    outdir = os.path.dirname(a.out) or "."
    doc["siblings"] = {
        name: os.path.exists(os.path.join(outdir, probe))
        for name, probe in (("netview", "netview.json"),
                            ("response", "netview.json"))
    }

    # ── outbound deep links ────────────────────────────────────────────────
    # The identity COP lives on panoscope (`/who`, ADR 1047), which is a
    # different service on a different host, so this surface cannot detect it on
    # disk the way it detects its own siblings above. It is CONFIGURED, and when
    # it is not configured every owner renders as plain text rather than as a
    # link to nowhere — the same rule as the sibling probe: a dead link on a
    # status page teaches people that the links do not work.
    doc["links"] = {
        # Two targets, and which one a row gets depends on what resolved. A
        # person reconciled against a live Ory identity links to THAT PERSON in
        # the id-admin-console — the same deep-link the relay-health card uses,
        # so the two surfaces land an operator in the same place. Everything
        # else links to the identity roster by handle, which is all a handle can
        # honestly address.
        "id_console_base": a.id_console_url or None,
        "identity_base": a.identity_url or None,
        "identity_param": "identity",
    }

    doc["cost"] = build_cost(census, demand, transport, classes_why)
    doc["retention_note"] = (
        "Prometheus on this goatnet retains 15 days, so no panel here draws a "
        "month-over-month line from it. Population growth comes from each "
        "peer's own created_at and is complete from the first run; connected "
        "and demand history accumulate in the poller's own ledger.")
    return doc


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--ledger", required=True)
    ap.add_argument("--goatnet", default="unknown")
    ap.add_argument("--vantage", default=os.uname().nodename)
    ap.add_argument("--prom", default=DEFAULT_PROM)
    ap.add_argument("--netbird-url", default=os.environ.get("PEERVIEW_NETBIRD_URL", ""))
    ap.add_argument("--netbird-token", default=os.environ.get("PEERVIEW_NETBIRD_TOKEN", ""))
    ap.add_argument("--traefik-container", default="traefik")
    ap.add_argument("--window-s", type=int, default=300)
    ap.add_argument("--minute-history",
                    default="/var/lib/peerview/contention-minutes.json",
                    help="accumulator for the contention test; see _merge_minute_history")
    ap.add_argument("--max-log-bytes", type=int, default=48 * 1024 * 1024)
    ap.add_argument("--taxonomy", default="/opt/peerview/netbird_acl_taxonomy.yml")
    ap.add_argument("--service-classes", default="/opt/peerview/peerview-service-classes.yaml")
    ap.add_argument("--roster", default="/opt/peerview/roster.json",
                    help="declared owner registry (docs/operators/roster.json), "
                         "shipped beside the poller; absent = every owner reads unknown")
    # The shared peer -> person resolver (docs/design/peer-identity-resolution.md).
    # URL empty + a container name = resolve the portal's IP on its docker
    # network at run time, which is how the site-host resolver has reached it since
    # design: the portal's mesh hostname does not resolve from the host, and the
    # published :8088 is the oauth2 gate rather than the app.
    ap.add_argument("--portal-url", default=os.environ.get("PEERVIEW_PORTAL_URL", ""))
    ap.add_argument("--portal-container", default=os.environ.get("PEERVIEW_PORTAL_CONTAINER", ""))
    ap.add_argument("--portal-network", default=os.environ.get("PEERVIEW_PORTAL_NET", "mgmt"))
    ap.add_argument("--portal-port", default=os.environ.get("PEERVIEW_PORTAL_PORT", "8090"))
    ap.add_argument("--portal-key", default=os.environ.get("PEERVIEW_PORTAL_KEY", ""),
                    help="operator API key; without it the owner column falls back "
                         "to the local registry file and says so")
    ap.add_argument("--id-console-url", default=os.environ.get("PEERVIEW_ID_CONSOLE_URL", ""),
                    help="id-admin-console base; a resolved person links to "
                         "<base>/users/<ory-id> there")
    ap.add_argument("--declared-owners", default="/opt/peerview/declared-owners.txt",
                    help="`<peer-name-or-glob>: <handle>` lines for substrate peers "
                         "that carry no onboarding name; a FALLBACK, never an override")
    ap.add_argument("--identity-url", default=os.environ.get("PEERVIEW_IDENTITY_URL", ""),
                    help="base URL of the identity COP (panoscope /who); empty "
                         "renders owners as text instead of links")
    a = ap.parse_args()

    doc = build(a)

    d = os.path.dirname(a.out) or "."
    os.makedirs(d, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=d, prefix=".peerview.")
    with os.fdopen(fd, "w") as f:
        json.dump(doc, f, separators=(",", ":"))
    os.chmod(tmp, 0o644)
    os.replace(tmp, a.out)
    dark = [k for k, v in doc["sources"].items() if not v["ok"]]
    print(f"peerview-poll: wrote {a.out} ({os.path.getsize(a.out)}b); "
          f"sources dark: {', '.join(dark) or 'none'}", file=sys.stderr)


if __name__ == "__main__":
    main()
