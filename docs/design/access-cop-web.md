# Access COP + data catalog — the web surface (`/access`)

| | |
|---|---|
| Status | Design + v1 build (2026-07-09, track `access-cop-web`). Delivers ADR 1026's "See it" web surface (R.3) + the ADR 1075 catalog's browse half, as a third panoscope page. |
| Scope | Read-only browse-as-yourself: the data catalog (orgs → producers → topics → schemas/data types) and the cross-layer access picture (org tree, people, per-identity four-layer permissions, why traces, reverse topic queries) — rendered in the browser, gated per viewer. **No mutations** (owner decision 2026-07-09: admin verbs stay terminal-side; grant/revoke arrive later via ADR 1076 as portal-enforced verbs, not page buttons). |
| Related | [ADR 1026](https://github.com/usareur-af/goatcore/blob/main/docs/adr/1026-data-access-common-operating-picture.md) / [`data-access-cop.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/data-access-cop.md) (the parent — server engine built, web surface = this) · [`data-access-roles.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/data-access-roles.md) §12 (the Access COP charter) · ADR 1073/`rbac-adherence.md` (viewer scoping) · ADR 1028 (topic visibility = advisory) · `panoscope.md` §"Web-native `top` + `who` pages" (the hosting pattern) · [`zenoh-surface-deep-links.md`](https://github.com/usareur-af/goatcore/blob/main/docs/design/zenoh-surface-deep-links.md) (Block 94) |

## 1. What this is

The **third picture** (`data-access-roles.md` §12): panoscope's COP shows *live data*, the id-admin console shows *identities and platform rights*; this page shows **access in relation to publishers, orgs, and the live data** — who can touch what, drawn over what is actually flowing — plus the **catalog** a consumer browses to discover streams and data types. One page, two panes, two gate tiers:

- **Catalog pane** — orgs (display-named, alias-rendered), each org's producers and declared topics (key pattern, format, schema/data type with a rendered proto view via the portal), with a **live-activity overlay** from panoscope's own flow state ("declared and flowing" vs "declared, no traffic"). This is the ADR 1075 catalog's *browse* half; the request→approve loop lands later with the grant rows.
  - **Default view folds "nothing declared" rows (2026-09-06).** One catalog row is one org row in the portal, and on a per-identity slot pool (ADR 1025) that is one row per onboarded person, named by whatever they typed at mint — so most rows are people who declared no topics. The portal's org listing now carries `declared_topics` (live, non-retired rows; one `GROUP BY`, no per-org fan-out) and the page shows only orgs with something to discover, with a one-line toggle to unfold the rest and a title that says how many are folded. A search always looks at every org. `declared_topics` absent (a portal predating the field) means unknown, and unknown rows are **shown** — an old portal must never make the catalog look empty. Rule in `frontend/src/catalog-filter.ts`, unit-tested. What this does NOT do: mark an org as revoked or unfinished. A mesh-first cut (ADR 1031) leaves the portal row in place by design, and the listing carries no revocation state; a properly offboarded org (`DELETE /api/vendors/<anchor>`) is gone from the listing already.
- **Access pane** — the org tree with its people and producers (roster reconcile states), a per-identity four-layer permission picture (`/api/access/who`), the **why** trace (`/api/access/why` — the troubleshooting workhorse), and the reverse topic query (`/api/access/topic` — who can write/read this key).

## 2. Where it lives — resolving ADR 1026's open "build home" question

ADR 1026 proposed a standalone service "to be confirmed against maintenance cost." **Resolved (2026-07-09): the COP web surface is a panoscope page, not a standalone service.** The web-top-who track built exactly the four things a standalone COP would have to duplicate: the oauth2-proxy sidecar + `ViewerScope` per-request gating, the operator-key portal client, the live fabric feed (the §12 activity overlay), and the multi-page frontend with honest degraded states. A fifth hosted service would add a fence, a cert, a deploy ledger row, and a Traefik route for zero capability gain — the wrong side of the complexity budget (ADR 1016). Panoscope's charter grows from "live visualization" to "the fabric's read-side COP family": one surface, three pages (`/` live COP · `/top` load · `/who` roster · `/access` catalog + access picture). The R.3 deep-link contract is honored with panoscope URLs: **`/access` (browse) and `/access/identity?id=<identity_id>` (identity-keyed detail)** — the relay baseball card's R.2 link retargets here when deployed.

## 3. Authorization — two tiers on one page, per ratified policy

The portal stays the data engine (bearer-token, operator-vs-vendor — it has no Ory/group awareness and gains none here); **panoscope is the group-aware policy-enforcement point**, exactly as for `/who`. All gates below are fail-closed in `PANOSCOPE_RBAC_MODE=enforce` and pass-through in `off` (pre-RBAC behavior, per the flag family's contract).

| Pane / endpoint | Gate (enforce mode) | Why |
|---|---|---|
| Catalog: org list, per-org topic metadata, schema/data-type views | **any authenticated viewer** (past the sidecar) | Ratified: topic metadata is visible to all authenticated (ADR 1028 "visibility = advisory, not access"; ADR 1075 catalog item — discovery is the point). Org display names + key patterns + formats enable "request access"; they reveal no payloads and no per-identity information. |
| Catalog: **live-rate overlay** on catalog rows | per-viewer `ViewerScope::allows_key` | Live rates are fabric data, not catalog metadata — a floor viewer sees the catalog row with an honest `—` where an in-scope viewer sees msg/s. Same rule as every other live read. |
| Access pane: roster/org tree, `who`, `why`, `topic` reverse query | **`who_groups`** (`PANOSCOPE_RBAC_WHO_GROUPS`, default `ADMINS,IAM-ADMIN`) | Cross-identity reads are the COP's concentrated privilege (`data-access-cop.md` §"Security of the access picture itself" — operator-tier at the portal). Same gate + rationale as `/who`; `data.view.all` is deliberately NOT admitted (mission-data visibility ≠ identity administration). |
| Everything | explicit 403 with a "restricted by your role" page state | F-177: never a silent empty. |

Follow-on (not v1): a **self tier** — a non-admin viewer resolving *their own* identity through the portal's self-or-operator `who`/`why`. Needs a per-viewer principal join (Ory email → registry identity) rather than panoscope's operator key; sequenced with the registry data-roles resolver (rbac follow-ons item 2 / ADR 1074 build item 1).

## 4. Data sources (all existing; nothing new server-side in the portal)

| Content | Source | Notes |
|---|---|---|
| Org list | portal `GET /api/orgs` (authenticated) | `{prefix, display_name, approved, synthetic, created_at}`; alias join via the existing alias registry |
| Per-org topics | portal `GET /api/orgs/{org}/topics` (authenticated), fetched lazily per expanded org | avoids an N-orgs fan-out on page load; short server-side cache |
| Schemas / data types | deep-link to the portal's existing `schema.html?key=` viewer (rendered proto tree) | the catalog does not rebuild the schema viewer (Block 94: link out for per-layer detail) |
| Live activity overlay | panoscope's own flow/footprint state | per-key eps where `allows_key`; "no traffic" is a real verdict only when panoscope's feed is live — otherwise `unmetered` |
| Org tree + people + states | portal `GET /api/access/roster` (operator; proxied behind `who_groups`) — the same 15 s server-side cache `/who` uses | rows group by `vendor`/org; reconcile states per the built engine |
| Per-identity picture | portal `GET /api/access/who?id=` (proxied) | four-layer writes/reads with layer + reason |
| Why trace | portal `GET /api/access/why?id=&key=` (proxied) | verdict + per-layer steps; names the F-177 cause |
| Reverse topic query | portal `GET /api/access/topic?key=` (proxied) | writers/readers/receivers with reasons |
| Labels + grant edges | **do not exist yet** (ADR 1075 build items 1 + 4 are declared, unbuilt — no registry columns, no endpoints) | the page renders an explicit "no labels/grants declared on this goatnet yet" state — never fabricated lanes/edges. When the rows land, the catalog gains label lanes and the access pane gains grant edges without redesign. |

## 5. v1 deliverable (this track)

1. **Panoscope endpoints** (same lock + honesty discipline as `/api/top`): `GET /api/access/catalog` (orgs + aliases + scoped live rollup; short cache), `GET /api/access/catalog/topics?org=` (lazy per-org topics + scoped per-key rates), `GET /api/access/query/why?id=&key=` and `GET /api/access/query/topic?key=` (who_groups-gated portal proxies). Roster + per-identity detail reuse the `/api/who/*` endpoints.
2. **`/access` page** (+ `/access/identity?id=` deep-link route): catalog pane (org → topics → schema links, search, live overlay), access pane (org tree from roster, identity detail, why/topic query forms), subset-disclosure footer naming `goat access who|topic|why` as the CLI equivalents, explicit 403/portal-down/stale/empty states.
3. **Docs in the same PR**: this design doc; `data-access-cop.md` build-home resolution + R.3 status; `panoscope.md` page family note; rbac follow-ons tee-up item 0e split out (per its pickup rule); in-flight entry with the live-fire gate.

Out of scope, named: mutations of any kind; the request/approve loop (needs ADR 1075 grant rows); label lanes (same); the self tier (§3); ADR 1076 `graph`/`simulate` verbs (the page re-points at that API when it exists — it is a client of the language, never a fourth policy surface).

## 6. Rollback

Additive pages/endpoints behind the same default-off flag family; rollback = redeploy the prior panoscope image pin (config-tier, ADR 1060 dispatch).
