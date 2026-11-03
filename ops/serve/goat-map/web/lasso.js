// SPDX-License-Identifier: Apache-2.0
//
// Lasso selection — the server decides what the map shows.
//
// WHY. goat-map drew every track in the store on load, unbounded. That does not
// survive growth: unreadable on screen, and a payload that grows with the
// estate. The fix is a QUERY, not a smaller draw.
//
// On load the query is the current VIEWPORT, so the map shows the area you are
// looking at. An earlier version started blank and waited for a lasso; on a
// six-contact site that read as a broken surface rather than a deliberate one.
//
// The lasso is a QUERY, not a view filter. The polygon goes to the server and
// the server decides what comes back — nothing is fetched and hidden. That
// distinction is the whole security value: filtering in the browser would mean
// the whole picture had already been shipped to whoever opened the page.
//
// No drawing library. A polygon is a list of clicked points; MapLibre already
// gives us click coordinates and a GeoJSON source. Adding a dependency to draw
// a closed path would cost more bytes on this link than the feature saves.

// SITE POSITIONS ARE DELIBERATELY NOT PLOTTED.
//
// An earlier version of this file drew a dot per site from the quantized
// anchor. Nobody asked for it — the request was area search for TRACKS, and
// site readiness was already a panel — and it was wrong on three counts:
// infrastructure position is a targeting aid that sites.py blurs on purpose,
// so putting it on a map hands back the thing the blur exists to withhold;
// a dot carries no sign that it is approximate, so a viewer reads it as the
// site's location; and because the anchor is quantized the marker landed in
// open water beside the actual site, which is the blur rendered as a fact.
//
// Readiness still answers "which sites, what state" in its own panel, where a
// verdict does not imply a coordinate. If site geometry is ever wanted here it
// needs an explicit decision about disclosure, not a layer added in passing.
(function () {
  var pts = [];          // [lon,lat] in click order
  var armed = false;
  var lastResult = null;

  // What the picture on screen currently answers, and over what ground it was
  // asked. Held so a camera move can be checked against it — see
  // noteCameraMoved. `null` means nothing is being asserted.
  var shownFor = null;      // 'viewport' | 'lasso' | null
  var shownBounds = null;   // {w,s,e,n} the viewport query covered
  var shownHint = null;     // {text,kind} as last stated, to re-state on staleness

  // One-shot allowance for the page's OWN camera move — see noteCameraMoved.
  var autofitGrace = true;

  function el(id) { return document.getElementById(id); }

  function polygonGeoJSON(closed) {
    if (pts.length < 2) return { type: 'FeatureCollection', features: [] };
    var ring = pts.slice();
    if (closed && ring.length >= 3) ring.push(ring[0]);
    return {
      type: 'FeatureCollection',
      features: [{
        type: 'Feature', properties: {},
        geometry: closed && ring.length >= 4
          ? { type: 'Polygon', coordinates: [ring] }
          : { type: 'LineString', coordinates: ring },
      }],
    };
  }

  function redrawLasso(closed) {
    var src = window.map && window.map.getSource('lasso');
    if (src) src.setData(polygonGeoJSON(closed));
  }

  // Erase the outline outright. This is NOT redrawLasso(false) — that draws an
  // OPEN LineString through whatever points are held, which for the four
  // viewport corners is a three-sided box (the closing leg is only appended
  // when `closed`). An area the operator did not draw must leave no mark.
  //
  // AND THE TWO LAYERS DISAGREE ABOUT AN OPEN RING, which is what made this
  // read as a rendering glitch rather than a bug. `lasso-fill` shades the
  // whole quad while `lasso-line` draws only three sides: MapLibre's fill
  // bucket applies no geometry-type filter (fill_bucket.ts hands loadGeometry()
  // straight to classifyRings), classifyRings returns a single ring unchanged
  // whether or not it is closed (`if (len <= 1) return [rings]`), and earcut
  // then triangulates it as though it were. Verified against the vendored
  // v5.24.0. So an open ring is NOT a safe way to say "draw nothing" — only an
  // empty FeatureCollection is.
  //
  // The one open ring still emitted is the 2-point in-progress draw, which is
  // deliberate and harmless: zero area, and classifyRings drops zero-area rings.
  function eraseLasso() {
    var src = window.map && window.map.getSource('lasso');
    if (src) src.setData({ type: 'FeatureCollection', features: [] });
  }

  function setHint(text, kind) {
    var h = el('lasso-hint');
    if (!h) return;
    h.hidden = !text;
    h.textContent = text || '';
    h.className = 'provenance lasso' + (kind ? ' ' + kind : '');
  }

  // Everything the surface is currently asserting, dropped at once: the
  // outline, the claim about what it covers, and the hint. Used wherever the
  // picture stops being true — a failed query, a cancel, a clear.
  function stopAsserting() {
    eraseLasso();
    shownFor = null;
    shownBounds = null;
    shownHint = null;
  }

  // --- the query ------------------------------------------------------------

  function runQuery(opts) {
    opts = opts || {};
    if (pts.length < 3) {
      setHint('A lasso needs at least three points.', 'warn');
      return;
    }
    setHint('Querying…');
    fetch('api/query', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
      body: JSON.stringify({ polygon: pts, planes: ['tracks'] }),
    })
      .then(function (r) { return r.ok ? r.json() : Promise.reject(r.status); })
      .then(function (d) {
        lastResult = d;
        applyResult(d, opts);
      })
      .catch(function (err) {
        // Say the picture is unavailable rather than leaving the last result
        // on screen looking current. A stale map is worse than an empty one.
        //
        // The OUTLINE has to go with the results. It means "the picture came
        // from in here"; over an empty map it asserts that the area was
        // searched and found empty, which is the opposite of what happened.
        clearResults();
        stopAsserting();
        setHint('Query failed (' + err + ') — nothing is being shown.', 'warn');
      });
  }

  // Query whatever the map is currently showing. This is what runs on load.
  //
  // WHY THIS EXISTS. The first version started blank and waited for a lasso.
  // On a site with six contacts that is not a feature, it is a surface that
  // looks broken — the header said "6 TRACKS" over an empty map and the only
  // affordance was a hint styled like a button. Blank-by-default was never the
  // security property; SERVER-SIDE FILTERING and a BOUNDED PAYLOAD are, and a
  // viewport query keeps both: the server still decides what comes back, and
  // the same count-driven tiering applies. The operator sees the picture they
  // navigated to, then lassoes to narrow it.
  function queryViewport() {
    if (!window.map) return;
    var b = window.map.getBounds();
    pts = [[b.getWest(), b.getSouth()], [b.getEast(), b.getSouth()],
           [b.getEast(), b.getNorth()], [b.getWest(), b.getNorth()]];
    // No outline: this is "everything on screen", not an area anyone chose.
    // Drawing one was worse than cosmetic — the box froze at the bounds
    // captured here, so after the autofit camera move (app.js fitAll) or any
    // pan it sat on the map as a stale boundary meaning nothing, and the page
    // opened looking as though a selection were already active.
    eraseLasso();
    runQuery({ viewport: true, bounds: { w: b.getWest(), s: b.getSouth(), e: b.getEast(), n: b.getNorth() } });
  }

  // Is everything now on screen inside the ground the last query covered?
  //
  // This is the test for whether a VIEWPORT result is still honest. Zooming in
  // keeps it honest — every pixel visible was searched. Panning to new ground
  // does not: contacts may sit there that were never fetched, and the map would
  // show that ground as empty without ever having asked about it.
  //
  // Refuses to judge across the antimeridian (west > east) rather than guess:
  // an unhandled wrap would silently answer "contained" for a view that is not.
  function viewInsideQueried() {
    if (!shownBounds || !window.map) return true;
    if (shownBounds.w > shownBounds.e) return true;   // wrapped — do not judge
    var b = window.map.getBounds();
    if (b.getWest() > b.getEast()) return true;
    return b.getWest() >= shownBounds.w && b.getEast() <= shownBounds.e
        && b.getSouth() >= shownBounds.s && b.getNorth() <= shownBounds.n;
  }

  // A viewport result answers "what is on screen". Move the camera onto ground
  // it never covered and that sentence stops being true — so say so, on the
  // same surface that made the claim.
  //
  // OPERATOR NAVIGATION NEVER RE-QUERIES. One person dragging a map would
  // become an unbounded stream of queries, exactly the amplification ADR 1123
  // forbids; they ask again deliberately, with "Search visible area". The one
  // exception below is the page's own camera move, which is bounded by the page.
  //
  // A LASSO result needs none of this. Its polygon is geographic, so it stays
  // over the same ground as the camera moves and remains drawn on screen —
  // it keeps showing which area the picture came from, whatever you pan to.
  function noteCameraMoved(ev) {
    if (shownFor !== 'viewport' || !shownHint) return;
    if (viewInsideQueried()) return;

    // MapLibre sets originalEvent only for a gesture — a drag, a scroll-zoom.
    // A programmatic move (fitAll, the zoom buttons) has none. The grace below
    // is for the page's own move, so a move the operator made never takes it.
    var operatorMoved = !!(ev && ev.originalEvent);

    // The page moves the camera itself, once: app.js `fitAll` frames the
    // returned tracks right after the load-time query, and with its padding it
    // can land on ground that query never covered. That disagreement is the
    // page's doing, not the operator's, so resolve it by asking again rather
    // than by warning them about a move they did not make.
    //
    // Strictly one-shot, and it cannot loop: the re-ask moves no camera, and
    // `fitAll` is itself guarded by `state.fitted` so it runs once per load.
    // The ceiling is one extra query per page load — bounded by the page, not
    // by how much the operator drags the map, which is the distinction ADR 1123
    // actually draws.
    if (autofitGrace && !operatorMoved) {
      autofitGrace = false;
      queryViewport();
      return;
    }

    shownFor = 'moved';
    setHint(shownHint.text
      + ' — THE VIEW HAS MOVED beyond the area this answered for; press '
      + '"Search visible area" to ask about what is on screen now.', 'warn');
  }

  function applyResult(d, opts) {
    opts = opts || {};
    var t = d.tracks || {};
    var bits = [];

    if (t.mode === 'features') {
      state.tracks = new Map((t.tracks || []).map(function (x) { return [x.id, x]; }));
      setCells('track-cells', []);
      bits.push((t.tracks || []).length + ' tracks');
    } else if (t.mode === 'aggregate') {
      // Above the limit the server bins rather than enumerating. Say so
      // plainly: an operator must never mistake a cluster for a contact.
      state.tracks = new Map();
      setCells('track-cells', t.cells || []);
      bits.push((t.cells || []).length + ' track clusters'
        + (t.count != null ? ' (' + t.count + ' contacts)' : ''));
    }


    // Tell the header what this view is showing. In aggregate mode the
    // individual count is deliberately not what is drawn, so report the
    // contact total when the server gave one and nothing when it withheld it.
    if (typeof setShownTracks === 'function') {
      setShownTracks(t.mode === 'features' ? (t.tracks || []).length
                     : (t.count != null ? t.count : null));
    }

    var aggregated = t.mode === 'aggregate';
    var text = (opts.viewport ? 'This view: ' : 'Lasso: ') + bits.join(' · ')
      + (aggregated ? ' — CLUSTERED, draw a smaller area for detail' : '')
      + (!aggregated && /(^|[^0-9])0 tracks/.test(bits.join(' · '))
         ? ' — none in this area; the estate may have them elsewhere' : '');
    var kind = aggregated ? 'agg' : '';
    setHint(text, kind);

    // Record what this picture answers and over what ground, so a later camera
    // move can be checked against it rather than left to look current.
    shownFor = opts.viewport ? 'viewport' : 'lasso';
    shownBounds = opts.bounds || null;
    shownHint = { text: text, kind: kind };

    if (typeof render === 'function') render();
  }

  function setCells(sourceId, cells) {
    var src = window.map && window.map.getSource(sourceId);
    if (!src) return;
    src.setData({
      type: 'FeatureCollection',
      features: cells.map(function (c) {
        return {
          type: 'Feature',
          properties: {
            // Label only when the server gave a count. Below the `full`
            // disclosure tier counts are withheld as denominators, and the
            // UI must not invent one or imply the number is unknown-but-small.
            label: c.count != null ? String(c.count) : '',
            verdict: c.verdict || '',
            synthetic: c.synthetic ? 1 : 0,
          },
          geometry: { type: 'Point', coordinates: [c.lon, c.lat] },
        };
      }),
    });
  }


  function clearResults() {
    // Reset the header too, or it keeps asserting a count for a view that is
    // no longer on screen.
    if (typeof setShownTracks === 'function') setShownTracks(null);
    state.tracks = new Map();
    setCells('track-cells', []);
    if (typeof render === 'function') render();
  }

  // --- interaction ----------------------------------------------------------

  function arm() {
    armed = true;
    pts = [];
    // Drop the previous claim before the hint becomes drawing instructions —
    // otherwise a pan mid-draw would replace those instructions with a
    // staleness notice about a query the operator has already moved on from.
    stopAsserting();
    if (window.map) window.map.getCanvas().style.cursor = 'crosshair';
    setHint('Click to place points · Backspace undoes the last · double-click or Enter to search · Esc to cancel');
  }

  function disarm() {
    armed = false;
    if (window.map) window.map.getCanvas().style.cursor = '';
  }

  function cancel() {
    disarm();
    pts = [];
    stopAsserting();
    setHint('');
  }

  window.lassoInit = function (map) {
    map.addSource('lasso', { type: 'geojson', data: polygonGeoJSON(false) });
    map.addSource('track-cells', { type: 'geojson', data: { type: 'FeatureCollection', features: [] } });

    map.addLayer({
      id: 'lasso-line', type: 'line', source: 'lasso',
      paint: { 'line-color': '#38bdf8', 'line-width': 1.6, 'line-dasharray': [2, 2] },
    });
    map.addLayer({
      id: 'lasso-fill', type: 'fill', source: 'lasso',
      paint: { 'fill-color': '#38bdf8', 'fill-opacity': 0.06 },
    });

    // Clusters render as HOLLOW SQUARES, deliberately unlike the round dots
    // used for contacts. An operator glancing at the screen must never read a
    // cluster of 400 as a single track.
    map.addLayer({
      id: 'track-cell', type: 'symbol', source: 'track-cells',
      layout: {
        'text-field': ['get', 'label'], 'text-font': ['Noto Sans Medium'],
        'text-size': 12, 'text-allow-overlap': true,
      },
      paint: { 'text-color': '#f0b429', 'text-halo-color': '#0a0e14', 'text-halo-width': 1.4 },
    });
    map.addLayer({
      id: 'track-cell-box', type: 'circle', source: 'track-cells',
      paint: {
        'circle-radius': 14, 'circle-color': 'rgba(0,0,0,0)',
        'circle-stroke-width': 1.4, 'circle-stroke-color': '#f0b429',
        'circle-stroke-opacity': 0.75,
      },
    }, 'track-cell');

    // Camera moves are checked, never acted on: the surface downgrades what it
    // claims and waits for the operator to ask again.
    map.on('moveend', function (e) { noteCameraMoved(e); });

    map.on('click', function (e) {
      if (!armed) return;
      pts.push([e.lngLat.lng, e.lngLat.lat]);
      redrawLasso(pts.length >= 3);
    });
    map.on('dblclick', function (e) {
      if (!armed) return;
      e.preventDefault();
      disarm();
      redrawLasso(true);
      runQuery();
    });

    document.addEventListener('keydown', function (ev) {
      // Backspace/Delete removes the last point. Clicking out a polygon with no
      // undo means one misplaced vertex costs you the whole shape — and the
      // longer the shape, the more it costs, which is exactly backwards.
      if ((ev.key === 'Backspace' || ev.key === 'Delete') && armed && pts.length) {
        ev.preventDefault();
        pts.pop();
        redrawLasso(pts.length >= 3);
        setHint(pts.length
          ? 'Removed the last point · ' + pts.length + ' remaining · Backspace to undo again'
          : 'All points removed · click to start again');
        return;
      }
      if (ev.key === 'Escape') cancel();
      if (ev.key === 'Enter' && armed) { disarm(); redrawLasso(true); runQuery(); }
    });

    var btn = el('lasso-btn');
    if (btn) btn.addEventListener('click', function () { armed ? cancel() : arm(); });
    var clr = el('lasso-clear');
    if (clr) clr.addEventListener('click', function () { cancel(); clearResults(); });

    var vp = el('lasso-view');
    if (vp) vp.addEventListener('click', queryViewport);

    // Show the picture for wherever the map opened, rather than nothing.
    queryViewport();
  };
})();
