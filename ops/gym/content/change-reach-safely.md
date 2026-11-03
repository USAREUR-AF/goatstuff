# Changing who can reach what

The model is in [`docs/design/warrant.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/warrant.md) and
the decision is [ADR 1129](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1129-warrant-scoped-expiring-network-authorizations.md).
This is the walk: which file, which command, what CI decides, how it is applied,
and what proves it afterwards.

## 1. Edit the source, not the artifact

| File | Role |
|---|---|
| `ops/warrant/rule-intents/netbird-acl.yaml` | **source.** Rules, per site, naming a `port_set:` rather than ports. |
| `ops/warrant/port-sets.yaml` | **source.** The seven named port sets — the only place a port number is written. |
| `ops/ansible/netbird_acl_matrix.yml` | **generated.** What is reviewed and applied. |

Editing the generated matrix is a round trip: the next `generate.py` reverts
you, and CI rejects the diff in the meantime. The matrix's own header does not
yet say it is generated — that is a real trap and the reason this module exists.

A rule names groups by **suffix** (`MGMT-HOST`, `DEVELOPER`). The cutover
playbook resolves suffix → `<SITE-PREFIX>-<SUFFIX>` → group UUID at apply time.

## 2. Regenerate

```bash
python3 ops/warrant/generate.py            # write the matrix in place
python3 ops/warrant/generate.py --check    # exit 3 if it is not byte-identical
```

Only the `ports:` line is synthesized. Comments, ordering, anchors and the
assertions section pass through verbatim, which is what makes byte-identical a
meaningful gate rather than a tautology — the ports exist in exactly one place,
so reproducing the hand-written file is positive evidence that set resolution
works.

## 3. Open a pull request — that IS the request

There is no approval service, no queue and no side channel. Routing is
`CODEOWNERS`, the decision is a review, attribution is permanent, and rollback
is `git revert`. Three checks meet you:

| Check | What it decides |
|---|---|
| `ops/ci/check-warrant-registry.py` | The registry is valid and the matrix is byte-identical to what it generates. |
| `ops/ci/check-warrant-narrowing.py` | Whether your change **grants nothing new**. |
| `ops/ci/check-netbird-acl-parity.py` | Whether what is declared matches what is deployed. |

The narrowing prover is the part worth understanding. A **narrowing** — every
permit in your ACL already covered by a permit in the base — can merge without
waiting for the owner, because the only thing it can do is take access away, and
that is recoverable by revert. A **widening** waits for the owner named in
CODEOWNERS. The check is deliberately conservative: a genuine narrowing it
cannot follow grades as *not proven* and goes to a human. Being wrong in that
direction costs a review; being wrong in the other auto-merges an over-grant.

Two readings it holds on purpose, because inverting either would treat a wide
rule as narrow: `protocol: all` covers tcp/udp/icmp, and an **empty ports list
means every port**.

## 4. Expiry is a field, not a service

`expires:` on a rule intent is the whole expiry mechanism. CI goes red when a
grant is past due, and `ops/warrant/expire.py --apply` (scheduled) opens the PR
that removes it. It opens a PR rather than pushing: the work of *noticing* is
automated, the decision is not.

## 5. Apply

The cutover playbook synthesizes one NetBird policy per rule. Applying is a
deploy like any other — the preflight wrapper, the ledger, the rollback path.

## 6. Prove it afterwards

Three different questions, three different checks. Do not substitute one for
another:

- **Is what I declared deployed?** `check-netbird-acl-parity.py`. Note that one
  goatnet aliases another's section, so declared and deployed counts differ
  legitimately — check the alias before calling a count mismatch drift.
- **Does the reach I intended actually work?** `goat acl test` — the assertions,
  which are rechecked when nothing is written (ADR 1126), because drift arriving
  from the other side is exactly what a change-triggered check cannot see.
- **What does the mesh look like now?** netvis. Read its five levels for what
  each one proves; a reachability picture is not an assertion.

## The two failure shapes to expect

**A rule that narrows correctly and breaks something anyway.** `protocol: all`
in an old rule is an *information* problem before a discipline one — it is what
a rule says when nobody could prove what narrowing it would break. The listening
surface joined against the declared grant is the evidence that closes that gap;
where nothing has reported, the join reads `unknown` and cannot justify the
narrowing, which is the fail-closed behaviour rather than a missing feature.

**A class that cannot resolve its own names.** A group with no self-source rule
cannot reach its own members — including for DNS. Nothing in the ACL surfaces
this as an error; every name simply stops resolving for those peers.
