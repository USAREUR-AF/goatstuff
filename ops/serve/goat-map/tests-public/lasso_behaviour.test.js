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

  global.state = { all: new Map(), tracks: new Map(), area: null, clustered: false };
  global.applyArea = () => { ctx.applied = (ctx.applied || 0) + 1; };
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
  // --- on load the picture is live: no query, no outline, no claim ----------
  let c = instance(); await c.flush();
  check('load: no query sent', c.posted.length === 0, 'posted=' + c.posted.length);
  check('load: nothing is drawn', c.feats().length === 0);
  check('load: no hint', c.hint === '', c.hint);
  check('load: no area', state.area === null);

  // --- moving the camera never queries and never warns ----------------------
  c.bounds = { w: 20, s: 20, e: 40, n: 40 };
  c.fire('moveend', { originalEvent: { type: 'mouseup' } });
  await c.flush();
  check('pan: no query', c.posted.length === 0);
  check('pan: no hint', c.hint === '', c.hint);

  const drawSquare = (k) => {
    c.buttons['lasso-btn'].click();
    [[-k, -k], [k, -k], [k, k], [-k, k]].forEach((p) => c.fire('click', { lngLat: { lng: p[0], lat: p[1] } }));
  };

  // --- a failed area search goes back to every track, and says so -----------
  c = instance(); await c.flush();
  drawSquare(5);
  check('failure precondition: an outline is drawn', c.feats().length === 1 && c.geom().type === 'Polygon');
  c.fail = true;
  c.fire('dblclick', { preventDefault: () => {} });
  await c.flush();
  check('failure: outline erased', c.feats().length === 0);
  check('failure: every track shown', state.area === null && /every track is shown/.test(c.hint), c.hint);

  // --- too many in the area: clusters, never individual tracks --------------
  c = instance(); await c.flush();
  c.response = { tracks: { mode: 'aggregate', cells: [{ lat: 0, lon: 0, count: 500 }], count: 500 } };
  drawSquare(5);
  c.fire('dblclick', { preventDefault: () => {} });
  await c.flush();
  check('clusters: flagged', state.clustered === true && /CLUSTERED/.test(c.hint), c.hint);
  c.buttons['lasso-view'].click();
  check('show all: clusters gone', state.clustered === false && state.area === null && c.feats().length === 0);

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
  check('draw: the area narrows the live picture', state.area.length === 4 && /live/.test(c.hint), c.hint);

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
