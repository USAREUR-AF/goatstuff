// SPDX-License-Identifier: Apache-2.0
//
// goat-map front end. Renders GeoTrackV1 and nothing else -- it has no
// knowledge of CoT, ASTERIX, SAPIENT or Zenoh key grammar. Swapping the
// sensor family upstream should require no change in this file.

const AFF_COLOR = {
  friend: '#38bdf8',
  'assumed-friend': '#38bdf8',
  hostile: '#fb7185',
  suspect: '#fb7185',
  neutral: '#34d399',
  unknown: '#f0b429',
  pending: '#f0b429',
};
const AFF_FALLBACK = '#a5b4fc';

// Seconds after which a track is drawn as fading, and the age at which the
// server would have dropped it anyway.
const FADE_START_S = 20;

const state = {
  tracks: new Map(),
  selected: null,
  meta: null,
  autofit: true,
  fitted: false,
};

const el = (id) => document.getElementById(id);

function colorFor(t) {
  return AFF_COLOR[t.affiliation] || AFF_FALLBACK;
}

function ageOf(t) {
  return (Date.now() - Date.parse(t.observed_at)) / 1000;
}

// A short leader line in the direction of travel, length proportional to
// speed (30 seconds of run-out). Reads as heading without needing an icon.
function leader(t) {
  if (t.course_deg == null || t.speed_mps == null || t.speed_mps <= 0) return null;
  const metres = Math.max(120, t.speed_mps * 30);
  const rad = (t.course_deg * Math.PI) / 180;
  const dLat = (metres * Math.cos(rad)) / 111320;
  const dLon = (metres * Math.sin(rad)) / (111320 * Math.cos((t.lat * Math.PI) / 180));
  return [[t.lon, t.lat], [t.lon + dLon, t.lat + dLat]];
}

function featureCollections() {
  const points = [];
  const trails = [];
  const leaders = [];
  for (const t of state.tracks.values()) {
    const age = ageOf(t);
    const fade = age <= FADE_START_S ? 1 : Math.max(0.18, 1 - (age - FADE_START_S) / 60);
    const props = {
      id: t.id,
      label: t.label,
      color: colorFor(t),
      fade,
      alert: t.kind === 'alert' ? 1 : 0,
      synthetic: t.synthetic ? 1 : 0,
      selected: state.selected === t.id ? 1 : 0,
    };
    points.push({ type: 'Feature', properties: props, geometry: { type: 'Point', coordinates: [t.lon, t.lat] } });
    if (t.trail && t.trail.length > 1) {
      trails.push({ type: 'Feature', properties: props, geometry: { type: 'LineString', coordinates: t.trail } });
    }
    const ld = leader(t);
    if (ld) leaders.push({ type: 'Feature', properties: props, geometry: { type: 'LineString', coordinates: ld } });
  }
  return {
    points: { type: 'FeatureCollection', features: points },
    trails: { type: 'FeatureCollection', features: trails },
    leaders: { type: 'FeatureCollection', features: leaders },
  };
}

// ---------------------------------------------------------------- map ----

const protocol = new pmtiles.Protocol();
maplibregl.addProtocol('pmtiles', protocol.tile);

// MapLibre requires an absolute sprite URL, and the service is host-agnostic
// (mesh IP today, a mesh DNS name behind Traefik next), so the style's asset
// URLs are rewritten against wherever the page was actually served from.
// The basemap extract is per-goatnet (GOATMAP_BASEMAP -> /api/meta), NOT a
// constant. A filename is a site name, and this repo's rule is that site names
// live in deploy/goatnets/<goatnet>.env only.
// Unset is a hard error, not a fallback: serving *some other environment's*
// geography must never be the default.
async function loadStyle() {
  const name = state.meta && state.meta.basemap;
  if (!name) throw new Error('no GOATMAP_BASEMAP configured for this goatnet');
  const style = await (await fetch('basemap/style-dark.json')).json();
  const base = location.origin;
  style.sprite = `${base}/basemap/sprites/dark`;
  style.glyphs = `${base}/basemap/fonts/{fontstack}/{range}.pbf`;
  style.sources.protomaps.url = `pmtiles://${base}/basemap/${name}`;
  return style;
}

let map = null;
let mapReady = false;

function initMap(style) {
  // The opening view comes from GOATMAP_CENTER/GOATMAP_ZOOM via /api/meta,
  // which boot has already fetched by the time this runs. The fallback is a
  // zoomed-out null island: obviously nowhere, rather than convincingly
  // somewhere real that happens to be another environment.
  const m = state.meta || {};
  map = new maplibregl.Map({
    container: 'map',
    style,
    center: m.center || [0, 0],
    zoom: m.center ? (m.zoom || 10) : 1,
    attributionControl: { compact: true },
    hash: false,
  });
  map.addControl(new maplibregl.NavigationControl({ showCompass: false }), 'bottom-right');
  map.addControl(new maplibregl.ScaleControl({ unit: 'metric' }), 'bottom-right');
  map.on('load', onMapLoad);
}

function onMapLoad() {
  const empty = { type: 'FeatureCollection', features: [] };
  map.addSource('trails', { type: 'geojson', data: empty });
  map.addSource('leaders', { type: 'geojson', data: empty });
  map.addSource('tracks', { type: 'geojson', data: empty });

  map.addLayer({
    id: 'trail-line',
    type: 'line',
    source: 'trails',
    layout: { 'line-cap': 'round', 'line-join': 'round' },
    paint: {
      'line-color': ['get', 'color'],
      'line-width': 1.4,
      'line-opacity': ['*', 0.34, ['get', 'fade']],
      // Dashed trail is the second, redundant synthetic cue — colour alone
      // fails for colour-blind viewers and on a projector.
      'line-dasharray': ['case', ['==', ['get', 'synthetic'], 1],
        ['literal', [2, 2]], ['literal', [1]]],
    },
  });

  map.addLayer({
    id: 'leader-line',
    type: 'line',
    source: 'leaders',
    paint: {
      'line-color': ['get', 'color'],
      'line-width': 1.2,
      'line-opacity': ['*', 0.75, ['get', 'fade']],
    },
  });

  map.addLayer({
    id: 'track-halo',
    type: 'circle',
    source: 'tracks',
    paint: {
      'circle-radius': ['case', ['==', ['get', 'selected'], 1], 15, 11],
      'circle-color': ['get', 'color'],
      'circle-opacity': ['*', 0.16, ['get', 'fade']],
      'circle-stroke-width': ['case', ['==', ['get', 'selected'], 1], 1.2, 0],
      'circle-stroke-color': ['get', 'color'],
      'circle-stroke-opacity': 0.8,
    },
  });

  map.addLayer({
    id: 'track-dot',
    type: 'circle',
    source: 'tracks',
    paint: {
      'circle-radius': ['case', ['==', ['get', 'alert'], 1], 5.5, 4],
      // Synthetic contacts render HOLLOW: a filled dot means a real
      // detection and nothing else. A viewer who cannot tell simulation from
      // reality at a glance is worse off than one with no map at all.
      'circle-color': ['case', ['==', ['get', 'synthetic'], 1], 'rgba(0,0,0,0)', ['get', 'color']],
      'circle-opacity': ['get', 'fade'],
      'circle-stroke-width': ['case', ['==', ['get', 'synthetic'], 1], 2, 1],
      'circle-stroke-color': ['case', ['==', ['get', 'synthetic'], 1], ['get', 'color'], '#0a0e14'],
      'circle-stroke-opacity': 0.85,
    },
  });

  map.addLayer({
    id: 'track-label',
    type: 'symbol',
    source: 'tracks',
    layout: {
      'text-field': ['case', ['==', ['get', 'synthetic'], 1],
        ['concat', 'SIM ', ['get', 'label']], ['get', 'label']],
      'text-font': ['Noto Sans Medium'],
      'text-size': 11,
      'text-offset': [0, -1.15],
      'text-allow-overlap': false,
    },
    paint: {
      'text-color': ['get', 'color'],
      'text-opacity': ['get', 'fade'],
      'text-halo-color': '#0a0e14',
      'text-halo-width': 1.2,
    },
  });

  map.on('click', 'track-dot', (e) => select(e.features[0].properties.id, true));
  map.on('mouseenter', 'track-dot', () => (map.getCanvas().style.cursor = 'pointer'));
  map.on('mouseleave', 'track-dot', () => (map.getCanvas().style.cursor = ''));

  mapReady = true;
  // The lasso owns the map's contents from here. Its sources and layers are
  // added after the base layers so clusters draw above the basemap.
  window.map = map;
  if (typeof window.lassoInit === 'function') window.lassoInit(map);
  render();
}

function render() {
  // The panels are rendered whether or not the map is ready: a basemap that
  // fails to load must not take the track list down with it.
  renderList();
  renderDetail();
  if (!mapReady) return;
  const fc = featureCollections();
  map.getSource('tracks').setData(fc.points);
  map.getSource('trails').setData(fc.trails);
  map.getSource('leaders').setData(fc.leaders);
  if (state.autofit && !state.fitted && state.tracks.size > 0) {
    fitAll();
    state.fitted = true;
  }
}

function fitAll() {
  if (!mapReady || state.tracks.size === 0) return;
  const b = new maplibregl.LngLatBounds();
  for (const t of state.tracks.values()) b.extend([t.lon, t.lat]);
  map.fitBounds(b, { padding: { top: 90, bottom: 120, left: 340, right: 300 }, maxZoom: 13, duration: 700 });
}

// --------------------------------------------------------------- panel ----

function fmt(v, digits = 1, unit = '') {
  if (v == null) return '—';
  return `${Number(v).toFixed(digits)}${unit}`;
}

function renderList() {
  const list = el('track-list');
  const rows = [...state.tracks.values()].sort((a, b) => a.label.localeCompare(b.label, undefined, { numeric: true }));
  el('empty').hidden = rows.length > 0;
  list.innerHTML = rows
    .map((t) => {
      const age = ageOf(t);
      const kn = t.speed_mps == null ? '—' : `${(t.speed_mps * 1.94384).toFixed(0)} kn`;
      const crs = t.course_deg == null ? '—' : `${String(Math.round(t.course_deg)).padStart(3, '0')}°`;
      return `<li data-id="${t.id}" class="${state.selected === t.id ? 'sel' : ''}">
        <span class="swatch" style="background:${colorFor(t)}"></span>
        <span><span class="id">${t.label}${t.synthetic ? ' <em class="sim">SIM</em>' : ''}</span><br><span class="sub">${crs} · ${kn} · ${t.dimension}</span></span>
        <span class="age">${age < 1 ? 'now' : `${age.toFixed(0)}s`}</span>
      </li>`;
    })
    .join('');
  for (const li of list.children) li.onclick = () => select(li.dataset.id, true);
}

function select(id, fly) {
  state.selected = state.selected === id ? null : id;
  const t = state.tracks.get(id);
  if (fly && mapReady && state.selected && t) map.easeTo({ center: [t.lon, t.lat], duration: 600 });
  render();
}

function renderDetail() {
  const box = el('detail');
  const t = state.selected ? state.tracks.get(state.selected) : null;
  if (!t) {
    box.hidden = true;
    return;
  }
  box.hidden = false;
  el('d-title').textContent = `Track ${t.label}`;
  const rows = [
    ['position', `${t.lat.toFixed(5)}, ${t.lon.toFixed(5)}`],
    ['altitude', fmt(t.alt_m, 1, ' m HAE')],
    ['course', t.course_deg == null ? '—' : `${String(Math.round(t.course_deg)).padStart(3, '0')}°`],
    ['speed', t.speed_mps == null ? '—' : `${t.speed_mps.toFixed(1)} m/s · ${(t.speed_mps * 1.94384).toFixed(1)} kn`],
    ['affiliation', t.affiliation],
    ['dimension', t.dimension],
    ['message', t.kind],
    ['cot type', t.cot_type || '—'],
    ['error (ce/le)', `${fmt(t.ce_m, 0, ' m')} / ${fmt(t.le_m, 0, ' m')}`],
    ['updates', String(t.updates)],
    ['observed', `${ageOf(t).toFixed(0)}s ago`],
    ['sensor clock', t.reported_at || '—'],
    ['source', `${t.source.sensor} @ ${t.source.site}`],
    ['producer', t.org || `${(t.producer_anchor || '').slice(0, 8)}… (unverified)`],
    ['data', t.synthetic ? 'SYNTHETIC — simulated, not a real detection' : 'real detection'],
    ['descriptor', t.descriptor || '—'],
  ];
  el('d-body').innerHTML = rows.map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`).join('');
}

function renderLegend() {
  const seen = new Map();
  for (const t of state.tracks.values()) seen.set(t.affiliation, colorFor(t));
  if (seen.size === 0) seen.set('unknown', AFF_COLOR.unknown);
  el('legend-keys').innerHTML = [...seen]
    .map(([k, c]) => `<span class="key"><i style="background:${c}"></i>${k}</span>`)
    .join('');
}

// The map must never imply it is showing everything. Two ways it can lie:
// simulated tracks read as real, and producers present on the fabric that this
// viewer is not seeing. Both are stated, not hidden.
// Non-positional sensor status (jamming, mode, health). These have no dot on
// the map by nature, so if they are not surfaced here they are invisible, in a
// "dropped" counter nobody reads.
// A status row is a READOUT, never a control, and it must carry its own age.
// The label alone ("SENSOR STATUS — Jamming Status") is a category, not a state:
// it does not say whether jamming is happening or when it was last reported, and
// a three-hour-old alert would read the same as one from a second ago.
//
// A sensor's CoT may carry no value beyond the type code, so the label can be
// all the sensor said. That is why the AGE has to be shown — it is the only thing
// distinguishing a live alert from a stale one.
function setStatusBanner(stats) {
  var e = el('sensor-status');
  if (!e) return;
  var rows = (stats && stats.statuses) || [];
  if (!rows.length) { e.hidden = true; return; }
  e.hidden = false;

  var now = Date.now();
  var freshest = Infinity;
  var parts = rows.map(function (r) {
    var age = r.observed_at ? (now - Date.parse(r.observed_at)) / 1000 : null;
    if (age !== null && age < freshest) freshest = age;
    return r.label + (age === null ? ' (no timestamp)' : ' · ' + fmtAge(age));
  });

  // Same 120 s TTL a track gets. Past it the row is history, and it stops
  // wearing the alarm colour — otherwise the map cries wolf indefinitely
  // after a sensor goes quiet.
  var stale = !(freshest < 120);
  e.className = 'readout sensor' + (stale ? ' stale' : ' live');
  e.textContent = (stale ? 'LAST SENSOR ALERT' : 'SENSOR ALERT') + ' — ' + parts.join(' · ')
    + (stale ? ' — not current' : '');
  e.title = rows.map(function (r) {
    return r.label + ' from ' + (r.source && r.source.sensor || '?')
      + ' (' + (r.affiliation || 'unknown') + ')'
      + (r.observed_at ? ', last seen ' + r.observed_at : '');
  }).join('\n');
}

// Compact age. Deliberately coarse: the decision this supports is "is this
// now or is this history", not stopwatch accuracy.
function fmtAge(s) {
  if (s < 60) return Math.max(0, Math.round(s)) + 's ago';
  if (s < 3600) return Math.round(s / 60) + 'm ago';
  if (s < 86400) return (s / 3600).toFixed(1) + 'h ago';
  return Math.round(s / 86400) + 'd ago';
}

function setProvenanceBanner(stats) {
  const synth = [...state.tracks.values()].filter((t) => t.synthetic).length;
  const unverified = (stats && stats.unverified_producers) || [];
  const bits = [];
  if (synth) bits.push(`${synth} SYNTHETIC track${synth === 1 ? '' : 's'} shown (hollow, dashed, SIM-tagged)`);
  if (unverified.length) bits.push(`${unverified.length} unverified producer${unverified.length === 1 ? '' : 's'}`);
  const reg = stats && stats.registry;
  if (reg && reg.enabled && reg.error) bits.push('producer registry stale — labels may be out of date');
  if (reg && !reg.enabled) bits.push('producer registry not configured — provenance unverified');
  const el2 = el('provenance');
  el2.hidden = bits.length === 0;
  el2.textContent = bits.join(' · ');
}

// How many tracks the CURRENT view is actually showing. null until a query has
// run. Kept here rather than in lasso.js because the header is app.js's, and a
// second module writing the same DOM node is how the two drift apart.
var shownTracks = null;
function setShownTracks(n) { shownTracks = n; }

function setStats(s) {
  // The header must describe THIS VIEW, not the estate, or it contradicts the
  // map — the same failure the received-counter comment below records. With
  // the lasso in play the store can hold six tracks while the drawn area holds
  // none, and a bare "6" over an empty map reads as a broken surface. Show
  // shown-of-total so both numbers are true and the gap is legible.
  el('stat-tracks').textContent =
    shownTracks === null ? s.tracks : shownTracks + ' / ' + s.tracks;
  // Count the leg actually feeding the picture. Showing the ingest counter
  // while the store is fed from the fabric read "0 RECEIVED" over six live
  // tracks — a header that contradicts the map teaches people to distrust it.
  const fromFabric = s.store_source === 'fabric';
  const received = fromFabric ? s.fabric_messages : s.messages;
  // Volume counters are withheld from scoped viewers server-side (they are
  // denominators — they describe the whole estate, not the viewer's slice).
  // Render the absence as an em dash rather than letting `undefined` reach
  // the DOM: a header reading "NaN" is how a correctly-scoped view gets
  // reported as a broken one.
  const has = (v) => v !== undefined && v !== null;
  el('stat-rate').textContent = fromFabric || !has(s.msgs_per_s) ? '—' : s.msgs_per_s;
  el('stat-msgs').textContent = has(received) ? received.toLocaleString() : '—';
  setProvenanceBanner(s);
  setStatusBanner(s);
}

// The picture is incomplete and says so. `partial` is a boolean at every tier;
// the count of what is hidden rides only at `full`, because the number of
// tracks you cannot see is itself a measure of the estate you cannot see.
function setScopeBanner(d) {
  const e = el('scope-banner');
  if (!e) return;
  if (!d || !d.partial) { e.hidden = true; return; }
  e.hidden = false;
  e.textContent = d.withheld === undefined
    ? 'PARTIAL VIEW — showing only tracks your access covers'
    : `PARTIAL VIEW — ${d.withheld} track${d.withheld === 1 ? '' : 's'} withheld`;
}

function setLink(up, text) {
  const box = el('link');
  box.className = `link ${up ? 'link-up' : 'link-down'}`;
  el('link-text').textContent = text;
}

// ---------------------------------------------------------------- feed ----

function connect() {
  const es = new EventSource('api/stream');

  es.addEventListener('open', () => setLink(true, 'live'));

  es.addEventListener('snapshot', (e) => {
    const d = JSON.parse(e.data);
    // DO NOT repopulate the map from the stream. The page is blank until the
    // operator draws a lasso; a snapshot that quietly filled it back in would
    // defeat both the readability goal and the "transfer nothing by default"
    // property. Stats and banners still update — those are counters, not the
    // picture.
    setStats(d.stats);
    setScopeBanner(d);
    setLink(true, 'live');
    renderLegend();
    render();
  });

  es.addEventListener('upsert', (e) => {
    // Live updates apply ONLY to tracks the current lasso already returned.
    // A new contact outside the drawn area must not appear unasked — the
    // lasso is a query, and its result set is the answer to that query.
    const t = JSON.parse(e.data);
    if (state.tracks.has(t.id)) state.tracks.set(t.id, t);
  });

  es.addEventListener('expire', (e) => {
    const { id } = JSON.parse(e.data);
    state.tracks.delete(id);
    if (state.selected === id) state.selected = null;
  });

  es.addEventListener('stats', (e) => {
    setStats(JSON.parse(e.data));
    renderLegend();
  });

  es.addEventListener('error', () => setLink(false, 'reconnecting'));
}

async function boot() {
  try {
    const meta = await (await fetch('api/meta')).json();
    state.meta = meta;
    el('source-label').textContent = `${meta.source.sensor} · ${meta.source.site} · ${meta.source.node}`;
    el('clock-note').textContent =
      `Ages are measured from receipt at this service, not from the sensor clock — a sensor feed's own ` +
      `timestamps may not be trustworthy. A track is dropped after ${meta.ttl_s}s without an update. ` +
      `Normalised form republished on ${meta.publish_prefix || '(republish disabled)'}.`;
  } catch (err) {
    el('source-label').textContent = 'metadata unavailable';
  }

  try {
    initMap(await loadStyle());
  } catch (err) {
    // No basemap is a degraded map, not a dead page -- the track list, the
    // detail card and the counters all still work.
    console.error('basemap failed to load', err);
    setLink(false, 'basemap failed');
  }

  connect();
  renderLegend();
  // Redraw on a timer so fade and age stay honest between messages.
  setInterval(render, 1000);
}

el('detail-close').onclick = () => select(state.selected, false);
el('follow').onclick = () => fitAll();

boot();
