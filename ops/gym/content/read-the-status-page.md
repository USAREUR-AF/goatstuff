# Reading the status page without being misled

The status page (`isitup`) is the first thing most people open and the easiest
thing to misread. It answers one question well — *is the service running where
it is deployed* — and three questions that people ask of it, badly.

## What a row is

Each row comes from a `isitup_services` entry in your goatnet's group_vars. The
fields you can see, and what they actually assert:

| Field | What it asserts | What it does NOT assert |
|---|---|---|
| green / red | A probe from the host succeeded | That *you* can reach it |
| `access:` | What fence sits in front — `public`, `admins`, `api` | Your own rights |
| `kind:` | `ui` or `api` — whether a human or a program is the caller | Anything about health |
| `category:` | Where the row is grouped on the page | Criticality |
| `desc:` | What the service is for | Whether it is the one you want |

## What the page shows you first

The page folds. Sign-in, docs and tools are open; every other section is a
single line with a count, and a section holding a faulted row opens itself —
so a closed section with no warning on its heading has nothing wrong in it.

Some rows carry a **Planned** or **Retiring** label. Planned means declared
and not brought up yet; retiring means on its way out. Neither is counted in
the banner at the top, and neither is alerted on, so a red dot beside one is
expected rather than an outage.

Services that members host on this goatnet — a partner's platform, a
member's tool — are not on this page. They are on **External services**,
linked at the top, grouped by the owner who runs each one. A problem with one
of those goes to its owner, not to the platform team.

## The three misreadings

**"It is green so I can open it."** The probe runs *on the host*, inside the
fence. A row can be green and hand you a 403. Read `access:` first: `admins`
means an oauth2-proxy fence with an allowed-groups list, and `api` means the
service correctly answers 401/403 to an unauthenticated caller — which is the
service *working*, and is why those rows declare `expect_codes` rather than
demanding a 200.

**"It is not red, so nothing is wrong."** There is a third state. A row whose
`container:` name does not exist on the host reports *probe unavailable* — an
UNKNOWN, which renders as neither up nor down and therefore is the state nobody
chases.

**"It is on the page, so it is reachable."** A row declares an FQDN. If no mesh
DNS record exists for that name, the service deploys **dark**: up, healthy,
green, and unresolvable by every peer. This is F-197, and it is why the per-site
DNS zones are *generated* from these same rows and CI-gated for drift — the row
is the declaration, and the generator makes the declaration true.

## What the page cannot tell you

- **Its own health.** There is no `isitup` row for isitup. Its outage looks like
  every row going stale rather than any row turning red.
- **Whether the running container is the one you deployed.** An interrupted
  ansible run leaves single-file bind mounts converged on the host and stale
  inside the container; the next run prints `changed=0 failed=0`, and the page
  stays green throughout. Green means running, not current.
- **Whether the data behind it is fresh.** A UI that renders a cached page
  perfectly is green. Freshness is the health surface's question, not this
  page's.

## The habit worth building

Before you open a row, say out loud what you expect: up or down, and whether
*you* will get in. Then click. The rows you get wrong are the ones telling you
something about the fence, the DNS tier, or your own groups — and each of those
is worth more than the page you were trying to reach.
