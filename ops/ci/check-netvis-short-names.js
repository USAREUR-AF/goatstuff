#!/usr/bin/env node
/* netvis SHORT names must be unique, and must not depend on the render.
 *
 * WHY THIS CHECK EXISTS
 *
 * The short name's whole job is to map to exactly one NetBird name. A
 * derivation that only looks at one peer at a time cannot guarantee that:
 * several machines held by one person, or one handle shared across unit
 * codes, collapse onto the same label, and an operator reading the matrix
 * axis can no longer tell which row is which machine. Nothing fails; the
 * surface simply misleads. This check makes that a failure.
 *
 * WHAT IT ASSERTS, over the whole peer set:
 *   1. every short name is unique;
 *   2. the same set of peers yields the same names whatever ORDER the API
 *      returns them in (a label that moves is a label you cannot cite);
 *   3. the short name of a peer does not change when the view is narrowed —
 *      i.e. uniqueness is computed once over the full list, not per view;
 *   4. the common case stays scannable: only colliding names are lengthened.
 *
 * USAGE
 *   ops/ci/check-netvis-short-names.js                 # the committed fixture
 *   ops/ci/check-netvis-short-names.js <reach.json>    # a live /api/reach capture
 *
 * The fixture holds every name SHAPE that collided in the field, with personal
 * handles replaced by synthetic ones of the same shape. Point the script at a
 * live capture (`curl -s https://netvis.<site>.<zone>/api/reach`) to check the
 * real fleet.
 */
'use strict';

const fs = require('fs');
const path = require('path');

const ROOT = path.resolve(__dirname, '..', '..');
const IDENTITY = path.join(ROOT, 'crates', 'netvis', 'src', 'assets', 'identity.js');
const FIXTURE = path.join(ROOT, 'crates', 'netvis', 'tests', 'fixtures', 'reach-shapes.json');
/* The axis gutter in matrix.js is `min(360, longest * 7.1 + 22)` px, so a
   label past ~47 chars is drawn into a gutter that stopped growing. */
const MAX_LABEL = 47;

const payloadPath = process.argv[2] || FIXTURE;
let fail = 0;
function bad(msg) { console.error('FAIL  ' + msg); fail++; }
function ok(msg) { console.log('ok    ' + msg); }

/* identity.js is a browser asset: an IIFE that hangs `goatIdentity` off the
   global. It takes `globalThis` precisely so it can also be read here. */
require(IDENTITY);
const goatIdentity = globalThis.goatIdentity;
if (!goatIdentity || typeof goatIdentity.disambiguate !== 'function') {
  console.error('FAIL  identity.js did not export goatIdentity.disambiguate — the uniqueness pass is gone');
  process.exit(1);
}

const payload = JSON.parse(fs.readFileSync(payloadPath, 'utf8'));
const peers = payload.peers || [];
const nodes = payload.nodes || [];
const families = (payload.panels || []).map((p) => p.family).filter((f) => f && f !== '<unscoped>');
if (!peers.length) { console.error('FAIL  ' + payloadPath + ' carries no peers'); process.exit(1); }

function label(list) {
  const ids = list.map((p) => goatIdentity.resolve(p, nodes, families));
  goatIdentity.disambiguate(ids, list);
  return ids;
}

/* 1 — uniqueness ------------------------------------------------------- */
const ids = label(peers);
const byShort = new Map();
ids.forEach((id, i) => {
  if (!byShort.has(id.short)) byShort.set(id.short, []);
  byShort.get(id.short).push(peers[i].name);
});
const dup = [...byShort.entries()].filter(([, v]) => v.length > 1);
if (dup.length) {
  bad(dup.length + ' short name(s) map to more than one peer — the derived name is not a name');
  dup.forEach(([k, v]) => console.error('        ' + v.length + 'x  ' + k + '\n' + v.map((n) => '           ' + n).join('\n')));
} else {
  ok(peers.length + ' peers -> ' + byShort.size + ' distinct short names, no collisions');
}

/* 2 — order independence ----------------------------------------------- */
const orders = {
  reversed: peers.map((p, i) => [p, i]).reverse(),
  shuffled: (() => {                                     // seeded, so a failure reproduces
    const a = peers.map((p, i) => [p, i]);
    let s = 0x9e3779b9;
    for (let i = a.length - 1; i > 0; i--) {
      s = (s ^ (s << 13)) >>> 0; s = (s ^ (s >>> 17)) >>> 0; s = (s ^ (s << 5)) >>> 0;
      const j = s % (i + 1);
      const t = a[i]; a[i] = a[j]; a[j] = t;
    }
    return a;
  })()
};
Object.keys(orders).forEach((how) => {
  const reIds = label(orders[how].map(([p]) => p));
  const moved = [];
  reIds.forEach((id, k) => {
    const original = ids[orders[how][k][1]];
    if (id.short !== original.short) moved.push(original.short + '  ->  ' + id.short);
  });
  if (moved.length) {
    bad(moved.length + ' short name(s) changed when the peer list was ' + how
        + ' — a label that moves with the payload is a label you cannot cite');
    moved.slice(0, 8).forEach((m) => console.error('        ' + m));
  } else {
    ok('short names unchanged when the peer list is ' + how);
  }
});

/* 3 — view independence ------------------------------------------------
   The failure this guards against is disambiguating per view: run the pass
   over a FILTERED list and the labels differ from the full-set ones, so a
   name an operator wrote down stops matching once they change the filter.
   The contract is that disambiguate() is called once, on everything. */
const subset = peers.filter((_, i) => i % 3 === 0);
const subIds = subset.map((p) => goatIdentity.resolve(p, nodes, families));
goatIdentity.disambiguate(subIds, subset);
let drift = 0;
subset.forEach((p, k) => { if (subIds[k].short !== ids[peers.indexOf(p)].short) drift++; });

const wired = fs.readFileSync(path.join(ROOT, 'crates', 'netvis', 'src', 'assets', 'matrix.js'), 'utf8');
if (/goatIdentity\.disambiguate\(/.test(wired)) {
  ok('matrix.js calls goatIdentity.disambiguate() (running it per view instead would move '
     + drift + ' of ' + subset.length + ' labels in a 1-in-3 subset)');
} else {
  bad('matrix.js never calls goatIdentity.disambiguate() — resolve() sees one peer and cannot know the set');
}

/* 4 — the short case stays short --------------------------------------- */
const base = peers.map((p) => goatIdentity.resolve(p, nodes, families).short);
const baseCount = base.reduce((m, s) => m.set(s, (m.get(s) || 0) + 1), new Map());
const overlong = [];
ids.forEach((id, i) => { if (baseCount.get(base[i]) === 1 && id.short !== base[i]) overlong.push(base[i] + '  ->  ' + id.short); });
const lengthened = ids.filter((id, i) => id.short !== base[i]).length;
if (overlong.length) {
  bad(overlong.length + ' label(s) lengthened that did not collide — disambiguation must be minimal, '
      + 'or every axis label pays for the few that clash');
  overlong.slice(0, 5).forEach((m) => console.error('        ' + m));
} else {
  ok('lengthened ' + lengthened + '/' + ids.length + ' labels, and only ones that collided');
}
const long = ids.filter((id) => id.short.length > MAX_LABEL);
if (long.length) {
  bad(long.length + ' label(s) over ' + MAX_LABEL + ' chars for an axis: ' + long.slice(0, 3).map((i) => i.short).join(' | '));
} else {
  const widest = ids.reduce((a, b) => (a.short.length >= b.short.length ? a : b));
  ok('widest label ' + widest.short.length + ' chars (budget ' + MAX_LABEL + '): ' + widest.short);
}

console.log(fail ? '\nnetvis short names: FAILED (' + fail + ')' : '\nnetvis short names: OK  [' + path.basename(payloadPath) + ']');
process.exit(fail ? 1 : 0);
