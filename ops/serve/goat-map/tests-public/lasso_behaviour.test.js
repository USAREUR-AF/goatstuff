// Behaviour test for web/lasso.js — what the map ASSERTS, and when it stops.
//
// WHY THIS EXISTS. The bug that prompted it was invisible to every check we
// had: `redrawLasso(false)` under a comment reading "no outline" drew a
// three-sided box that then sat on the map, frozen at bounds the camera had
// already left. It parsed, it lint-cleaned, and the Python suite cannot reach
// a browser file at all. The defect was in what the surface CLAIMED, so that
// is what this asserts.
//
// No test framework and no dependencies: `node tests-public/lasso_behaviour.test.js`.
// A browser-driver suite for one 300-line file would cost more to keep alive
// than the file, and the whole point is that this runs on every PR.
//
// It eval()s the real lasso.js against a stub MapLibre. The file is an IIFE
// with no exports, so there is nothing to import — and re-implementing it here
// would test a copy, which is how the original comment came to disagree with
// the original code in the first place.

const fs = require('fs');
const path = require('path');

const SRC_PATH = process.argv[2] || path.join(__dirname, '..', 'web', 'lasso.js');
const SRC = fs.readFileSync(SRC_PATH, 'utf8');

function instance(opts) {
  opts = opts || {};
  const ctx = {
    hint: '', kind: '', posted: [], sources: {}, buttons: {}, handlers: {},
    bounds: { w: -10, s: -10, e: 10, n: 10 },
    fail: !!opts.failFirst,
    response: { tracks: { mode: 'features', tracks: [{ id: 'a' }, { id: 'b' }] } },
  };

  global.state = { tracks: new Map() };
  global.render = () => {};
  global.setShownTracks = () => {};
  global.document = {
    getElementById: (id) => {
      if (id === 'lasso-hint') {
        return {
          set hidden(v) {}, set textContent(v) { ctx.hint = v; },
          set className(v) { ctx.kind = v; },
        };
      }
      return ctx.buttons[id] || (ctx.buttons[id] = {
        addEventListener: (ev, fn) => { ctx.buttons[id].click = fn; },
      });
    },
    addEventListener: () => {},
  };
  global.fetch = (url, o) => {
    ctx.posted.push(JSON.parse(o.body));
    return ctx.fail ? Promise.reject(503)
      : Promise.resolve({ ok: true, json: () => Promise.resolve(ctx.response) });
  };

  const map = {
    getBounds: () => ({
      getWest: () => ctx.bounds.w, getSouth: () => ctx.bounds.s,
      getEast: () => ctx.bounds.e, getNorth: () => ctx.bounds.n,
    }),
    getSource: (id) => ({ setData: (d) => { ctx.sources[id] = d; } }),
    addSource: (id, cfg) => { ctx.sources[id] = cfg.data; },
    addLayer: () => {},
    getCanvas: () => ({ style: {} }),
    on: (ev, a, b) => {
      const fn = typeof a === 'function' ? a : b;
      (ctx.handlers[ev] = ctx.handlers[ev] || []).push(fn);
    },
    once: () => {},
  };
  global.window = { map };

  eval(SRC);
  window.lassoInit(map);

  ctx.fire = (ev, arg) => (ctx.handlers[ev] || []).forEach((f) => f(arg));
  ctx.feats = () => (ctx.sources['lasso'] && ctx.sources['lasso'].features) || [];
  ctx.geom = () => ctx.feats()[0] && ctx.feats()[0].geometry;
  ctx.flush = () => new Promise((r) => setTimeout(r, 5));
  return ctx;
}

const R = [];
const check = (n, c, d) => R.push({ ok: !!c, n, d });

(async () => {
  // --- the load-time query must leave no mark -------------------------------
  let c = instance(); await c.flush();
  check('load: one query sent', c.posted.length === 1, 'posted=' + c.posted.length);
  check('load: polygon is the 4 viewport corners', c.posted[0].polygon.length === 4);
  check('load: NOTHING is drawn', c.feats().length === 0,
        JSON.stringify(c.feats().map((f) => f.geometry.type)));
  check('load: hint states the view', /This view/.test(c.hint), c.hint);

  // --- the page's own camera move re-asks once, silently --------------------
  c.bounds = { w: 20, s: 20, e: 40, n: 40 };
  c.fire('moveend', {});                       // programmatic: no originalEvent
  await c.flush();
  check('autofit: re-asked instead of warning', c.posted.length === 2,
        'posted=' + c.posted.length);
  check('autofit: did not warn', !/MOVED/.test(c.hint), c.hint);

  c.bounds = { w: 60, s: 60, e: 80, n: 80 };
  c.fire('moveend', {});
  await c.flush();
  check('autofit allowance is one-shot', c.posted.length === 2,
        'posted=' + c.posted.length);
  check('a later move warns instead', /MOVED/.test(c.hint), c.hint);

  // --- operator navigation NEVER re-queries (ADR 1123) ----------------------
  c = instance(); await c.flush();
  const n = c.posted.length;
  c.bounds = { w: 20, s: 20, e: 40, n: 40 };
  c.fire('moveend', { originalEvent: { type: 'mouseup' } });
  await c.flush();
  check('drag onto new ground: no query', c.posted.length === n,
        'posted=' + c.posted.length);
  check('drag onto new ground: warns', /MOVED/.test(c.hint), c.hint);

  // --- zooming IN stays honest: every visible pixel was searched ------------
  c = instance(); await c.flush();
  const n2 = c.posted.length;
  c.bounds = { w: -2, s: -2, e: 2, n: 2 };
  c.fire('moveend', { originalEvent: { type: 'wheel' } });
  await c.flush();
  check('zoom in: no warning', !/MOVED/.test(c.hint), c.hint);
  check('zoom in: no query', c.posted.length === n2);

  // --- a failed query asserts nothing --------------------------------------
  //
  // Draw a real lasso FIRST, so there is an outline on the map to lose. A
  // failing load-time query would pass this trivially — nothing was ever drawn
  // — and prove nothing about the path that matters: an outline left over an
  // empty map says the area was searched and found empty, the opposite of what
  // a failure means.
  c = instance(); await c.flush();
  c.buttons['lasso-btn'].click();
  [[-5, -5], [5, -5], [5, 5], [-5, 5]].forEach((p) =>
    c.fire('click', { lngLat: { lng: p[0], lat: p[1] } }));
  c.fire('dblclick', { preventDefault: () => {} });
  await c.flush();
  check('failure precondition: an outline is drawn',
        c.feats().length === 1 && c.geom().type === 'Polygon');

  // It must be a LASSO query that fails, not "Search visible area": the
  // viewport path erases the outline on its way in, so a failure there would
  // pass whether or not the error handler does anything. The lasso path draws
  // the ring and then queries, which is the only path where the handler is
  // what removes it.
  c.fail = true;
  c.buttons['lasso-btn'].click();               // re-arm and draw again
  [[-6, -6], [6, -6], [6, 6], [-6, 6]].forEach((p) =>
    c.fire('click', { lngLat: { lng: p[0], lat: p[1] } }));
  check('failure precondition 2: outline redrawn before the failing query',
        c.feats().length === 1 && c.geom().type === 'Polygon');
  c.fire('dblclick', { preventDefault: () => {} });
  await c.flush();
  check('failure: outline erased', c.feats().length === 0,
        JSON.stringify(c.feats().map((f) => f.geometry.type)));
  check('failure: says nothing is shown', /nothing is being shown/.test(c.hint), c.hint);
  c.bounds = { w: 50, s: 50, e: 60, n: 60 };
  c.fire('moveend', { originalEvent: {} });
  await c.flush();
  check('failure: no stale claim after a move', !/MOVED/.test(c.hint), c.hint);

  // --- a DRAWN lasso is a CLOSED ring: four sides, not three ----------------
  c = instance(); await c.flush();
  c.fire('click', { lngLat: { lng: 0, lat: 0 } });
  check('click while disarmed is ignored', c.feats().length === 0);

  c.buttons['lasso-btn'].click();
  check('arm: hint gives drawing instructions', /Click to place points/.test(c.hint), c.hint);

  [[-5, -5], [5, -5], [5, 5], [-5, 5]].forEach((p) =>
    c.fire('click', { lngLat: { lng: p[0], lat: p[1] } }));
  check('draw: Polygon, not LineString', c.geom().type === 'Polygon', c.geom().type);
  const ring = c.geom().coordinates[0];
  check('draw: ring CLOSED (4 sides drawn, not 3)',
        ring.length === 5 && ring[0][0] === ring[4][0] && ring[0][1] === ring[4][1],
        'ring points=' + ring.length);

  const n3 = c.posted.length;
  c.fire('dblclick', { preventDefault: () => {} });
  await c.flush();
  check('draw: dblclick queries', c.posted.length === n3 + 1);
  check('draw: hint says Lasso', /^Lasso:/.test(c.hint), c.hint);

  // A lasso polygon is geographic: it stays over the same ground, so panning
  // away neither invalidates it nor triggers anything.
  const n4 = c.posted.length;
  c.bounds = { w: 70, s: 70, e: 80, n: 80 };
  c.fire('moveend', { originalEvent: { type: 'mouseup' } });
  await c.flush();
  check('lasso survives a pan: no warning', !/MOVED/.test(c.hint), c.hint);
  check('lasso survives a pan: no query', c.posted.length === n4);
  check('lasso survives a pan: still drawn',
        c.feats().length === 1 && c.geom().type === 'Polygon');

  // --- an in-progress 2-point draw is a zero-area LineString ----------------
  // Deliberate: MapLibre's fill layer WILL shade an open ring (it applies no
  // geometry-type filter), but classifyRings drops a zero-area one, so this
  // preview cannot smear a fill across the map the way the 4-corner box did.
  c = instance(); await c.flush();
  c.buttons['lasso-btn'].click();
  c.fire('click', { lngLat: { lng: 0, lat: 0 } });
  c.fire('click', { lngLat: { lng: 4, lat: 4 } });
  check('2-point draw is a LineString', c.geom().type === 'LineString', c.geom().type);
  check('2-point draw has zero area', c.geom().coordinates.length === 2);

  // --- Clear drops every claim ---------------------------------------------
  c = instance(); await c.flush();
  c.buttons['lasso-clear'].click();
  check('clear: no outline', c.feats().length === 0);
  check('clear: no hint', c.hint === '', c.hint);
  c.bounds = { w: 70, s: 70, e: 80, n: 80 };
  c.fire('moveend', { originalEvent: {} });
  await c.flush();
  check('clear: no stale claim after a move', !/MOVED/.test(c.hint), c.hint);

  const failed = R.filter((r) => !r.ok);
  console.log(R.map((r) => (r.ok ? '  ok   ' : '  FAIL ') + r.n +
    (r.d ? '   [' + r.d + ']' : '')).join('\n'));
  console.log('\n' + (R.length - failed.length) + '/' + R.length + ' passed');
  process.exit(failed.length ? 1 : 0);
})();
