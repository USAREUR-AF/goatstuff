// SPDX-License-Identifier: Apache-2.0
//
// Two overlays drawn under the live tracks:
//   sites    the declared sites (/api/sites), at their coarse anchors; the
//            opening view fits them so every site is on screen
//   history  every track's path over a past window (/api/history), chosen
//            from the toolbar's History control, refreshed every minute
// Both read the same scoped APIs as the rest of the page: what a viewer may
// not see never reaches this file.

(function () {
  const HISTORY_REFRESH_MS = 60000;
  const AFF = {
    friend: '#38bdf8', 'assumed-friend': '#38bdf8', hostile: '#fb7185', suspect: '#fb7185',
    neutral: '#34d399', unknown: '#f0b429', pending: '#f0b429',
  };
  const empty = { type: 'FeatureCollection', features: [] };
  let map = null;
  let windowS = 0;
  let timer = null;

  function hoursLabel(s) {
    const h = s / 3600;
    return h >= 1 ? `${h % 1 ? h.toFixed(1) : h} h` : `${Math.round(s / 60)} min`;
  }

  function note(text) {
    const n = document.getElementById('history-note');
    if (!n) return;
    n.textContent = text || '';
    n.hidden = !text;
  }

  // ---- sites -------------------------------------------------------------
  async function loadSites() {
    let body;
    try {
      body = await (await fetch('api/sites')).json();
    } catch (err) {
      return;
    }
    const sites = (body && body.sites) || [];
    map.getSource('sites').setData({
      type: 'FeatureCollection',
      features: sites.map((s) => ({
        type: 'Feature',
        geometry: { type: 'Point', coordinates: [s.lon, s.lat] },
        properties: { label: s.label || s.site_id, systems: (s.counts && s.counts.total) || 0 },
      })),
    });
    // Open on every declared site, unless tracks have already set the view.
    const st = window.state;
    if (sites.length && !(st && st.fitted)) {
      const b = new maplibregl.LngLatBounds();
      sites.forEach((s) => b.extend([s.lon, s.lat]));
      map.fitBounds(b, { padding: { top: 140, bottom: 140, left: 340, right: 320 }, maxZoom: 9, duration: 0 });
    }
  }

  // ---- history -----------------------------------------------------------
  async function loadHistory() {
    if (!windowS) {
      map.getSource('history').setData(empty);
      map.getSource('history-ends').setData(empty);
      note('');
      return;
    }
    let body;
    try {
      const r = await fetch(`api/history?window_s=${windowS}`);
      body = await r.json();
      if (!r.ok) throw new Error(body.error || r.status);
    } catch (err) {
      note(`History unavailable: ${err.message || err}`);
      return;
    }
    const lines = [];
    const ends = [];
    for (const t of body.tracks || []) {
      const color = AFF[t.affiliation] || '#a5b4fc';
      const coords = t.points.map((p) => [p[0], p[1]]);
      const props = { id: t.id, label: t.label || t.id, color, synthetic: t.synthetic ? 1 : 0 };
      if (coords.length > 1) lines.push({ type: 'Feature', geometry: { type: 'LineString', coordinates: coords }, properties: props });
      ends.push({ type: 'Feature', geometry: { type: 'Point', coordinates: coords[coords.length - 1] }, properties: props });
    }
    map.getSource('history').setData({ type: 'FeatureCollection', features: lines });
    map.getSource('history-ends').setData({ type: 'FeatureCollection', features: ends });
    const n = (body.tracks || []).length;
    let text = `History: ${n} track${n === 1 ? '' : 's'} over the last ${hoursLabel(body.window_s)}`;
    if (body.window_s < windowS) text += ` (this service keeps ${hoursLabel(body.max_window_s)})`;
    if (body.partial) text += ' · partial view';
    note(text);
  }

  function setWindow(s) {
    windowS = Number(s) || 0;
    if (timer) clearInterval(timer);
    timer = windowS ? setInterval(loadHistory, HISTORY_REFRESH_MS) : null;
    loadHistory();
  }

  window.overlayInit = function (m) {
    map = m;
    const below = map.getLayer('trail-line') ? 'trail-line' : undefined;
    map.addSource('history', { type: 'geojson', data: empty });
    map.addSource('history-ends', { type: 'geojson', data: empty });
    map.addSource('sites', { type: 'geojson', data: empty });

    map.addLayer({
      id: 'history-line', type: 'line', source: 'history',
      layout: { 'line-cap': 'round', 'line-join': 'round' },
      paint: {
        'line-color': ['get', 'color'], 'line-width': 1.6, 'line-opacity': 0.55,
        'line-dasharray': ['case', ['==', ['get', 'synthetic'], 1], ['literal', [2, 2]], ['literal', [1]]],
      },
    }, below);
    map.addLayer({
      id: 'history-end', type: 'circle', source: 'history-ends',
      paint: { 'circle-radius': 2.5, 'circle-color': ['get', 'color'], 'circle-opacity': 0.7 },
    }, below);

    // Sites: a ring, not a filled dot, so a site never reads as a detection.
    map.addLayer({
      id: 'site-ring', type: 'circle', source: 'sites',
      paint: {
        'circle-radius': 9, 'circle-color': 'rgba(0,0,0,0)',
        'circle-stroke-width': 2, 'circle-stroke-color': '#e6edf6', 'circle-stroke-opacity': 0.85,
      },
    }, below);
    map.addLayer({
      id: 'site-label', type: 'symbol', source: 'sites',
      layout: {
        'text-field': ['get', 'label'], 'text-font': ['Noto Sans Medium'], 'text-size': 12.5,
        'text-offset': [0, 1.5], 'text-anchor': 'top', 'text-allow-overlap': true,
      },
      paint: { 'text-color': '#e6edf6', 'text-halo-color': '#0a0e14', 'text-halo-width': 1.4 },
    });

    const sel = document.getElementById('history-window');
    if (sel) {
      // ?history=<seconds> opens with that window on, so a view can be shared.
      const q = new URLSearchParams(location.search).get('history');
      if (q && [...sel.options].some((o) => o.value === q)) sel.value = q;
      sel.addEventListener('change', () => setWindow(sel.value));
      if (sel.value !== '0') setWindow(sel.value);
    }
    loadSites();
  };
})();
