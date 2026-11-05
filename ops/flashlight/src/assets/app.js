// SPDX-License-Identifier: Apache-2.0
//
// flashlight Tier 1 client — vanilla JS, no framework dependency.
// Opens an EventSource against /events with the current page's query
// string, renders incoming SampleEvents into the live table + the
// Canvas 2D sparkline, updates the connection-status pill on
// readyState changes. Per docs/design/flashlight.md §"Tier 1
// surfaces" + docs/design/minimalism-under-network-constraints.md.

(function () {
  'use strict';

  // ----- Config + state -------------------------------------------------
  // Display-only lexicon — the participant noun in goatnet-appropriate
  // wording, injected server-side as window.GOAT_LEX (see render.rs).
  // Render-time labels only; the `vendor`/`key` filter kind + `?vendor=`
  // param stay literal. Defaults to nova-a wording when absent.
  const LEX = (window.GOAT_LEX && window.GOAT_LEX.org)
    ? window.GOAT_LEX
    : { org: 'vendor', orgs: 'vendors' };
  const MAX_ROWS = clamp(parseInt(query('n'), 10) || 100, 1, 500);
  const SPARK_WINDOW_SEC = 60;
  const SPARK_BUCKET_MS = 1000;
  // Throttle for the auto-following payload panel — render the newest
  // event at most this often (each render fetches + may decode). Declared
  // here (not in the viewer block) so it's initialized before
  // enterTailMode() → startPanelLoop() runs at boot (avoids a TDZ error).
  const PANEL_REFRESH_MS = 600;

  const state = {
    paused: false,
    rows: [], // newest first
    rateBuckets: new Array(SPARK_WINDOW_SEC).fill(0),
    rateBucketStartMs: bucketStart(Date.now()),
    lagged: 0,
    // Payload viewer (pinned top). `following` true → the panel tracks
    // the newest event live; false (paused / a row clicked) → it stays
    // on `viewSeq`. `pendingSeq` is the newest seq awaiting a throttled
    // render in following mode (avoids decoding every event). `sampleView`
    // is the last fetched payload (raw + decoded; both panes render it).
    following: true,
    viewSeq: null,
    pendingSeq: null,
    sampleView: null,
    // Single-flight latch + monotonic request token for the payload-panel
    // fetch. `panelInFlight` keeps the auto-follow loop from stacking a
    // second `/api/sample` fetch over an unresolved one (on a slow link a
    // fetch outlasts the refresh interval); `panelReq` decides which
    // resolved fetch is allowed to paint — the most recent one requested,
    // even if newer samples landed during its round-trip.
    panelInFlight: false,
    panelReq: 0,
    // Navigate-anywhere catalog (GET /api/directory), cached for the
    // jump box's vendor-vs-key resolution + the browse list.
    directory: null,
    // Flat search index for the jump box (built from the directory): each
    // entry is {value, kind, label, hay} where `hay` is the lowercased
    // haystack matched against (value + peer id/handle aliases).
    jumpIndex: [],
    // Current rendered suggestions + which one is keyboard-highlighted.
    jumpMatches: [],
    jumpActive: -1,
  };

  // ----- DOM handles ---------------------------------------------------
  const dom = {
    focusEcho: document.getElementById('focus-echo'),
    statusPill: document.getElementById('status-pill'),
    tbody: document.getElementById('events-tbody'),
    countLabel: document.getElementById('count-label'),
    pauseBtn: document.getElementById('pause-btn'),
    clearBtn: document.getElementById('clear-btn'),
    sparkCanvas: document.getElementById('sparkline'),
    sparkCtx: null,
    detail: document.getElementById('sample-detail'),
    detailTitle: document.getElementById('sample-detail-title'),
    detailMeta: document.getElementById('sample-detail-meta'),
    detailClose: document.getElementById('sample-detail-close'),
    decodedPane: document.getElementById('sample-decoded-pane'),
    decodedLabel: document.getElementById('sample-decoded-label'),
    decoded: document.getElementById('sample-decoded'),
    rawLabel: document.getElementById('sample-raw-label'),
    raw: document.getElementById('sample-raw'),
    jumpForm: document.getElementById('jump-form'),
    jumpInput: document.getElementById('jump-input'),
    jumpResults: document.getElementById('jump-results'),
    directory: document.getElementById('directory'),
    directoryBody: document.getElementById('directory-body'),
    directorySub: document.getElementById('directory-sub'),
    sparklineWrap: document.querySelector('.sparkline-wrap'),
    eventsTable: document.querySelector('.events-table'),
  };
  if (dom.sparkCanvas) {
    dom.sparkCtx = dom.sparkCanvas.getContext('2d');
    sizeCanvas();
    window.addEventListener('resize', sizeCanvas);
  }

  // ----- Focus echo ---------------------------------------------------
  renderFocusEcho();

  // ----- Buttons ------------------------------------------------------
  // Pause freezes the tail AND the payload panel: paused → not following
  // (inspect a frozen row); running → following the newest event live.
  if (dom.pauseBtn) {
    dom.pauseBtn.addEventListener('click', () => setPaused(!state.paused));
  }
  if (dom.clearBtn) {
    dom.clearBtn.addEventListener('click', () => {
      state.rows.length = 0;
      state.lagged = 0;
      renderTable();
    });
  }

  function setPaused(paused) {
    state.paused = paused;
    state.following = !paused;
    if (dom.pauseBtn) {
      dom.pauseBtn.textContent = paused ? '▶ resume' : '⏸ pause';
      dom.pauseBtn.classList.toggle('paused', paused);
    }
    // Resuming → snap the panel back to the newest event immediately.
    if (state.following && state.rows.length) {
      state.pendingSeq = state.rows[0].seq;
      refreshPanel();
    }
    updatePanelStatus();
  }

  // ----- Payload viewer wiring ----------------------------------------
  // Clicking a row pauses the stream and pins the panel to that event
  // (you're inspecting a specific frozen sample). Resume (pause button,
  // or the panel's ↺ live) goes back to auto-following the newest.
  if (dom.tbody) {
    dom.tbody.addEventListener('click', (ev) => {
      const tr = ev.target.closest('tr.ev');
      if (!tr || !tr.dataset.seq) return;
      const seq = parseInt(tr.dataset.seq, 10);
      setPaused(true);
      showSeq(seq);
    });
  }
  if (dom.detailClose) {
    // ↺ live — resume following the newest event.
    dom.detailClose.addEventListener('click', () => setPaused(false));
  }

  // ----- Keyboard: "/" focuses the jump box (like panoscope) ----------
  document.addEventListener('keydown', (ev) => {
    if (ev.key !== '/' || ev.metaKey || ev.ctrlKey || ev.altKey) return;
    if (isTypingTarget(ev.target)) return; // don't hijack while typing
    if (!dom.jumpInput) return;
    ev.preventDefault();
    dom.jumpInput.focus();
    dom.jumpInput.select();
  });

  function isTypingTarget(el) {
    if (!el) return false;
    const tag = (el.tagName || '').toLowerCase();
    return tag === 'input' || tag === 'textarea' || el.isContentEditable === true;
  }

  // ----- Mode: directory (no focus) vs live tail (focus declared) -----
  // Empty focus lands on the navigate-anywhere directory instead of an
  // empty event tail — never "subscribed to everything" (design doc).
  const EMPTY_FOCUS = isEmptyFocus();

  // The jump box + directory catalog load on every view. The catalog
  // feeds the header datalist (type-ahead) always, and renders the
  // browse list when the focus is empty.
  initNavigation();

  // ----- EventSource --------------------------------------------------
  const sseUrl = '/events' + (window.location.search || '');
  let es = null;
  if (EMPTY_FOCUS) {
    enterDirectoryMode();
  } else {
    enterTailMode();
    connect();
  }

  // Focused view: reveal the pinned payload panel (auto-following the
  // newest event) and start the throttled refresh loop.
  function enterTailMode() {
    if (dom.detail) dom.detail.removeAttribute('hidden');
    updatePanelStatus();
    startPanelLoop();
  }

  function isEmptyFocus() {
    const q = window.location.search;
    return !q || q === '?';
  }

  // Hide the live-tail surfaces and show the directory; suppress the
  // status pill (no subscription in directory mode).
  function enterDirectoryMode() {
    if (dom.directory) dom.directory.removeAttribute('hidden');
    if (dom.sparklineWrap) dom.sparklineWrap.setAttribute('hidden', '');
    if (dom.eventsTable) dom.eventsTable.setAttribute('hidden', '');
    if (dom.statusPill) {
      dom.statusPill.className = 'status-pill';
      dom.statusPill.textContent = 'browse';
    }
  }

  // Fetch the catalog once; build the jump-box search index and (in
  // directory mode) render the browse list. Failure degrades to the
  // static fallback already in the DOM — the URL bar still works.
  function initNavigation() {
    wireJumpBox();
    fetch('/api/directory')
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error('http ' + r.status))))
      .then((dir) => {
        state.directory = dir;
        buildJumpIndex(dir);
        // Re-render the focus echo now that the directory is loaded, so a
        // vendor anchor resolves to its real name (it first paints from the
        // URL alone, before the catalog arrives).
        renderFocusEcho();
        if (EMPTY_FOCUS) renderDirectory(dir);
      })
      .catch(() => {
        if (EMPTY_FOCUS) renderDirectoryError();
      });
  }

  function wireJumpBox() {
    if (!dom.jumpForm || !dom.jumpInput) return;

    // Submit (Enter / form submit) navigates to the highlighted match,
    // else the top match, else the raw text typed.
    dom.jumpForm.addEventListener('submit', (ev) => {
      ev.preventDefault();
      const chosen = chosenJumpValue();
      if (chosen) window.location.assign(jumpTarget(chosen));
    });

    // Type-ahead: re-filter on every keystroke (substring, both
    // directions, over key + vendor + peer id).
    dom.jumpInput.addEventListener('input', () => {
      refreshJumpMatches(dom.jumpInput.value);
    });

    // Keyboard: arrows move the highlight, Escape closes the dropdown.
    dom.jumpInput.addEventListener('keydown', (ev) => {
      if (!state.jumpMatches.length) return;
      if (ev.key === 'ArrowDown') {
        ev.preventDefault();
        setJumpActive(state.jumpActive + 1);
      } else if (ev.key === 'ArrowUp') {
        ev.preventDefault();
        setJumpActive(state.jumpActive - 1);
      } else if (ev.key === 'Escape') {
        closeJumpResults();
      }
    });

    // Re-open on focus if there's text; close on blur (deferred so a
    // result mousedown registers first).
    dom.jumpInput.addEventListener('focus', () => refreshJumpMatches(dom.jumpInput.value));
    dom.jumpInput.addEventListener('blur', () => window.setTimeout(closeJumpResults, 150));

    // mousedown (not click) so it fires before the input's blur.
    if (dom.jumpResults) {
      dom.jumpResults.addEventListener('mousedown', (ev) => {
        const li = ev.target.closest('li.jump-result');
        if (!li) return;
        ev.preventDefault();
        window.location.assign(jumpTarget(li.getAttribute('data-value')));
      });
    }
  }

  // Resolve a jump-box entry to a focus URL: an exact vendor-prefix
  // match tails the whole vendor; anything else is treated as a key.
  function jumpTarget(raw) {
    const vendors = (state.directory && state.directory.vendors) || [];
    const isVendor = vendors.some((v) => v.prefix === raw);
    const param = isVendor ? 'vendor' : 'key';
    return '/?' + param + '=' + encodeURIComponent(raw);
  }

  // The value Enter should navigate to: the highlighted suggestion, else
  // the top suggestion, else the raw trimmed text (so an exact key the
  // operator pastes still works even if it isn't in the catalog).
  function chosenJumpValue() {
    if (state.jumpActive >= 0 && state.jumpMatches[state.jumpActive]) {
      return state.jumpMatches[state.jumpActive].value;
    }
    if (state.jumpMatches.length) return state.jumpMatches[0].value;
    return (dom.jumpInput && dom.jumpInput.value || '').trim();
  }

  // Build the flat search index from the catalog. Each entry carries the
  // navigable `value`, a `kind` (vendor/key/peer), and a lowercased `hay`
  // haystack that includes peer id + handle aliases so typing a peer id
  // surfaces that peer's keys.
  function buildJumpIndex(dir) {
    const idx = [];
    const seen = Object.create(null);
    const add = (value, kind, alias, label) => {
      if (!value) return;
      const dedupe = kind + ' ' + value;
      if (seen[dedupe]) return;
      seen[dedupe] = true;
      // `value` is the canonical navigate id; `label` is the friendly name
      // shown in the dropdown (the org display_name), fallback to value.
      idx.push({ value: value, kind: kind, label: label || value, hay: (value + ' ' + (alias || '')).toLowerCase() });
    };
    ((dir && dir.vendors) || []).forEach((v) => {
      add(v.prefix, 'vendor', v.display_name, v.display_name || v.prefix);
      (v.topics || []).forEach((t) => add(t.key_pattern, 'key'));
    });
    ((dir && dir.peers) || []).forEach((p) => {
      const alias = [p.peer_id, p.display_name, p.vendor, p.site].filter(Boolean).join(' ');
      (p.topics || []).forEach((k) => add(k, 'key', alias));
    });
    state.jumpIndex = idx;
  }

  // Case-insensitive substring match (matches anywhere in the haystack —
  // peer-id prefix or any middle segment of a key). Ranked by match
  // position, then value length, then lexicographic. Capped to keep the
  // dropdown lean per the minimalism doctrine.
  function jumpFilter(query) {
    const q = (query || '').trim().toLowerCase();
    if (!q) return [];
    const hits = [];
    for (let i = 0; i < state.jumpIndex.length; i++) {
      const e = state.jumpIndex[i];
      const at = e.hay.indexOf(q);
      if (at >= 0) hits.push({ e: e, at: at });
    }
    hits.sort(
      (a, b) =>
        a.at - b.at ||
        a.e.value.length - b.e.value.length ||
        (a.e.value < b.e.value ? -1 : 1)
    );
    return hits.slice(0, 50).map((h) => h.e);
  }

  function refreshJumpMatches(query) {
    state.jumpMatches = jumpFilter(query);
    state.jumpActive = state.jumpMatches.length ? 0 : -1;
    renderJumpResults((query || '').trim().toLowerCase());
  }

  function renderJumpResults(q) {
    if (!dom.jumpResults) return;
    if (!state.jumpMatches.length) {
      closeJumpResults();
      return;
    }
    dom.jumpResults.innerHTML = state.jumpMatches
      .map(
        (e, i) =>
          '<li class="jump-result' +
          (i === state.jumpActive ? ' active' : '') +
          '" role="option" aria-selected="' +
          (i === state.jumpActive) +
          '" data-value="' +
          escapeAttr(e.value) +
          '"><span class="jump-kind">' +
          escapeHtml(e.kind === 'vendor' ? LEX.org : e.kind) +
          '</span><span class="jump-val">' +
          highlightMatch(e.label || e.value, q) +
          '</span></li>'
      )
      .join('');
    dom.jumpResults.hidden = false;
    if (dom.jumpForm) dom.jumpForm.setAttribute('aria-expanded', 'true');
  }

  function setJumpActive(i) {
    const n = state.jumpMatches.length;
    if (!n) return;
    state.jumpActive = ((i % n) + n) % n; // wrap both directions
    const items = dom.jumpResults ? dom.jumpResults.children : [];
    for (let k = 0; k < items.length; k++) {
      const on = k === state.jumpActive;
      items[k].classList.toggle('active', on);
      items[k].setAttribute('aria-selected', on);
      if (on && items[k].scrollIntoView) items[k].scrollIntoView({ block: 'nearest' });
    }
  }

  function closeJumpResults() {
    if (dom.jumpResults) {
      dom.jumpResults.hidden = true;
      dom.jumpResults.innerHTML = '';
    }
    state.jumpMatches = [];
    state.jumpActive = -1;
    if (dom.jumpForm) dom.jumpForm.setAttribute('aria-expanded', 'false');
  }

  // Wrap the matched substring of `value` in <mark>. The match position is
  // re-derived against the value (not the alias-augmented haystack), so a
  // peer-id-only match simply renders the value unmarked — still listed.
  function highlightMatch(value, q) {
    const at = value.toLowerCase().indexOf(q);
    if (at < 0 || !q) return escapeHtml(value);
    return (
      escapeHtml(value.slice(0, at)) +
      '<mark>' +
      escapeHtml(value.slice(at, at + q.length)) +
      '</mark>' +
      escapeHtml(value.slice(at + q.length))
    );
  }

  function renderDirectory(dir) {
    if (!dom.directoryBody) return;
    const peers = dir.peers || [];
    const parts = [];

    // --- Vendors / groups (from the portal) ---
    if (!dir.enabled) {
      parts.push(
        '<p class="empty-state">portal not configured — browse by role below, type a Zenoh key in the jump box, or use the URL: <a class="help-link" href="/?key=notional/**">?key=notional/**</a></p>'
      );
    } else if (!dir.vendors.length) {
      parts.push('<p class="empty-state">no ' + LEX.orgs + ' registered in the portal yet.</p>');
    } else {
      parts.push('<div class="dir-section-label">' + LEX.orgs + ' / groups</div>');
      parts.push(
        '<div class="directory-grid">' +
          dir.vendors.map(vendorCard).join('') +
          '</div>'
      );
    }

    // --- Peers grouped by role (from the schema-decl topology) ---
    parts.push(renderRoleGroups(peers));

    dom.directoryBody.innerHTML = parts.join('');

    if (dom.directorySub) {
      const nTopics = dir.enabled
        ? dir.vendors.reduce((acc, v) => acc + v.topic_count, 0)
        : 0;
      const bits = [];
      if (dir.enabled) bits.push(dir.vendors.length + ' ' + LEX.org + '(s) · ' + nTopics + ' topic(s)');
      if (peers.length) bits.push(peers.length + ' peer(s)');
      dom.directorySub.textContent =
        (bits.join(' · ') || 'nothing observed yet') + ' — click to start tailing';
    }
  }

  function vendorCard(v) {
    const topics = v.topics.map((t) => topicRow(t.key_pattern, t.format, t.description, t.publish_count)).join('');
    return (
      '<div class="dir-vendor"><div class="dir-vendor-head">' +
      '<a class="dir-vendor-name" href="/?vendor=' +
      encodeURIComponent(v.prefix) +
      '" title="Tail everything from ' +
      escapeHtml(v.prefix) +
      '">' +
      escapeHtml(v.display_name) +
      ' ↗</a><span class="dir-vendor-count">' +
      v.topic_count +
      ' topic(s)</span></div><ul class="dir-topics">' +
      (topics || '<li class="empty-state">no topics declared</li>') +
      '</ul></div>'
    );
  }

  // A single tailable topic row (shared by vendor cards + peer cards).
  function topicRow(key, format, description, publishCount) {
    const fmt = fmtIcon(formatToHint(format));
    const desc = description ? ' — ' + description : '';
    const pub = publishCount ? ' · ' + publishCount + ' pub' : '';
    return (
      '<li><a class="dir-topic" href="/?key=' +
      encodeURIComponent(key) +
      '" title="Tail ' +
      escapeHtml(key) +
      '"><span class="dir-fmt" title="' +
      escapeHtml(fmt.label) +
      '">' +
      fmt.glyph +
      '</span><span class="dir-key">' +
      escapeHtml(key) +
      '</span><span class="dir-meta">' +
      escapeHtml((format || '') + desc + pub) +
      '</span></a></li>'
    );
  }

  // Render peers grouped Sources → Derivers → Sinks. Unknown-role peers
  // (no declared pub/sub) are noise and omitted. Each peer card lists
  // its tailable keys. Returns '' when there are no role-bearing peers.
  function renderRoleGroups(peers) {
    if (!peers.length) return '';
    const groups = [
      { role: 'source', label: 'sources', hint: 'publish-only peers' },
      { role: 'deriver', label: 'derivers', hint: 'consume + publish derived' },
      { role: 'sink', label: 'sinks', hint: 'subscribe-only peers' },
    ];
    const out = [];
    groups.forEach((g) => {
      const inGroup = peers.filter((p) => p.role === g.role);
      if (!inGroup.length) return;
      out.push(
        '<div class="dir-section-label" title="' +
          escapeHtml(g.hint) +
          '">' +
          g.label +
          ' <span class="dir-section-count">(' +
          inGroup.length +
          ')</span></div>'
      );
      out.push('<div class="directory-grid">' + inGroup.map(peerCard).join('') + '</div>');
    });
    return out.join('');
  }

  function peerCard(p) {
    const name = p.display_name || p.peer_id;
    const tag = p.role_class ? p.role_class : p.role;
    const meta = [p.vendor, p.site].filter(Boolean).join(' · ');
    const topics = p.topics.map((k) => topicRow(k, '', '', 0)).join('');
    return (
      '<div class="dir-vendor"><div class="dir-vendor-head">' +
      '<span class="dir-vendor-name" title="' +
      escapeHtml(p.peer_id) +
      (meta ? ' · ' + escapeHtml(meta) : '') +
      '">' +
      escapeHtml(name) +
      '</span><span class="dir-vendor-count" title="declared role_class / computed role">' +
      escapeHtml(tag) +
      '</span></div><ul class="dir-topics">' +
      (topics || '<li class="empty-state">no keys</li>') +
      '</ul></div>'
    );
  }

  function renderDirectoryError() {
    if (!dom.directoryBody) return;
    dom.directoryBody.innerHTML =
      '<p class="empty-state">could not load the catalog. Jump to a raw key with the box above, or try <a class="help-link" href="/?vendor=notional">?vendor=notional</a></p>';
  }

  // Map a portal format string (e.g. "protobuf:Track", "json") to the
  // content-hint vocabulary the fmtIcon() glyph map understands.
  function formatToHint(format) {
    const f = (format || '').toLowerCase();
    if (f.startsWith('json')) return 'json-obj';
    if (f.startsWith('protobuf') || f.startsWith('proto')) return 'protobuf';
    if (f.startsWith('cbor')) return 'cbor';
    if (f.startsWith('text') || f === 'utf8' || f === 'utf-8') return 'text';
    return f ? 'binary' : 'empty';
  }

  function escapeAttr(s) {
    return escapeHtml(s);
  }

  function connect() {
    setStatus('reconnecting', 'connecting');
    try {
      es = new EventSource(sseUrl);
    } catch (e) {
      setStatus('error', 'error');
      return;
    }
    es.addEventListener('open', () => setStatus('live', 'live'));
    es.addEventListener('sample', (ev) => {
      if (state.paused) return;
      let payload;
      try {
        payload = JSON.parse(ev.data);
      } catch (e) {
        return;
      }
      pushRow(payload, /* lagged= */ false);
      bumpRate();
      renderTable();
      drawSparkline();
      // Auto-follow: mark the newest seq for the throttled panel refresh.
      if (state.following && payload.seq != null) {
        state.pendingSeq = payload.seq;
      }
    });
    es.addEventListener('lagged', (ev) => {
      let payload;
      try {
        payload = JSON.parse(ev.data);
      } catch (e) {
        payload = { missed: '?' };
      }
      state.lagged += payload.missed || 0;
      pushRow(
        {
          key: '(lagged · missed ' + payload.missed + ')',
          ts_ms: Date.now(),
          payload_len: 0,
        },
        /* lagged= */ true
      );
      renderTable();
    });
    es.addEventListener('error', () => {
      // EventSource auto-reconnects; reflect that in the pill.
      const readyState = es && es.readyState;
      if (readyState === 2 /* CLOSED */) {
        setStatus('offline', 'offline');
      } else {
        setStatus('reconnecting', 'reconnecting');
      }
    });
  }

  // ----- Helpers ------------------------------------------------------
  function pushRow(sample, isLagged) {
    state.rows.unshift({ ...sample, _lagged: isLagged });
    if (state.rows.length > MAX_ROWS) {
      state.rows.length = MAX_ROWS;
    }
  }

  function bumpRate() {
    const now = Date.now();
    const startNow = bucketStart(now);
    if (startNow !== state.rateBucketStartMs) {
      // Roll the ring buffer forward to keep the time-aligned window.
      const gapBuckets = Math.min(
        SPARK_WINDOW_SEC,
        Math.floor((startNow - state.rateBucketStartMs) / SPARK_BUCKET_MS)
      );
      for (let i = 0; i < gapBuckets; i++) {
        state.rateBuckets.shift();
        state.rateBuckets.push(0);
      }
      state.rateBucketStartMs = startNow;
    }
    state.rateBuckets[state.rateBuckets.length - 1] += 1;
  }

  function renderTable() {
    if (!dom.tbody) return;
    const html = state.rows
      .map((row) => {
        const ts = formatTs(row.ts_ms);
        const len = row.payload_len === 0 ? '—' : row.payload_len + 'B';
        if (row._lagged) {
          // Lagged markers carry no seq/payload — render inert.
          return (
            '<tr class="lagged"><td class="ts">' +
            ts +
            '</td><td class="key" colspan="3">' +
            escapeHtml(row.key) +
            '</td></tr>'
          );
        }
        const selected = !state.following && row.seq === state.viewSeq ? ' selected' : '';
        const fmt = fmtIcon(row.format);
        return (
          '<tr class="ev' +
          selected +
          '" data-seq="' +
          row.seq +
          '" title="Click to inspect this message’s raw payload (seq ' +
          row.seq +
          ').">' +
          '<td class="ts">' +
          ts +
          '</td><td class="key">' +
          escapeHtml(row.key) +
          '</td><td class="fmt" title="' +
          escapeHtml(row.encoding ? fmt.label + ' · ' + row.encoding : fmt.label) +
          '">' +
          fmt.glyph +
          '</td><td class="len">' +
          len +
          '</td></tr>'
        );
      })
      .join('');
    dom.tbody.innerHTML = html;
    if (dom.countLabel) {
      const tail = state.lagged > 0 ? ' · ' + state.lagged + ' missed' : '';
      dom.countLabel.textContent =
        'events · showing ' + state.rows.length + ' of ' + MAX_ROWS + tail;
    }
  }

  // Map the server's content-classified format hint to the panoscope
  // icon set. Authoritative wire format (protobuf message name etc.)
  // comes from the portal registry in the Tier 2 decode view; this is
  // the cheap content heuristic for the at-a-glance column.
  function fmtIcon(format) {
    switch (format) {
      case 'json-obj':
        return { glyph: '{ }', label: 'JSON object' };
      case 'json-arr':
        return { glyph: '[*]', label: 'JSON array' };
      case 'text':
        return { glyph: 'txt', label: 'text' };
      case 'protobuf':
        return { glyph: 'pb', label: 'protobuf (from wire encoding)' };
      case 'cbor':
        return { glyph: 'cb', label: 'CBOR (from wire encoding)' };
      case 'binary':
        return { glyph: '0x', label: 'binary' };
      case 'empty':
        return { glyph: '·', label: 'empty payload' };
      default:
        return { glyph: '?', label: 'unknown' };
    }
  }

  // ----- Payload viewer (pinned top, auto-following) ------------------
  // Throttled refresh loop: in following mode, render the newest event's
  // payload at most ~every PANEL_REFRESH_MS (declared in the config block
  // above so it's initialized before this runs at boot). Paused mode
  // renders only on explicit row-click / toggle.
  function startPanelLoop() {
    setInterval(() => {
      if (!state.following) return;
      // Single-flight: never stack a second auto-follow fetch on top of
      // one already in flight. On a slow link a sample fetch round-trips
      // slower than PANEL_REFRESH_MS; without this guard the loop fires a
      // new fetch every tick, the in-flight ones pile up against the
      // browser's per-host connection cap, and (with the old viewSeq guard)
      // each one was discarded on arrival because a newer seq had already
      // landed — so a fast topic painted nothing until traffic lulled.
      if (state.panelInFlight) return;
      if (state.pendingSeq != null && state.pendingSeq !== state.viewSeq) {
        refreshPanel();
      }
    }, PANEL_REFRESH_MS);
  }

  // Render whatever the panel should currently show: newest pending seq
  // (following) — used by the loop + on resume.
  function refreshPanel() {
    if (state.pendingSeq != null) showSeq(state.pendingSeq);
  }

  // Fetch a sample (raw + decoded) and render it into the pinned panel.
  // Render eligibility is keyed to a monotonic request token, NOT to
  // viewSeq equality: a fetch always paints the freshest sample it asked
  // for even if newer samples arrived during its round-trip (auto-follow
  // intentionally lags by one RTT rather than going blank on a fast topic).
  // A later showSeq() — the next loop tick once this one lands, or a row
  // click — bumps panelReq and supersedes us.
  function showSeq(seq) {
    const req = ++state.panelReq;
    state.viewSeq = seq;
    state.panelInFlight = true;
    renderTable(); // reflect the selection highlight
    updatePanelStatus();
    fetch('/api/sample/' + seq)
      .then((r) => {
        if (r.status === 404) throw new Error('expired');
        if (!r.ok) throw new Error('http ' + r.status);
        return r.json();
      })
      .then((view) => {
        if (req !== state.panelReq) return; // a newer request superseded us
        state.sampleView = view;
        renderSampleDetail();
      })
      .catch((err) => {
        if (req !== state.panelReq) return;
        if (dom.decodedPane) dom.decodedPane.hidden = true;
        if (dom.raw) {
          dom.raw.textContent =
            err && err.message === 'expired'
              ? 'sample expired — scrolled out of the server-side ring'
              : 'could not load sample (' + (err && err.message) + ')';
        }
      })
      .finally(() => {
        // Clear the in-flight latch only if no newer request superseded us
        // (an explicit row click can bump panelReq mid-flight).
        if (req === state.panelReq) state.panelInFlight = false;
      });
  }

  // Fill both panes: the deserialized message AND the serialized bytes.
  // Decoded pane = protobuf→JSON when available, else UTF-8 text for
  // texty topics, else hidden (the meta line carries the decode_note
  // explaining why). Raw pane = always the on-wire bytes as a hexdump.
  function renderSampleDetail() {
    const view = state.sampleView;
    if (!view) return;
    if (dom.decodedPane && dom.decoded && dom.decodedLabel) {
      if (view.decoded != null) {
        // decoded_label distinguishes a true schema decode from the
        // schema-less structural fallback (field numbers, not names).
        dom.decodedLabel.textContent = view.decoded_label || 'decoded';
        dom.decoded.textContent = view.decoded;
        dom.decodedPane.hidden = false;
      } else if (view.text != null) {
        dom.decodedLabel.textContent = 'text';
        dom.decoded.textContent = view.text;
        dom.decodedPane.hidden = false;
      } else {
        dom.decodedPane.hidden = true;
      }
    }
    if (dom.raw) dom.raw.textContent = formatHex(view.hex);
    if (dom.rawLabel) {
      dom.rawLabel.textContent =
        'raw · hex · ' + (view.truncated ? 'first ' + view.preview_len + ' of ' : '') + view.payload_len + ' B';
    }
    updatePanelStatus(view);
  }

  // Title shows live-vs-paused; meta shows seq + key + size + decode note.
  function updatePanelStatus(view) {
    view = view || state.sampleView;
    if (dom.detailTitle) {
      dom.detailTitle.textContent = state.following ? 'payload · live' : 'payload · paused';
    }
    // The ↺ live affordance only makes sense while paused.
    if (dom.detailClose) dom.detailClose.hidden = state.following;
    if (!dom.detailMeta) return;
    if (!view) {
      dom.detailMeta.textContent = state.following ? 'waiting for events…' : '';
      return;
    }
    const size = view.truncated
      ? 'first ' + view.preview_len + ' of ' + view.payload_len + ' B'
      : view.payload_len + ' B';
    const note =
      view.decode_note && view.decode_note !== 'decoded' ? ' · ' + view.decode_note : '';
    dom.detailMeta.textContent =
      'seq ' + view.seq + ' · ' + view.key + ' · ' + size + note;
  }

  // Group hex into space-separated byte pairs, 16 bytes per line, for a
  // readable hexdump-style view.
  function formatHex(hex) {
    if (!hex) return '(empty)';
    const out = [];
    for (let i = 0; i < hex.length; i += 2) {
      out.push(hex.slice(i, i + 2));
      if (out.length % 16 === 0) out.push('\n');
      else out.push(' ');
    }
    return out.join('').trimEnd();
  }

  function drawSparkline() {
    if (!dom.sparkCtx) return;
    const ctx = dom.sparkCtx;
    const w = dom.sparkCanvas.width;
    const h = dom.sparkCanvas.height;
    ctx.clearRect(0, 0, w, h);

    const max = Math.max(1, ...state.rateBuckets);
    const stepX = w / state.rateBuckets.length;

    // Bars from left (oldest) to right (newest).
    ctx.fillStyle = getCssVar('--accent', '#20964f');
    for (let i = 0; i < state.rateBuckets.length; i++) {
      const v = state.rateBuckets[i];
      const barH = (v / max) * h;
      ctx.fillRect(i * stepX, h - barH, Math.max(1, stepX - 1), barH);
    }

    // Update peak/now stats next to the sparkline label so the operator
    // reads the canvas without an explicit numeric y-axis (Block 89 UX
    // polish). `peak` is the rolling-60s maximum used for
    // canvas y-scaling above. `now` is the most-recent 1-sec bucket.
    const statsEl = document.getElementById('sparkline-stats');
    if (statsEl) {
      const peak = max;
      const now = state.rateBuckets[state.rateBuckets.length - 1] || 0;
      statsEl.textContent = 'peak ' + fmtRate(peak) + ' · now ' + fmtRate(now);
    }
  }

  // Format an events/sec value compactly: integers when ≥ 10, one
  // decimal when ≥ 0.1, two decimals when smaller. Matches the
  // sparkline label's "events / sec" units.
  function fmtRate(v) {
    if (v >= 10) return Math.round(v).toString();
    if (v >= 0.1) return v.toFixed(1);
    return v.toFixed(2);
  }

  function setStatus(cls, label) {
    if (!dom.statusPill) return;
    dom.statusPill.className = 'status-pill ' + cls;
    dom.statusPill.textContent = label;
  }

  // Map a vendor anchor (UUID slot id / handle) to its real name via the
  // loaded directory, so the focus echo reads the org name not the UUID.
  // Degrades to the raw value when the directory isn't loaded or has no match
  // (portal-down honesty, ADR 1039).
  function vendorDisplayName(v) {
    const vendors = (state.directory && state.directory.vendors) || [];
    for (let i = 0; i < vendors.length; i++) {
      if (vendors[i].prefix === v && vendors[i].display_name) return vendors[i].display_name;
    }
    return v;
  }

  function renderFocusEcho() {
    if (!dom.focusEcho) return;
    const q = window.location.search;
    if (!q || q === '?') {
      dom.focusEcho.innerHTML =
        'no slice — search for a ' + escapeHtml(LEX.org) + ', topic, or peer in the jump box above';
      return;
    }
    const parts = q
      .slice(1)
      .split('&')
      .filter((s) => s.length > 0 && s.includes('='))
      .map((s) => {
        const [k, v] = s.split('=');
        const key = decodeURIComponent(k);
        const val = decodeURIComponent(v);
        // Relex the param name to the goatnet noun (vendor → org) and resolve
        // the anchor to its real name; the URL itself stays ?vendor=<uuid>.
        const dispKey = key === 'vendor' ? LEX.org : key;
        const dispVal = key === 'vendor' ? vendorDisplayName(val) : val;
        return '<strong>' + escapeHtml(dispKey) + '</strong>=' + escapeHtml(dispVal);
      });
    dom.focusEcho.innerHTML = 'focus · ' + parts.join(' · ');
  }

  function sizeCanvas() {
    if (!dom.sparkCanvas) return;
    const dpr = window.devicePixelRatio || 1;
    const cssW = dom.sparkCanvas.clientWidth;
    const cssH = dom.sparkCanvas.clientHeight;
    dom.sparkCanvas.width = Math.floor(cssW * dpr);
    dom.sparkCanvas.height = Math.floor(cssH * dpr);
    if (dom.sparkCtx) {
      dom.sparkCtx.setTransform(dpr, 0, 0, dpr, 0, 0);
      // After transform, width/height now report CSS units to drawing
      // calls; keep w/h consistent in drawSparkline by using
      // clientWidth/Height via the canvas attrs (set above).
    }
  }

  function bucketStart(ms) {
    return Math.floor(ms / SPARK_BUCKET_MS) * SPARK_BUCKET_MS;
  }

  function formatTs(ms) {
    const d = new Date(ms);
    return (
      pad2(d.getUTCHours()) +
      ':' +
      pad2(d.getUTCMinutes()) +
      ':' +
      pad2(d.getUTCSeconds())
    );
  }

  function pad2(n) {
    return n < 10 ? '0' + n : '' + n;
  }

  function clamp(n, lo, hi) {
    return Math.max(lo, Math.min(hi, n));
  }

  function query(name) {
    const params = new URLSearchParams(window.location.search);
    return params.get(name);
  }

  function escapeHtml(s) {
    if (s == null) return '';
    return String(s)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  function getCssVar(name, fallback) {
    const v = getComputedStyle(document.documentElement).getPropertyValue(name);
    return (v && v.trim()) || fallback;
  }

  // Repaint the sparkline once per second even when no events arrive
  // so the rolling window decays visibly.
  setInterval(() => {
    bumpRollWithoutIncrement();
    drawSparkline();
  }, 1000);

  function bumpRollWithoutIncrement() {
    const now = Date.now();
    const startNow = bucketStart(now);
    if (startNow !== state.rateBucketStartMs) {
      const gapBuckets = Math.min(
        SPARK_WINDOW_SEC,
        Math.floor((startNow - state.rateBucketStartMs) / SPARK_BUCKET_MS)
      );
      for (let i = 0; i < gapBuckets; i++) {
        state.rateBuckets.shift();
        state.rateBuckets.push(0);
      }
      state.rateBucketStartMs = startNow;
    }
  }

  // ----- Tier 2 augmentation (Phase E) ----------------------------------
  // Per docs/design/flashlight.md §Tier 2: lazy-load the WASM analytics
  // module when conditions allow, OR forward events to /api/compute when
  // ?compute=server. Either way, populate the `.tier-2-only` augmentation
  // surfaces in the page from the rendered AnalyticsSnapshot.
  //
  // The boot decision is:
  //   - ?compute=server → server-side path; no WASM load.
  //   - ?compute=browser → force WASM load regardless of network hints.
  //   - default → load WASM iff effectiveType === '4g' && !saveData,
  //              i.e. the operator is on a strong connection. Otherwise
  //              stay Tier 1-only; the operator can flip the URL param.
  //
  // Tier 2 eviction is best-effort: any failure in load / push / snapshot
  // drops us back to Tier 1 silently. The Tier 1 page keeps running.

  const t2Mode = (function () {
    // No analytics on the directory landing — there's no live slice.
    if (EMPTY_FOCUS) return 'off';
    const v = (query('compute') || '').toLowerCase();
    if (v === 'server') return 'server';
    if (v === 'browser') return 'browser-forced';
    // Auto-mode gate. navigator.connection is the NetworkInformation
    // API — present in Chrome/Edge/Opera, absent in Safari/Firefox.
    // When absent, fall through to Tier 1-only.
    const c = navigator.connection;
    if (!c) return 'off';
    if (c.saveData) return 'off';
    if (c.effectiveType === '4g') return 'browser-auto';
    return 'off';
  })();

  // ?compute=server re-sends the most recent events on every snapshot
  // tick (the endpoint is stateless), so this is also the buffer bound.
  const T2_SERVER_EVENTS = 1000;

  const t2 = {
    mode: t2Mode,
    engine: null, // wasm module {push_event, snapshot_json}
    pendingEvents: [], // buffer for ?compute=server batched POSTs
    snapshotTimer: null,
  };

  if (t2.mode === 'browser-auto' || t2.mode === 'browser-forced') {
    bootTier2Browser();
  } else if (t2.mode === 'server') {
    bootTier2Server();
  }

  function bootTier2Browser() {
    // Streaming-compiled per design doc Decision 1. The `?compute=server`
    // and ?compute=browser strings come from the URL grammar; the WASM
    // bytes path is the rust-embed-served /api/wasm/ endpoint that
    // PR #598 wired up.
    import('/api/wasm/flashlight_analytics.js')
      .then((mod) => mod.default('/api/wasm/flashlight_analytics_bg.wasm').then(() => mod))
      .then((mod) => {
        t2.engine = {
          push_event: (jsonBytes) => mod.push_event(jsonBytes),
          snapshot_json: () => mod.snapshot_json(),
        };
        showT2Surfaces();
        startSnapshotLoop();
      })
      .catch((err) => {
        // Eviction path — Tier 2 unavailable; stay on Tier 1.
        console.warn('flashlight: Tier 2 WASM load failed, staying Tier 1', err);
        t2.mode = 'off';
      });
  }

  function bootTier2Server() {
    // ?compute=server — no WASM needed. Periodically POST the recent
    // event buffer to /api/compute and render the response into the
    // same augmentation surfaces. Same operator-facing badges so the
    // ?compute=browser/server URL flip swaps where compute runs
    // without changing what the operator sees.
    showT2Surfaces();
    startSnapshotLoop();
  }

  function showT2Surfaces() {
    document.querySelectorAll('.tier-2-only').forEach((el) => {
      el.removeAttribute('hidden');
    });
  }

  function hideT2Surfaces() {
    document.querySelectorAll('.tier-2-only').forEach((el) => {
      el.setAttribute('hidden', '');
    });
  }

  // Forward each SSE sample event into Tier 2. The wire shape mirrors
  // the Event struct the analytics crate's serde_json deserializer
  // expects: { key, ts_ms, payload, format }. payload comes through as
  // an array of byte values when present.
  function forwardToTier2(payload) {
    if (t2.mode === 'off') return;
    const event = {
      key: payload.key || '',
      ts_ms: payload.ts_ms || Date.now(),
      // Tier 1's SSE shape uses `payload_len` (length only, not the
      // bytes) so server-side stats can be computed without forwarding
      // payload bytes. Synthesize a zero-filled vec of that length —
      // analytics' byte-percentile path only looks at length anyway.
      payload: zerosOfLen(payload.payload_len || 0),
      format: payload.format,
    };
    const json = JSON.stringify(event);
    if (t2.mode === 'browser-auto' || t2.mode === 'browser-forced') {
      if (!t2.engine) return;
      try {
        t2.engine.push_event(textEncode(json));
      } catch (err) {
        console.warn('flashlight: Tier 2 push_event failed, evicting', err);
        evictTier2();
      }
    } else if (t2.mode === 'server') {
      // Keep only the window the next POST sends. The flush below copied
      // the last T2_SERVER_EVENTS but never trimmed the buffer itself, so
      // it grew for the life of the page.
      t2.pendingEvents.push(event);
      if (t2.pendingEvents.length > T2_SERVER_EVENTS) {
        t2.pendingEvents.splice(0, t2.pendingEvents.length - T2_SERVER_EVENTS);
      }
    }
  }

  function startSnapshotLoop() {
    // 2-second cadence matches the design doc's "augmentation refresh
    // is opportunistic — operator doesn't need sub-second" guidance.
    t2.snapshotTimer = setInterval(renderT2Snapshot, 2000);
  }

  function renderT2Snapshot() {
    if (t2.mode === 'off') {
      hideT2Surfaces();
      if (t2.snapshotTimer) {
        clearInterval(t2.snapshotTimer);
        t2.snapshotTimer = null;
      }
      return;
    }
    if (t2.mode === 'browser-auto' || t2.mode === 'browser-forced') {
      if (!t2.engine) return;
      try {
        const snap = JSON.parse(t2.engine.snapshot_json());
        paintT2Badges(snap);
      } catch (err) {
        evictTier2();
      }
    } else if (t2.mode === 'server') {
      // Flush the pending buffer; keep recent events for next tick.
      const events = t2.pendingEvents.slice(-T2_SERVER_EVENTS);
      fetch('/api/compute', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ events: events }),
      })
        .then((r) => r.json())
        .then((snap) => paintT2Badges(snap))
        .catch((err) => {
          console.warn('flashlight: /api/compute call failed', err);
        });
    }
  }

  function paintT2Badges(snap) {
    if (!snap) return;
    paintRateBadge(snap.stats);
    // z-score + drift badges were removed in Phase F-3 — drift compared
    // a wire `format` the live event never carried (always 0) and the
    // z-score was a weak self-referential rate signal. The analytics
    // engine still computes them (the snapshot carries snap.anomaly /
    // snap.schema_diff); they just aren't surfaced until they have
    // trustworthy inputs. Honest signals only (Principle 11).
    paintT2Sparkline(snap.sparkline);
  }

  function paintRateBadge(stats) {
    const el = document.getElementById('t2-rate-value');
    if (!el || !stats) return;
    el.textContent = (stats.rate_hz || 0).toFixed(2) + ' Hz';
  }

  // Tier 2 sparkline — same Canvas 2D bar rendering as Tier 1, sourced
  // from SparklineSnapshot.points (60 bins over 5 min by default). The
  // window label is updated from the snapshot's window_ms so an operator
  // who supplies ?window_ms=N gets the right caption.
  const t2Spark = {
    canvas: null,
    ctx: null,
    lastBins: 0,
  };

  function ensureT2SparkCtx() {
    if (t2Spark.canvas) return t2Spark.ctx;
    t2Spark.canvas = document.getElementById('t2-sparkline');
    if (!t2Spark.canvas) return null;
    t2Spark.ctx = t2Spark.canvas.getContext('2d');
    sizeT2SparkCanvas();
    window.addEventListener('resize', sizeT2SparkCanvas);
    return t2Spark.ctx;
  }

  function sizeT2SparkCanvas() {
    if (!t2Spark.canvas) return;
    const dpr = window.devicePixelRatio || 1;
    const cssW = t2Spark.canvas.clientWidth;
    const cssH = t2Spark.canvas.clientHeight;
    t2Spark.canvas.width = Math.floor(cssW * dpr);
    t2Spark.canvas.height = Math.floor(cssH * dpr);
    if (t2Spark.ctx) {
      t2Spark.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    }
  }

  function paintT2Sparkline(sparkline) {
    if (!sparkline || !Array.isArray(sparkline.points)) return;
    const ctx = ensureT2SparkCtx();
    if (!ctx) return;
    const points = sparkline.points;
    const w = t2Spark.canvas.clientWidth;
    const h = t2Spark.canvas.clientHeight;
    ctx.clearRect(0, 0, w, h);
    if (points.length === 0) return;

    // window_ms → human-readable label. The Tier 1 sparkline uses
    // 60 s; the Tier 2 default is 5 min. Operators with ?window_ms=
    // see whatever they set, formatted as Nm or N s.
    const labelEl = document.getElementById('t2-sparkline-window');
    if (labelEl) {
      const ms = sparkline.window_ms || 0;
      labelEl.textContent = ms >= 60_000 ? Math.round(ms / 60_000) + ' m' : Math.round(ms / 1_000) + ' s';
    }

    const max = Math.max(1, ...points);
    const stepX = w / points.length;
    ctx.fillStyle = getCssVar('--accent', '#20964f');
    for (let i = 0; i < points.length; i++) {
      const v = points[i];
      const barH = (v / max) * h;
      ctx.fillRect(i * stepX, h - barH, Math.max(1, stepX - 1), barH);
    }
    t2Spark.lastBins = points.length;

    // Peak / now stats next to the T2 sparkline label, same shape as
    // Tier 1.
    const statsEl = document.getElementById('t2-sparkline-stats');
    if (statsEl) {
      const now = points[points.length - 1] || 0;
      statsEl.textContent = 'peak ' + fmtRate(max) + ' · now ' + fmtRate(now);
    }
  }

  function evictTier2() {
    t2.mode = 'off';
    t2.engine = null;
    hideT2Surfaces();
    if (t2.snapshotTimer) {
      clearInterval(t2.snapshotTimer);
      t2.snapshotTimer = null;
    }
  }

  function zerosOfLen(n) {
    if (n <= 0) return [];
    // Keep this small — Tier 1's SSE shape doesn't carry the actual
    // payload bytes, and the analytics core only uses .len() so a
    // sparse representation is sufficient. For very large payloads
    // we cap to avoid wasting browser memory.
    const cap = Math.min(n, 4096);
    return new Array(cap).fill(0);
  }

  function textEncode(s) {
    return new TextEncoder().encode(s);
  }

  // Patch the existing SSE 'sample' handler to forward into Tier 2.
  // Done by intercepting at the existing pushRow callsite via a
  // post-pushRow hook installed via Object.defineProperty would be
  // overengineered; instead we just append a forwardToTier2 call to
  // the same code path. The cleanest place is the sample listener
  // closure, but we can't reach it after `connect()` ran. Workaround
  // for the foundation PR: poll the most-recent row each snapshot
  // tick and replay deltas. Mark this as TODO until the next PR
  // refactors connect() to expose an `onSample` hook.
  //
  // For this foundation PR, we attach a second EventSource listener
  // on the same /events stream — same connection re-used by the
  // browser, both listeners see each event without a duplicate
  // network connection.
  if (t2.mode !== 'off') {
    const t2es = es;
    if (t2es) {
      t2es.addEventListener('sample', (ev) => {
        if (state.paused) return;
        try {
          const payload = JSON.parse(ev.data);
          forwardToTier2(payload);
        } catch (e) {
          // ignore
        }
      });
    }
  }
})();
