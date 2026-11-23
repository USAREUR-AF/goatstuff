// SPDX-License-Identifier: Apache-2.0
//
// Lasso selection — the server decides what the map shows.
//
// WHY. goat-map drew every track in the store on load, unbounded. That does not
// survive growth: unreadable on screen, and a payload that grows with the
// estate. The fix is a QUERY, not a smaller draw.
//
// On load the map shows every track the viewer may see, live from the stream.
// Drawing an area narrows that to the area, still live: a contact that enters
// it appears and one that leaves it goes. Above the server's limit the area is
// shown as clusters, refreshed on a timer.
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
  var clusterTimer = null;

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

  // Back to the whole live picture: no outline, no area, no hint.
  function showAll() {
    disarm();
    pts = [];
    eraseLasso();
    setCells('track-cells', []);
    clearInterval(clusterTimer);
    state.area = null;
    state.clustered = false;
    setHint('');
    applyArea();
    render();
  }

  // --- the query ------------------------------------------------------------

  function runQuery(opts) {
    opts = opts || {};
    if (pts.length < 3) {
      setHint('A lasso needs at least three points.', 'warn');
      return;
    }
    var asked = pts.slice();
    if (!opts.quiet) setHint('Querying…');
    fetch('api/query', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
      body: JSON.stringify({ polygon: asked, planes: ['tracks'] }),
    })
      .then(function (r) { return r.ok ? r.json() : Promise.reject(r.status); })
      .then(function (d) {
        if (armed || pts.length !== asked.length || pts.some(function (p, i) { return p !== asked[i]; })) return;
        applyResult(d, asked);
      })
      .catch(function (err) {
        if (opts.quiet) return;
        showAll();
        setHint('The area search failed (' + err + '); every track is shown.', 'warn');
      });
  }

  function applyResult(d, area) {
    var t = d.tracks || {};
    state.area = area;
    state.clustered = t.mode === 'aggregate';
    setCells('track-cells', state.clustered ? t.cells || [] : []);
    clearInterval(clusterTimer);
    if (state.clustered) {
      // Clusters come from a query, so they are asked for again on a timer.
      clusterTimer = setInterval(function () { runQuery({ quiet: true }); }, 5000);
      // An operator must never mistake a cluster for a contact.
      setHint('Drawn area: ' + (t.cells || []).length + ' track clusters'
        + (t.count != null ? ' (' + t.count + ' contacts)' : '')
        + '. CLUSTERED, draw a smaller area for detail', 'agg');
    } else {
      setHint('Showing the drawn area only, live. Clear shows every track.');
    }
    applyArea();
    if (state.clustered) setShownTracks(t.count != null ? t.count : null);
    render();
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


  // --- interaction ----------------------------------------------------------

  function arm() {
    showAll();
    armed = true;
    if (window.map) window.map.getCanvas().style.cursor = 'crosshair';
    setHint('Click to place points · Backspace undoes the last · double-click or Enter to search · Esc to cancel');
  }

  function disarm() {
    armed = false;
    if (window.map) window.map.getCanvas().style.cursor = '';
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
      if (ev.key === 'Escape' && armed) showAll();
      if (ev.key === 'Enter' && armed) { disarm(); redrawLasso(true); runQuery(); }
    });

    var btn = el('lasso-btn');
    if (btn) btn.addEventListener('click', function () { armed ? showAll() : arm(); });
    var clr = el('lasso-clear');
    if (clr) clr.addEventListener('click', showAll);

    var vp = el('lasso-view');
    if (vp) vp.addEventListener('click', showAll);
  };
})();
