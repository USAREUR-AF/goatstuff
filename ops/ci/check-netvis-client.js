#!/usr/bin/env node
/* netvis CLIENT smoke — the levels must draw, and draw what they claim.
 *
 * WHY THIS EXISTS
 *
 * Every silent failure in netvis of this kind lives in the client, and each
 * one looked from outside exactly like a correct empty result:
 *
 *   - `L4` was CALLED by paint() and never written. It threw a ReferenceError
 *     under a correct breadcrumb, so the end of every drill was a blank page
 *     that read as "no data" rather than "no code". It survived its whole
 *     life that way.
 *   - the filter narrowed DESTINATIONS as well as rows, so the page reported
 *     "1 machine reaching 0 machines" for a machine that reaches 108.
 *   - `Edge.realization` was on the wire from day one and read nowhere, so
 *     79% of the strokes on the aggregate levels were unpopulated design
 *     drawn identically to live reach.
 *   - a site pair with no cross-family rule drew an EMPTY cell, which on this
 *     matrix means "no rule permits it" — false wherever machines hold groups
 *     in both sites.
 *   - drilling an off-diagonal cell filtered both axes to the UNION of the
 *     two sites, so on a cross-family drill most of what you saw was not
 *     what you clicked.
 *
 * The crate had zero JavaScript tests. This is the harness for that class:
 * it runs the real `identity.js` + `matrix.js` against a complete `/api/reach`
 * document under a minimal DOM, and asserts on the elements they produce.
 *
 * USAGE
 *   ops/ci/check-netvis-client.js                  # the committed fixture
 *   ops/ci/check-netvis-client.js <reach.json>     # a live /api/reach capture
 *
 * Structural assertions only — nothing here depends on a name in the fixture
 * that a live capture would not have, except where it says so.
 */
'use strict';

const fs = require('fs');
const path = require('path');
const vm = require('vm');

const ROOT = path.resolve(__dirname, '..', '..');
const ASSETS = path.join(ROOT, 'crates', 'netvis', 'src', 'assets');
const FIXTURE = path.join(ROOT, 'crates', 'netvis', 'tests', 'fixtures', 'reach-doc.json');

const payloadPath = process.argv[2] || FIXTURE;
const PAYLOAD = JSON.parse(fs.readFileSync(payloadPath, 'utf8'));
const LIVE = payloadPath !== FIXTURE;

/* The gate's payload. Structural, and deliberately carrying one of each
   verdict — including `undetermined`, which is the one a renderer is most
   likely to drop because it has no counterexamples to draw. */
const ASSERTIONS = {
  provenance: {},
  declared_tests: 3,
  report: {
    site: 'fixture',
    source: 'live',
    verdict: 'undetermined',
    satisfied: 1,
    violated: 1,
    baselined: 0,
    undetermined: 1,
    baseline_now_passing: [],
    quantifier: 'all-to-all: every source must reach every destination',
    outcomes: [
      { name: 'admins reach the substrate on ssh', check: 'existence', verdict: 'satisfied',
        counterexamples: [], checked_pairs: 1 },
      { name: 'quarantine holds', check: 'isolation', verdict: 'violated', checked_pairs: 2,
        counterexamples: [{ src: 'SITE-A-QUARANTINED', dst: 'SITE-A-KID', port: '*', proto: 'any',
                            granted_by: 'site-a infra → kid' }] },
      { name: 'a typo', check: 'existence', verdict: 'undetermined', reason: 'unknown_group',
        counterexamples: [], checked_pairs: 0 },
    ],
  },
};

/* The /api/pair document behind the ownership lens. Two peers with DELIBERATELY
   DIFFERENT attributions, because the whole risk in this feature is a renderer
   that flattens them: `registry` is the recorded linkage and may be presented
   as the answer and linked; `name` parsed a handle but matched no live identity
   and is a lead only. A page that shows both the same way sends someone to page
   the wrong human. See crates/panoscope-core/src/peerowner.rs. */
const PAIR = {
  provenance: {},
  id_console_base: 'https://admin.auth.fixture.example.test',
  ownership_notes: [],
  source_to_destination: { permitted: true, paths: [] },
  destination_to_source: { permitted: false, paths: [] },
  both_ends_connected: true,
  source_owner: {
    netbird_name: 'PEER_A',
    attribution: 'registry',
    handle: 'aroster',
    org: 'goat',
    person_name: 'A Roster',
    person_email: 'a@example.test',
    ory_identity_id: 'ory-1',
    id_console_path: '/users/ory-1',
  },
  destination_owner: {
    netbird_name: 'PEER_B',
    attribution: 'name',
    handle: 'bhandle',
    org: 'goat',
    reason: 'handle resolved to org goat but no live Ory identity matched the roster email-hash',
  },
  /* Stewardship is the FALLBACK for a machine no person owns. The source end
     here is registry-attributed and must NOT get one — appending "and here are
     4 admins" under a named person invites paging the wrong one. */
  source_stewardship: {
    scope: 'ZAGROS-PROD', admin_group: 'ZAGROS-PROD-PLATFORM-ADMIN',
    scope_is_whole_goatnet: false,
    admins: [{ netbird_name: 'operator-laptop', connected: true }],
    principals: [{ person_name: 'A Roster', handle: 'aroster', devices: ['operator-laptop'],
                   devices_online: 1, unresolved: false }],
    basis: 'ZAGROS-PROD-PLATFORM-ADMIN administers scope ZAGROS-PROD (ADR 1124)',
  },
  destination_stewardship: {
    scope: 'SITE_ONE', admin_group: 'SITE_ONE-PLATFORM-ADMIN',
    scope_is_whole_goatnet: false,
    admins: [{ netbird_name: 'peer-sam-example-zagros-prod', connected: true },
             { netbird_name: 'peer-rtexample5768-zagros-prod', connected: false },
             { netbird_name: 'iPhone', connected: false }],
    /* THREE devices, TWO principals — one of them holding two machines, plus an
       unresolved device. The row must lead with PEOPLE; listing three entries
       here would be the defect this fixture exists to catch. */
    principals: [
      { person_name: 'Sam Example', handle: 'sam-example', id_console_path: '/users/ory-2',
        devices: ['peer-sam-example-zagros-prod', 'peer-rtexample5768-zagros-prod'], devices_online: 1,
        unresolved: false },
      { devices: ['iPhone'], devices_online: 0, unresolved: true },
    ],
    /* A machine in TWO scopes. Showing only the first was arbitrary group
       ordering deciding who is answerable — real on ZAGROS, where goat-tak-01
       holds ZAGROS-EXTERNAL-INFRA and EX-DEMO-POD. */
    also_scopes: [{
      scope: 'EX-DEMO', admin_group: 'EX-DEMO-PLATFORM-ADMIN',
      admins: [{ netbird_name: 'goat-tak-01', connected: true }],
      principals: [{ person_name: 'Ex Battery', handle: 'exbatt', devices: ['goat-tak-01'],
                     devices_online: 1, unresolved: false }],
    }],
    basis: 'SITE_ONE-PLATFORM-ADMIN administers scope SITE_ONE (ADR 1124)',
  },
};

/* Person links in the OWNER rows only. The revealed admin roster legitimately
   links a resolved administrator to their console page, and those live in
   .steward-person inside a disclosure — counting page-wide made a correct new
   affordance look like the flattening defect. Scope the assertion to the rows
   it is about; third time this file has needed that lesson. */
const ownerLinks = (host) =>
  byClass(host, 'owner-row')
    .filter((r) => r.cls().indexOf('steward') < 0)
    .flatMap((r) => walk(r).filter((n) => n.tagName === 'a'));

let fail = 0;
const bad = (m) => { console.error('FAIL  ' + m); fail++; };
const ok = (m) => console.log('ok    ' + m);

/* ------------------------------------------------------------------ DOM
   Just enough of one. The client touches a small, enumerable surface, and
   pulling in a full DOM implementation would make this check something the
   repo has to keep on its feet rather than something it can run. */
class El {
  constructor(tag) {
    this.tagName = tag;
    this.children = [];
    this.attributes = {};
    this.style = {};
    this.className = '';
    this.parentNode = null;
    this._text = '';
  }
  get textContent() {
    return this.children.length
      ? this.children.map((c) => c.textContent).join('')
      : this._text;
  }
  set textContent(v) {          // assignment CLEARS children — body() relies on it
    this.children.forEach((c) => { c.parentNode = null; });
    this.children = [];
    this._text = v == null ? '' : String(v);
  }
  get lastChild() { return this.children[this.children.length - 1] || null; }
  appendChild(c) { c.parentNode = this; this.children.push(c); return c; }
  removeChild(c) {
    const i = this.children.indexOf(c);
    if (i >= 0) { this.children.splice(i, 1); c.parentNode = null; }
    return c;
  }
  setAttribute(k, v) { this.attributes[k] = String(v); }
  getAttribute(k) { return this.attributes[k]; }
  /* RECORDED, not swallowed. This was a no-op, so anything reachable only by
     CLICKING — the identity card above all — could not be tested at all, and a
     check written against it passed by never opening it. That is the proxy-
     assertion failure this file has now hit three times. */
  addEventListener(ev, fn) { (this._on = this._on || {})[ev] = (this._on[ev] || []).concat(fn); }
  click() { ((this._on || {}).click || []).forEach((f) => f({ preventDefault() {} })); }
  /** class as the RENDERER set it: HTML uses className, SVG uses the attr. */
  cls() { return this.className || this.attributes.class || ''; }
}

function walk(node, out = []) {
  out.push(node);
  node.children.forEach((c) => walk(c, out));
  return out;
}
const find = (root, pred) => walk(root).filter(pred);
const byClass = (root, c) =>
  find(root, (n) => (' ' + n.cls() + ' ').indexOf(' ' + c + ' ') >= 0);
const byTag = (root, t) => find(root, (n) => n.tagName === t);

/* Run identity.js + matrix.js against one URL and hand back the root the
   client drew into. A fresh context per scenario, because the client keys its
   whole state off `location` at boot. */
async function render(search, pathname = '/') {
  const host = new El('div');
  host.attributes.id = 'reach-root';
  host.clientWidth = 1400;

  const document = {
    readyState: 'complete',
    createElement: (t) => new El(t),
    createElementNS: (_ns, t) => new El(t),
    getElementById: (id) => (id === 'reach-root' ? host : null),
    querySelector: () => null,
    addEventListener: () => {},
  };
  const sandbox = {
    document,
    location: { pathname, search },
    history: { pushState() {} },
    navigator: {},
    console,
    URLSearchParams,
    setTimeout,
    clearTimeout,
    // Each endpoint gets its OWN document. Serving the reach payload to every
    // caller would make a check pass on a page that never renders the thing
    // the endpoint exists for.
    fetch: (url) => Promise.resolve({
      json: () => Promise.resolve(
        String(url).startsWith('/api/assertions') ? ASSERTIONS
        : String(url).startsWith('/api/pair/') ? PAIR
        : PAYLOAD),
    }),
  };
  sandbox.window = sandbox;
  sandbox.globalThis = sandbox;
  sandbox.window.innerHeight = 1000;
  /* Recorded, not discarded. The resize handler re-enters body(), which is a
     second way into every level and therefore a second way to break one — the
     gate was reachable on load and threw on resize, and a harness that drops
     the listener cannot see that. */
  const listeners = {};
  sandbox.window.addEventListener = (ev, fn) => { (listeners[ev] = listeners[ev] || []).push(fn); };

  const ctx = vm.createContext(sandbox);
  for (const f of ['identity.js', 'matrix.js']) {
    vm.runInContext(fs.readFileSync(path.join(ASSETS, f), 'utf8'), ctx, { filename: f });
  }
  // Let boot()'s fetch chain settle.
  for (let i = 0; i < 8; i++) await new Promise((r) => setImmediate(r));
  host._listeners = listeners;
  return host;
}

/* The matrix proper — never the legend, which draws swatches with the same
   classes on purpose. Cells are appended row-major after the axis labels, so
   index k is (row k/cols, col k%cols). */
function grid(host) {
  const mx = byClass(host, 'mx')[0];
  if (!mx) return null;
  const svg = mx.children[0];
  const cells = svg.children.filter((n) => n.tagName === 'rect');
  // `_text`, not `textContent`: every axis label carries a <title> child and
  // the tooltip is not the label.
  const colLabels = svg.children
    .filter((n) => n.tagName === 'text' && n.attributes.transform)
    .map((n) => n._text);
  const rowLabels = svg.children
    .filter((n) => n.tagName === 'text' && n.attributes['text-anchor'] === 'end')
    .map((n) => n._text);
  return { cells, rowLabels, colLabels, cols: colLabels.length, rows: rowLabels.length };
}
const lit = (g) => g.cells.filter((c) => / on( |$)/.test(' ' + c.cls()));
const cellAt = (g, r, c) => g.cells[r * g.cols + c];
const text = (host) => walk(host).map((n) => n._text).join(' ');

/* A render that threw is the failure mode this whole file is about. paint()
   catches and says so, which is right for a viewer and useless for a check
   unless the check looks for it. */
function drew(host, what) {
  const warn = byClass(host, 'warn').map((n) => n.textContent).join(' ');
  if (/failed to render/.test(warn)) { bad(what + ': the level threw — ' + warn); return false; }
  return true;
}

(async () => {
  // ── 1. every level renders, and none of them throws ───────────────────
  const LEVELS = [
    ['', '/', 'site'],
    ['?level=class', '/', 'class'],
    ['?level=g', '/', 'group'],
    ['', '/policy', 'policy'],
    ['', '/findings', 'findings'],
    ['', '/armed', 'inert rules'],
    ['', '/assertions', 'assertions'],
  ];
  for (const [search, pathname, name] of LEVELS) {
    const host = await render(search, pathname);
    if (!drew(host, name)) continue;
    if (!host.children.length) { bad(name + ': drew nothing at all'); continue; }
    ok(name + ' renders');
  }

  /* Every level again, through the OTHER entry point. A resize re-enters
     body() without re-running boot(), so a level that depends on state boot()
     only sets on its own path renders once and throws on the first window
     resize — which is what the gate did: body() had no assertions branch, fell
     through to L0, and L0 reads a GSITE that prepare() never built. */
  for (const [search, pathname, name] of LEVELS) {
    const host = await render(search, pathname);
    const rs = (host._listeners && host._listeners.resize) || [];
    if (!rs.length) { bad(name + ': registered no resize handler — the harness or the client changed'); continue; }
    rs.forEach((fn) => fn());
    await new Promise((r) => setTimeout(r, 200)); // the handler debounces 120ms
    if (!drew(host, name + ' after resize')) continue;
    if (!host.children.length) { bad(name + ': drew nothing after a resize'); continue; }
    ok(name + ' survives a resize');
  }

  // ── 2. the realization ladder is ENCODED, not merely shipped ──────────
  const site = await render('');
  const g0 = grid(site);
  if (!g0) {
    bad('site level drew no matrix');
  } else {
    const rungs = new Set();
    g0.cells.forEach((c) => {
      const m = /\br-(realized|dormant|latent|unknown)\b/.exec(c.cls());
      if (m) rungs.add(m[1]);
    });
    if (!rungs.size) {
      bad('no cell carries a realization class — Edge.realization is on the wire and drawn nowhere again');
    } else {
      ok('site cells carry the realization ladder: ' + [...rungs].sort().join(', '));
    }
    /* A site cell takes the STRONGEST rung underneath it, so `latent` is
       invisible at that altitude by design. It has to appear where the pair
       is a pair — otherwise the encoding is present and never reached, which
       is the same defect one layer along. */
    const gg = grid(await render('?level=g'));
    if (!gg) {
      bad('the group level drew no matrix');
    } else if (!gg.cells.some((c) => /\br-latent\b/.test(c.cls()))) {
      bad('no group pair is drawn as latent — 79% of permitted pairs on a real goatnet are');
    } else {
      ok('the group level draws latent pairs as latent');
    }
    // The distinction the summary used to collapse.
    const t = text(site);
    if (!/LATENT/.test(t)) {
      bad('the summary never names LATENT — with 98 latent to 5 dormant, calling them all dormant is wrong for 95%');
    } else {
      ok('the summary distinguishes latent from dormant');
    }
  }

  // ── 3. a site pair crossed only by membership is DRAWN ────────────────
  //
  // Fixture-specific by design: it is the one shape a live capture may not
  // contain, and asserting it needs named sites.
  if (!LIVE && g0) {
    const A = g0.rowLabels.indexOf('SITE-A');
    const B = g0.colLabels.indexOf('SITE-B');
    const C = g0.colLabels.indexOf('SITE-C');
    if (A < 0 || B < 0 || C < 0) {
      bad('the fixture sites are not on the axes: ' + g0.rowLabels.join(', '));
    } else {
      const bridge = cellAt(g0, A, C);
      const ruled = cellAt(g0, A, B);
      if (!/ on( |$)/.test(' ' + bridge.cls())) {
        bad('SITE-A → SITE-C is drawn as a DENIAL. No rule connects them, but a machine holds groups in both, so reach across that boundary is real');
      } else if (!/m-other/.test(bridge.cls())) {
        bad('SITE-A → SITE-C is drawn as if a rule permitted it — the bridge must be distinguishable from a grant');
      } else {
        ok('SITE-A → SITE-C is drawn as bridged-by-membership, not as a denial');
      }
      if (!/m-pair/.test(ruled.cls())) {
        bad('SITE-A → SITE-B is rule-permitted and is not drawn as such');
      } else {
        ok('SITE-A → SITE-B is drawn as rule-permitted');
      }
    }

    const br = await render('?level=bridge&sa=SITE-A&sb=SITE-C');
    if (drew(br, 'bridge')) {
      const rows = byTag(br, 'tr').length - 1;   // less the header
      if (rows !== 1) {
        bad('the bridge level lists ' + rows + ' machine(s); exactly one holds groups in both sites');
      } else if (!/SITE-A-KID/.test(text(br)) || !/SITE-C-KID/.test(text(br))) {
        bad('the bridge level does not name the groups the machine holds on each side — that pair IS the door');
      } else {
        ok('the bridge level names the one machine that is in both sites, and its groups on each side');
      }
    }
  }

  // ── 4. a directional drill answers the question that was asked ────────
  if (!LIVE) {
    const cross = await render('?level=class&sa=SITE-A&sb=SITE-B');
    if (drew(cross, 'cross-site drill')) {
      const g = grid(cross);
      if (!g) {
        bad('the cross-site drill drew no matrix');
      } else {
        // Columns are the DESTINATION site only. SITE-A carries a class
        // SITE-B does not, so a union would show it.
        if (g.colLabels.indexOf('quarantined') >= 0) {
          bad('the cross-site drill put a SITE-A-only class on the DESTINATION axis — both axes are still the union of the two sites');
        } else {
          ok('the cross-site drill scopes rows to the source site and columns to the destination');
        }
        const n = lit(g).length;
        if (n !== 1) {
          bad('the cross-site drill lights ' + n + ' cell(s); exactly one group pair crosses SITE-A → SITE-B. The others are the two diagonals the drill navigated past');
        } else {
          ok('the cross-site drill lights only the crossing that was clicked');
        }
      }
    }

    // INFRA classifies to MGMT-HOST (identity.js SUFFIX) — the class is
    // what the axis carries, not the suffix.
    const grp = await render('?level=g&sa=SITE-A&sb=SITE-B&ca=MGMT-HOST&cb=MGMT-HOST');
    if (drew(grp, 'group drill')) {
      const g = grid(grp);
      if (!g) bad('the group drill drew no matrix');
      else if (g.rowLabels.length !== 1 || g.colLabels.length !== 1) {
        bad('the group drill shows ' + g.rowLabels.length + '×' + g.colLabels.length
            + ' groups; the clicked cell names one class in one site on each axis');
      } else {
        ok('the group drill shows exactly the two groups behind the cell');
      }
    }
  }

  // ── 4b. the gate reports every verdict, and says which way it rolls up ─
  {
    const host = await render('', '/assertions');
    if (drew(host, 'assertions')) {
      const t = text(host);
      for (const [needle, why] of [
        ['VIOLATED', 'a violated assertion is not named as violated'],
        ['UNDETERMINED', 'an undecidable assertion is not named as undecidable — greying it out tells the reader nothing while looking like it told them something'],
        ['SITE-A-QUARANTINED', 'the counterexample pair is not shown; a bare false is not a finding'],
        ['site-a infra → kid', 'the rule that grants what should not be granted is not named'],
        ['all-to-all', 'the quantifier is not stated, so the verdict is undefined'],
      ]) {
        if (!t.includes(needle)) bad('assertions: ' + why);
      }
      if (!/could not be DECIDED/.test(t)) {
        bad('assertions: one undetermined outcome did not cap the headline — an unknown must not read as a pass');
      } else {
        ok('the gate names every verdict and lets undetermined cap the roll-up');
      }
    }
  }

  /* ── 4b. the no-site row survives being empty, and says why ───────────
     The fixture's `All` is in no scope family and no rule references it, so
     its row and column are entirely dark — which is what the operator asked
     about, and the shape someone will eventually "fix" by hiding the row.
     Hiding it defeats the row's whole job: a group created without a site
     prefix lands here and changes THE LABEL, and a stray group nobody has
     written a rule for yet has no lit cell to be found by. So the row must
     stay, and the tooltip must carry the reason — otherwise the next reader
     files it as a bug again. */
  {
    const host = await render('');
    const mx = byClass(host, 'mx')[0];
    const g = grid(host);
    if (!g || !mx) {
      bad('no-site row: the site level drew no matrix');
    } else {
      /* Found by its TOOLTIP, not its label: the label is derived from the
         groups in the bucket, so it is `All` on the fixture, `All + mesh-dns`
         on zagros and something else again on the next goatnet. The phrase
         "not a site" is the part that identifies the row anywhere. */
      const tips = walk(mx).filter((n) => n.tagName === 'title' && /not a site/.test(n._text));
      const r = g.rowLabels.findIndex((l) => tips.some((t) => t._text.split('\n')[0] === l));
      if (!tips.length || r < 0) {
        bad('the no-site bucket is missing from the site axis — an unpopulated row was hidden, '
          + 'and with it the only signal that names a group created without a site prefix');
      } else {
        const tip = tips[0]._text;
        /* Both branches are real: `All` carries no rule on the fixture and on
           zagros, and DOES carry one on zagros-prod (`SSH Access` sources from
           it). A live capture of either must read correctly, so the check
           follows the picture rather than assuming the empty case. */
        const anyLit = g.rowLabels.some((_, i) =>
          / on( |$)/.test(' ' + cellAt(g, r, i).cls()) || / on( |$)/.test(' ' + cellAt(g, i, r).cls()));
        const want = anyLit ? /this row is live/ : /empty on purpose/;
        if (!LIVE && anyLit) {
          bad('no-site row: the fixture is meant to leave it EMPTY (nothing references `All`) — '
            + 'the fixture changed and this check no longer covers the empty case');
        } else if (!want.test(tip) || !/by accident/.test(tip)) {
          bad('no-site row (' + (anyLit ? 'lit' : 'empty') + '): the tooltip does not say which '
            + 'it is, nor that the row stays to catch a group that lands here — ' + tip);
        } else {
          ok('the no-site row is drawn ' + (anyLit ? 'lit' : 'unpopulated')
            + ' and its tooltip explains why it is there');
        }
      }
    }
  }

  /* ── 4c. the peer legend must not call a GRANT a non-connection ───────
     A dimmed cell at the peer level is a permitted pair with one machine's
     netbird session down. The legend used to call that "not connected",
     which is what an EMPTY cell means on the same grid — the same words for
     a grant and for a denial. This asserts the words never come back, and
     that the legend says which of the two a dimmed cell is. */
  {
    const host = await render('?level=peers&ga=0&gb=1');
    const lbox = byClass(host, 'legend-box')[0];
    if (!drew(host, 'peer level')) {
      bad('peer legend: the peer level did not render');
    } else if (!lbox) {
      bad('peer legend: the peer level drew no legend box');
    } else {
      /* The LEGEND only, not the whole page. identity.js says "enrolled, not
         connected" in a machine's axis tooltip, and there the words are
         right: that is one machine's own session, not a claim about a pair.
         Scoping this to the legend is the difference between a check that
         names the defect and one that bans a correct phrase everywhere. */
      const t = lbox.textContent;
      if (/not connected/.test(t)) {
        bad('peer legend: "not connected" is back. A dimmed cell is a PERMITTED pair with a '
          + 'machine offline; an empty cell is the denial. The legend must not use one phrase '
          + 'for both.');
      } else if (!/PERMITTED, an end is offline/.test(t) || !/both ends online/.test(t)) {
        bad('peer legend: the four liveness rows do not name the state — expected "both ends '
          + 'online" and "PERMITTED, an end is offline"');
      } else if (!/A dimmed cell is a GRANT/.test(t)) {
        bad('peer legend: nothing on the page says a dimmed cell is a grant rather than a denial');
      } else {
        ok('the peer legend names a dimmed cell as a grant, not as a non-connection');
      }
    }
  }

  /* ── 4d. the ownership lens must not flatten its four attributions ────
     "Who is responsible for this machine" is the question a viewer has once
     the matrix has told them WHICH machine. The answer is only useful if its
     PROVENANCE travels with it: `registry` is the recorded linkage (roster
     handle + a live identity) and is the only one safe to act on; `name`
     parsed a handle and matched no identity, so it is a lead; `unmatched`
     means the name encodes no principal, which is NOT "unowned"; `unread`
     means we did not look, which says nothing at all.

     Flattening any pair of those is the defect. Two are especially dangerous:
     rendering `name` like `registry` sends someone to page a person we have
     not actually confirmed, and rendering `unread` like `unmatched` reports a
     failure to measure as a measurement — the exact mistake that hid netvis
     itself for three days. */
  {
    const host = await render('?level=p&pa=0&pb=1');
    const rows = byClass(host, 'owner-row');
    /* THE ROWS ONLY, never the whole page. The disclosure under this section
       defines every one of these words in prose, so a page-wide text match
       passes whether or not a single row rendered — the first draft of this
       check did exactly that and survived deleting the provenance token. Same
       scoping lesson as 4c above. */
    const t = rows.map((r) => r.textContent).join(' | ');
    if (!rows.length) {
      bad('ownership: the pair pane drew no owner rows — a viewer has no way to find out who to ask');
    } else if (rows.length < 2) {
      bad('ownership: only ' + rows.length + ' owner row drawn; a pair has two ends and a one-way '
        + 'grant is a different question for each owner');
    } else {
      const links = ownerLinks(host).filter((a) => /admin\.auth\.fixture/.test(a.getAttribute('href') || ''));
      if (!links.length) {
        bad('ownership: the registry-attributed owner is not a link — the id-admin-console '
          + 'deep-link is the whole point of resolving the person');
      } else if (links.length > 1) {
        bad('ownership: ' + links.length + ' person links drawn. ONLY the registry attribution may '
          + 'be a link; an unconfirmed handle rendered as a clickable person is the flattening '
          + 'this check exists to prevent');
      } else if (!/A Roster/.test(t)) {
        bad('ownership: the resolved person\'s name is not on the page');
      } else if (!/unconfirmed/.test(t)) {
        bad('ownership: the name-attributed owner is not marked unconfirmed — it is indistinguishable '
          + 'from the recorded linkage, so a reader cannot tell a lead from an answer');
      } else if (byClass(host, 'owner-prov').length < rows.length) {
        /* Asserted on the ELEMENTS, not by matching the row's concatenated
           text: this mock joins children with no separator, so `goat` + `name`
           reads as `goatname` and a \bname\b match silently fails. Counting the
           tokens says exactly what the rule is — one per row, always. */
        bad('ownership: ' + byClass(host, 'owner-prov').length + ' provenance tokens for '
          + rows.length + ' rows. It must be on EVERY row, including the good one — a badge that '
          + 'appears only when something is wrong teaches readers to scan for the badge instead '
          + 'of reading the row');
      } else if (!['name', 'registry'].every((a) =>
                   byClass(host, 'owner-prov').some((n) => n.textContent === a))) {
        /* The OWNER attributions must both be present. Asserted by containment
           rather than an exact set, because a scope-stewardship row legitimately
           adds its own `scope` token — that row is a different claim with a
           different provenance and check 4g owns it. An exact-match assertion
           here made a correct new state look like a regression. */
        bad('ownership: the provenance tokens do not name both owner attributions in the fixture');
      } else {
        ok('the ownership lens renders provenance per row and links only the recorded linkage');
      }
    }
  }

  /* ── 4e. "we did not look" must not read as "there is no owner" ───────── */
  {
    const savedS = PAIR.source_owner, savedD = PAIR.destination_owner;
    PAIR.source_owner = { netbird_name: 'PEER_A', attribution: 'unread',
                          reason: 'portal unreachable: timeout' };
    PAIR.destination_owner = { netbird_name: 'PEER_B', attribution: 'unmatched',
                               reason: 'no roster handle matched this peer name' };
    const host = await render('?level=p&pa=0&pb=1');
    PAIR.source_owner = savedS; PAIR.destination_owner = savedD;
    // Rows only — the disclosure prose defines both phrases. See 4d.
    const t = byClass(host, 'owner-row').map((r) => r.textContent).join(' | ');
    if (!/not looked up/.test(t)) {
      bad('ownership: an UNREAD owner does not say it was not looked up — a resolver outage is '
        + 'being reported as a fact about the machine');
    } else if (!/no principal in the name/.test(t)) {
      bad('ownership: an UNMATCHED owner does not say the name carries no principal — it reads '
        + 'as "unowned", which is false for every infrastructure host');
    } else if (ownerLinks(host).filter((a) => /admin\.auth/.test(a.getAttribute('href') || '')).length) {
      bad('ownership: a person link was drawn for a peer with no resolved identity');
    } else {
      ok('unread and unmatched render as different, self-describing states');
    }
  }

  /* ── 4f. every cell must be hittable over its whole area ──────────────
     Static CSS assertion, not a DOM one: this harness has no style engine, so
     the only way to catch the regression is to read the stylesheet.

     `fill: none` in SVG means the interior is NOT PAINTED, and an unpainted
     interior is not a pointer target. Cells drawn as outlines (m-other —
     permitted via another membership, 138 of 660 pairs on ZAGROS's peer level)
     were clickable only on a 1.5px stroke around a ~12px cell. They carried a
     pointer cursor and a tooltip and almost never opened.

     It regressed by a rule that only meant to change appearance: an earlier
     block sets `fill: transparent` (a paint, hittable), a later block at equal
     specificity overrode it with `fill: none`, and the hit area went with it.
     `pointer-events: all` is what decouples "how it is painted" from "whether
     it can be clicked", so it must survive any future restyling. */
  {
    const css = fs.readFileSync(path.join(ASSETS, 'netvis.css'), 'utf8');
    const rule = css.match(/\.netvis\s+rect\.c\s*\{[^}]*\}/g) || [];
    const hasPE = rule.some((r) => /pointer-events\s*:\s*all/.test(r));
    const hasFillNone = rule.some((r) => /fill\s*:\s*none/.test(r));
    if (!rule.length) {
      bad('cell hit area: no `.netvis rect.c` rule found — the selector moved and this check is blind');
    } else if (hasFillNone && !hasPE) {
      bad('cell hit area: `.netvis rect.c` sets `fill: none` without `pointer-events: all`. '
        + 'An unpainted SVG interior is not a hit target, so every outline-drawn cell is '
        + 'clickable only on its ~1.5px stroke.');
    } else if (!hasPE) {
      bad('cell hit area: `.netvis rect.c` no longer declares `pointer-events: all`. Keep it even '
        + 'if the fill is currently painted — it is what stops a restyle from silently removing '
        + 'the hit area again.');
    } else {
      ok('cells are hittable over their whole area regardless of fill');
    }
  }

  /* ── 4g. scope stewardship answers for a machine, without pretending ──
     A machine owns no person, so the person lens correctly says "no principal
     in the name" — and that leaves the viewer with nobody to ask. Stewardship
     names who administers the machine's scope instead.

     It is a WEAKER claim than a resolved identity (mesh-derived: group
     membership, not the registry) and the whole risk is it being read as an
     equivalent one. Two rules, both asserted here: it must NOT appear under a
     registry-resolved owner, and it must carry its own `scope` provenance
     token rather than borrowing `registry`. */
  {
    const host = await render('?level=p&pa=0&pb=1');
    const stew = byClass(host, 'steward').filter((n) => n.cls().indexOf('owner-row') >= 0);
    const t = stew.map((r) => r.textContent).join(' | ');
    if (!stew.length) {
      bad('stewardship: no scope row drawn for the machine end — a viewer with an unowned '
        + 'machine still has nobody to ask');
    } else if (/ZAGROS-PROD/.test(t)) {
      /* Asserted by CONTENT, not by row count. The count was a proxy and it
         broke the moment a machine legitimately sat in two scopes — the source
         end here is registry-resolved and must contribute NO scope row, which
         is checkable directly by its scope name never appearing. A proxy
         assertion fails on correct new behaviour; this one fails only on the
         defect it names. */
      bad('stewardship: the registry-resolved end contributed a scope row. A resolved person is '
        + 'the stronger claim and stands alone; drawing admins under it invites paging the '
        + 'wrong human.');
    } else if (!/SITE_ONE/.test(t)) {
      bad('stewardship: the row drawn is not the machine end\'s scope');
    } else if (byClass(host, 'owner-prov').every((n) => n.textContent !== 'scope')) {
      bad('stewardship: the scope row carries no `scope` provenance token — it is '
        + 'indistinguishable from a registry-resolved owner');
    } else if (!/EX-DEMO/.test(t)) {
      /* A machine in two scopes has two sets of people who may act on it.
         Showing only the first was arbitrary group ordering deciding a
         responsibility answer — real on ZAGROS, where goat-tak-01 holds
         ZAGROS-EXTERNAL-INFRA and EX-DEMO-POD and reported only ZAGROS. */
      bad('stewardship: the machine sits in a SECOND scope and only the first is drawn. Group '
        + 'ordering must not decide who is answerable.');
    } else {
      ok('scope stewardship answers only where the person lens did not, names every scope, and labels itself');
    }
  }

  /* ── 4h. the scope row names PEOPLE, and says when the scope is everything ─
     Two defects an operator hit on the live surface, both about the row being
     technically true and practically useless:

     (1) It listed the admin group's MEMBERS, which are peers — so "who do I
         ask" was answered with hardware. On ZAGROS that was 11 device names for
         at most 5 humans, three of them one person's desktop, laptop and
         phone. Eleven names look like more information than five and carry
         less.
     (2) On a single-scope goatnet "scope ZAGROS" is tautological — every machine
         is in it, every machine gets the same admins — and the word "scope"
         implies a subdivision that does not exist there. The operator asked
         what it meant, which is the correct reaction to a label that promises
         narrowing and does not narrow.

     The fixture's destination end has THREE devices held by TWO principals
     (one holding two, one unresolved), so a renderer that leaks devices into
     the lead fails here. */
  {
    const host = await render('?level=p&pa=0&pb=1');
    const stew = byClass(host, 'steward').filter((n) => n.cls().indexOf('owner-row') >= 0);
    const t = stew.map((r) => r.textContent).join(' | ');
    if (!stew.length) {
      bad('scope row: not drawn');
    } else if (!/Sam Example/.test(t)) {
      bad('scope row: leads with something other than the administering PERSON');
    } else if (/peer-rtexample5768/.test(t)) {
      bad('scope row: a DEVICE name leaked into the summary line. The lead must be people; '
        + 'devices belong in the disclosure, or "who do I ask" is answered with hardware.');
    } else if (!/1 person|2 people/.test(t)) {
      bad('scope row: does not state how many PEOPLE — the count of devices is not the count '
        + 'of humans and the difference is the point');
    } else {
      ok('the scope row leads with people and keeps devices out of the summary');
    }
  }

  /* ── 4i. a single-scope goatnet must say the scope is the whole goatnet ── */
  {
    const saved = PAIR.destination_stewardship.scope_is_whole_goatnet;
    PAIR.destination_stewardship.scope_is_whole_goatnet = true;
    const host = await render('?level=p&pa=0&pb=1');
    PAIR.destination_stewardship.scope_is_whole_goatnet = saved;
    const t = byClass(host, 'steward').map((r) => r.textContent).join(' | ');
    if (!/whole goatnet/.test(t)) {
      bad('scope row: a scope that IS the whole goatnet renders like a narrowing one. '
        + '"scope ZAGROS" where everything is ZAGROS is tautological and reads as a subdivision '
        + 'that does not exist — say so instead.');
    } else {
      ok('a scope covering the whole goatnet says so rather than implying a subdivision');
    }
  }

  /* ── 4i-2. an unappointed scope-local admin group is not an accusation ──
     The canonical group set is complete-by-prefix so every scope has the same
     shape, and a scope-local PLATFORM-ADMIN exists so an activity CAN appoint
     an admin for its duration without making anyone a permanent goatnet-wide
     one. Most scopes never appoint one — EX-DEMO has 12 declared groups and
     one populated, by design.

     The row previously rendered that as "has no members", which reads as a
     gap, and the operator had already corrected this tree once for treating
     declared-and-empty as a finding. What a reader needs is where authority
     DOES sit: the host goatnet's admins, acting through whatever host-scope
     groups the machine also holds. That is not inheritance — SITE_ONE-PLATFORM-
     ADMIN reaches SITE_ONE-INFRA and nothing names ZAGROS-PROD-INFRA — it is a
     statement of where to go.

     A scope nothing hosts must still read bare: that one IS unanswerable, and
     collapsing the two cases would hide the only version worth acting on. */
  {
    const saved = JSON.parse(JSON.stringify(PAIR.destination_stewardship.also_scopes));
    PAIR.destination_stewardship.also_scopes = [{
      scope: 'EX-DEMO', admin_group: 'EX-DEMO-PLATFORM-ADMIN',
      admins: [], principals: [], host_scope: 'ZAGROS',
    }];
    const hosted = (await render('?level=p&pa=0&pb=1'));
    const th = byClass(hosted, 'steward').map((r) => r.textContent).join(' | ');

    PAIR.destination_stewardship.also_scopes = [{
      scope: 'ORPHAN', admin_group: 'ORPHAN-PLATFORM-ADMIN', admins: [], principals: [],
    }];
    const orphan = (await render('?level=p&pa=0&pb=1'));
    const to = byClass(orphan, 'steward').map((r) => r.textContent).join(' | ');
    PAIR.destination_stewardship.also_scopes = saved;

    if (!/ZAGROS/.test(th) || !/unappointed|hosted on/.test(th)) {
      bad('scope row: an empty scope-local PLATFORM-ADMIN on a HOSTED scope does not name the '
        + 'host. The group being unappointed is the design; leaving the reader with "no members" '
        + 'and no onward address is the defect.');
    } else if (/no members/.test(th)) {
      bad('scope row: a hosted scope with an unappointed local admin still reads "has no members". '
        + 'That phrasing makes a deliberate design read as a gap — the same alarm-on-declared-and-'
        + 'empty this surface was corrected for once already.');
    } else if (!/no members|no PLATFORM-ADMIN/.test(to)) {
      bad('scope row: a scope NOTHING hosts must still read as unanswerable. Softening that case '
        + 'too would hide the only version of this that anyone needs to act on.');
    } else {
      ok('an unappointed scope-local admin names its host; a scope nothing hosts still reads bare');
    }
  }

  /* ── 4j. the identity card shows what a box IS, not what it is tagged ──
     ADR 1116 §2: a class is a FUNCTION and a box holds one per function it
     performs; §3 gives the five slots. A machine holding SENSOR-FEED and
     SENSOR-TASKING is ONE sensor with two facets, and the card used to list
     two unrelated strings.

     The second assertion is the one that matters for tone: absent slots must
     read "does not", never "missing". §2 means an absent slot is a function
     the box does not perform — ordinary, not a gap — and this surface has
     already been corrected once for treating declared-and-empty as an alarm.
     A renderer that says "missing" reintroduces exactly that. */
  {
    const peers = PAYLOAD.peers || [];
    if (!peers.length) {
      bad('gear card: fixture has no peers');
    } else {
      /* Applied to EVERY fixture peer, because the card that opens is chosen
         by AXIS order, not array order — keying on peers[0] made the check
         depend on a layout detail and it opened a different machine. */
      const saved = peers.map((p) => p.gear);
      const GEAR = [{
        family: 'SENSOR',
        facets: [{ slot: 'data-plane', group: 'SITE_ONE-SENSOR-FEED', class: 'SENSOR-FEED' },
                 { slot: 'command-intake', group: 'SITE_ONE-SENSOR-TASKING', class: 'SENSOR-TASKING' }],
        slots_absent: ['telemetry', 'arbiter'],
      }];
      peers.forEach((p) => { p.gear = GEAR; });
      const host = await render('?level=peers&ga=0&gb=1');
      // Open it the way a reader does: click the machine's axis label.
      /* The FIRST machine label, which is peer 0 — the one the fixture gave
         gear to. Clicking whichever label happened to match first made the
         check depend on axis order. */
      const labels = find(host, (n) => n.cls().indexOf('ident') >= 0 && (n._on || {}).click);
      if (labels.length) labels[0].click();
      const cards = byClass(host, 'idcard');
      const cardText = cards.map((c) => c.textContent).join(' ');
      peers.forEach((p, i) => { p.gear = saved[i]; });
      if (!cards.length) {
        bad('gear card: clicking a machine label opened no identity card — the card is '
          + 'unreachable and any assertion about it would pass by never running');
      } else if (!/SENSOR/.test(cardText)) {
        bad('gear card: the family is not named — the box reads as tags, not as a thing');
      } else if (/missing/.test(cardText)) {
        bad('gear card: absent slots read as "missing". §2 means the box does not PERFORM that '
          + 'function, which is ordinary; "missing" is the declared-and-empty false alarm again.');
      } else if (!/does not/.test(cardText)) {
        bad('gear card: absent slots are not stated at all — the picture is half a box');
      } else {
        ok('the identity card names the gear family and phrases absent slots as "does not"');
      }
    }
  }

  // ── 5. the honesty refusal still fires ────────────────────────────────
  {
    const saved = PAYLOAD.policies_read;
    PAYLOAD.policies_read = false;
    const host = await render('');
    PAYLOAD.policies_read = saved;
    if (grid(host)) {
      bad('a matrix was drawn with policies_read=false — an empty cell there is an UNKNOWN, not a denial');
    } else if (!/cannot read the NetBird management API/.test(text(host))) {
      bad('policies_read=false drew no matrix and said nothing about why');
    } else if (((byClass(host, 'refused-notes')[0] || { children: [] }).children.filter((n) => n.tagName === 'li').length) !== (PAYLOAD.notes || []).length) {
      /* Each refusal gets its own line; joined into one paragraph with the
         caveat they read as a wall of text. */
      bad('an unread policy set must list each refusal on its own line, not run them together');
    } else {
      ok('an unread policy set refuses the matrix, says so in one sentence, and lists each refusal');
    }
  }

  // ── 6. the warrant lens reaches the page, not just the API ────────────
  //
  // An API field nobody renders is not a surface: the lens fields on /api/reach
  // must reach the page.
  {
    /* peers 0 and 2, not 0 and 1: both of the latter sit in SITE-A-INFRA, so
       no edge connects them and no rules table is drawn at all. Edge 0
       (SITE-A-INFRA -> SITE-A-KID) carries rule 0, which is the fixture rule
       holding the lens. */
    const host = await render('?level=p&pa=0&pb=2');
    const body = text(host);
    if (!/port-set/.test(body)) {
      bad('the pair pane has no port-set column — the warrant lens is API-only again');
    } else if (!/substrate/.test(body)) {
      bad('a rule carrying port_set=substrate rendered without naming the set');
    } else if (!/expires 2027-01-31/.test(body)) {
      bad('expires did not render — it is registry-only and appears on NO other '
        + 'surface, which is the whole reason this column exists');
    } else {
      ok('the pair pane names the port-set and shows the expiry');
    }
  }

  console.log(fail ? '\n' + fail + ' failure(s)' : '\nnetvis client: all checks pass');
  process.exit(fail ? 1 : 0);
})();
