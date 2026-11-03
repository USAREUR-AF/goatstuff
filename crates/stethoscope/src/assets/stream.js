// stethoscope — SSE client. Minimal vanilla JS, no framework.
// Bound to window.STETHOSCOPE_APPLICABLE_IDS (string[]) and
// window.STETHOSCOPE_DIAGNOSE_URL (string) emitted inline by render.rs.
(function () {
  'use strict';

  var expected = window.STETHOSCOPE_APPLICABLE_IDS || [];
  var sseUrl = window.STETHOSCOPE_DIAGNOSE_URL;
  // Display-only lexicon — participant noun in goatnet-appropriate
  // wording ("vendor" or "org"), injected by render.rs. Used
  // only for input-placeholder display; the detected `kind` value the
  // picker submits stays the literal 'vendor' machine token.
  var LEX = (window.STETHOSCOPE_LEX && window.STETHOSCOPE_LEX.org)
    ? window.STETHOSCOPE_LEX
    : { org: 'vendor', org_upper: 'VENDOR', org_title: 'Vendor' };

  var settled = 0;
  var worst = 0; // 0=running, 1=pass, 2=warn, 3=fail
  var verdictRank = { pass: 1, warn: 2, fail: 3, skipped: 0, na: 0 };

  function $(id) { return document.getElementById(id); }

  function escape(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c];
    });
  }

  function pillClass(v) { return 'sx-pill sx-pill--' + v; }
  function pillLabel(v) { return v.toUpperCase(); }

  function applyOutcome(o) {
    var card = $('card-' + o.check_id);
    if (!card) return;

    // Update card classes for the verdict.
    card.classList.remove('is-na', 'is-skipped');
    if (o.verdict === 'na') card.classList.add('is-na');
    if (o.verdict === 'skipped') card.classList.add('is-skipped');
    card.classList.add('is-flash');
    setTimeout(function () { card.classList.remove('is-flash'); }, 600);

    // Swap the icon dot to the verdict color.
    var iconCell = card.querySelector('.sx-card__icon');
    if (iconCell) {
      iconCell.innerHTML = verdictIcon(o.verdict);
    }

    // Swap the skeleton + cost chip for the real summary text.
    var summary = card.querySelector('.sx-card__summary');
    if (summary) {
      summary.innerHTML = '<span>' + escape(o.summary) + '</span>';
    }

    // Swap the running pill for the verdict pill.
    var pillCell = card.querySelector('.sx-card__pill');
    if (pillCell) {
      pillCell.innerHTML =
        '<span class="' + pillClass(o.verdict) + '">' +
        '<span class="sx-pill__dot"></span>' + pillLabel(o.verdict) +
        '</span>';
    }

    // Findings row.
    if (o.findings && o.findings.length) {
      var existing = card.querySelector('.sx-findings');
      if (existing) existing.remove();
      var row = document.createElement('div');
      row.className = 'sx-findings';
      for (var i = 0; i < o.findings.length; i++) {
        var f = o.findings[i];
        var glyph = f.severity === 'warn' ? '!' : 'i';
        var inner = '<span class="sx-finding__glyph">' + escape(glyph) +
          '</span><span>' + escape(f.label) + '</span>';
        if (f.href) {
          row.innerHTML += '<a class="sx-finding sx-finding--' + escape(f.severity) +
            '" href="' + escape(f.href) + '" title="' + escape(f.detail) + '">' + inner + '</a>';
        } else {
          row.innerHTML += '<span class="sx-finding sx-finding--' + escape(f.severity) +
            '" title="' + escape(f.detail) + '">' + inner + '</span>';
        }
      }
      card.appendChild(row);
    }

    // Remediation block — only on Fail/Warn cards where checks attach
    // actionable next steps. Suppressed on Pass/Skipped/NA where
    // remediation makes no sense.
    if (o.remediation && o.remediation.length && (o.verdict === 'fail' || o.verdict === 'warn')) {
      var oldRx = card.querySelector('.sx-remediation');
      if (oldRx) oldRx.remove();
      var rx = document.createElement('div');
      rx.className = 'sx-remediation';
      rx.innerHTML = '<div class="sx-remediation__head">remediation</div>';
      for (var j = 0; j < o.remediation.length; j++) {
        var r = o.remediation[j];
        var item = document.createElement('div');
        item.className = 'sx-remediation__item';
        var title = '<div class="sx-remediation__title">' + escape(r.title) + '</div>';
        var body = '<div class="sx-remediation__detail">' + escape(r.detail) + '</div>';
        if (r.href) {
          title = '<div class="sx-remediation__title"><a href="' + escape(r.href) +
            '">' + escape(r.title) + ' ›</a></div>';
        }
        item.innerHTML = title + body;
        rx.appendChild(item);
      }
      card.appendChild(rx);
    }

    // Roll-up update.
    var rank = verdictRank[o.verdict] || 0;
    if (rank > worst) worst = rank;
    settled++;
    var settledCell = $('settled-count');
    if (settledCell) settledCell.textContent = String(settled);
  }

  function verdictIcon(v) {
    if (v === 'pass')    return '<span style="color:var(--c-green-hi)">✓</span>';
    if (v === 'warn')    return '<span style="color:var(--c-amber-hi)">▲</span>';
    if (v === 'fail')    return '<span style="color:var(--c-red-hi)">✕</span>';
    if (v === 'skipped') return '<span style="color:var(--fg-muted)">—</span>';
    if (v === 'na')      return '<span style="color:var(--fg-faint)">⊘</span>';
    return '<span class="sx-spinner" aria-hidden="true"></span>';
  }

  function finalizeRollup() {
    var rollup = $('rollup');
    var label = $('rollup-label');
    if (!rollup || !label) return;
    rollup.classList.remove(
      'sx-rollup--running', 'sx-rollup--pass', 'sx-rollup--warn', 'sx-rollup--fail'
    );
    var cls;
    var text;
    if (worst === 3) { cls = 'sx-rollup--fail'; text = 'FAIL'; }
    else if (worst === 2) { cls = 'sx-rollup--warn'; text = 'WARN'; }
    else if (worst === 1) { cls = 'sx-rollup--pass'; text = 'PASS'; }
    else { cls = 'sx-rollup--running'; text = 'NO VERDICTS'; }
    rollup.classList.add(cls);
    label.textContent = text;
  }

  if (!sseUrl) {
    // Landing page (no target): skip SSE wiring; the picker is the
    // whole point of this render. Fall through to the picker setup
    // + theme toggle below.
  } else {
  var es = new EventSource(sseUrl);
  es.addEventListener('outcome', function (e) {
    try {
      var data = JSON.parse(e.data);
      applyOutcome(data);
    } catch (err) {
      console.error('outcome parse failed', err);
    }
  });
  es.addEventListener('done', function () {
    finalizeRollup();
    es.close();
  });
  es.addEventListener('error', function () {
    // EventSource auto-reconnects by default; we don't want that here —
    // a closed channel after `done` is the expected terminal state.
    if (settled >= expected.length) {
      es.close();
    }
  });

  // Re-run button.
  var rerun = $('rerun-btn');
  if (rerun) {
    rerun.addEventListener('click', function () {
      window.location.reload();
    });
  }
  }

  // ---- Target picker (ported from design-assets app.jsx) ----
  // "Place the stethoscope on…" popover. Triggered by the CHANGE
  // affordance on the target value. On landing pages (no target),
  // the picker is mounted open without a scrim so the URL state `?`
  // means "I haven't decided yet — help me pick."
  var RECENT_KEY = 'stethoscope.recent';
  var RECENT_CAP = 5;

  function readRecents() {
    try {
      var raw = localStorage.getItem(RECENT_KEY);
      if (!raw) return [];
      var arr = JSON.parse(raw);
      if (!Array.isArray(arr)) return [];
      return arr.filter(function (r) {
        return r && typeof r.kind === 'string' && typeof r.value === 'string';
      });
    } catch (e) { return []; }
  }

  function pushRecent(kind, value) {
    var recents = readRecents().filter(function (r) {
      return !(r.kind === kind && r.value === value);
    });
    recents.unshift({ kind: kind, value: value });
    if (recents.length > RECENT_CAP) recents.length = RECENT_CAP;
    try { localStorage.setItem(RECENT_KEY, JSON.stringify(recents)); } catch (e) {}
  }

  function urlencode(s) {
    // Match render.rs::urlencode — only encode chars outside the
    // unreserved set; specifically encode `/` and `:` and space.
    return String(s).replace(/[^A-Za-z0-9\-_.~]/g, function (c) {
      return c.split('').map(function (ch) {
        return '%' + ch.charCodeAt(0).toString(16).toUpperCase().padStart(2, '0');
      }).join('');
    });
  }

  function navigateTo(kind, value) {
    var v = String(value || '').trim();
    if (!v) return;
    pushRecent(kind, v);
    window.location.href = '/?' + kind + '=' + urlencode(v);
  }

  function renderRecents(container, current) {
    if (!container) return;
    var recents = readRecents();
    container.innerHTML = '';
    if (!recents.length) return;
    for (var i = 0; i < recents.length; i++) {
      var r = recents[i];
      var isCur = current && r.kind === current.kind && r.value === current.value;
      var btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'sx-preset' + (isCur ? ' is-current' : '');
      btn.dataset.kind = r.kind;
      btn.dataset.value = r.value;
      btn.innerHTML =
        '<span class="kind">' + escape(r.kind) + '</span>' +
        '<span class="value">' + escape(r.value) + '</span>';
      btn.addEventListener('click', (function (kind, value) {
        return function () { navigateTo(kind, value); };
      })(r.kind, r.value));
      container.appendChild(btn);
    }
  }

  function setKind(picker, kind, explicit) {
    var btns = picker.querySelectorAll('.sx-kind-btn');
    for (var i = 0; i < btns.length; i++) {
      btns[i].classList.toggle('is-on', btns[i].dataset.kind === kind);
    }
    var input = picker.querySelector('.sx-picker__input');
    if (input) {
      input.placeholder = kind === 'topic' ? LEX.org + '/topic/version'
        : kind === 'vendor' ? LEX.org
        : LEX.org + '/system';
      input.dataset.kind = kind;
    }
    // An explicit chip click pins the kind; auto-detect-on-typing
    // honors that and stops shape-detecting.
    if (explicit) {
      picker.dataset.kindExplicit = '1';
    }
  }

  function autoDetectKind(value) {
    // Operator typed without explicitly clicking a kind chip.
    // Shape-detect: no slash → vendor; 1 slash → system; 2+ slashes
    // → topic. Matches the portal's key_pattern convention. The
    // kind chips remain as an explicit override.
    var v = String(value || '').trim();
    if (!v) return 'vendor';
    var slashes = (v.match(/\//g) || []).length;
    if (slashes === 0) return 'vendor';
    if (slashes === 1) return 'system';
    return 'topic';
  }

  // ---- Live typeahead index ----
  // Fetched once per picker mount from /api/index. Stays in module
  // closure; cap-20 list with "+N more" indicator when filtered set
  // exceeds the cap.
  var indexPromise = null;
  function getIndex() {
    if (indexPromise) return indexPromise;
    indexPromise = fetch('/api/index', { credentials: 'same-origin' })
      .then(function (r) { return r.ok ? r.json() : { vendors: [], topics: [], systems: [], portal_disabled: true }; })
      .catch(function () { return { vendors: [], topics: [], systems: [], portal_disabled: true }; });
    return indexPromise;
  }

  var TYPEAHEAD_CAP = 20;

  function buildCandidates(idx, q) {
    // Each candidate is {kind, value, label}. `value` is the canonical
    // identifier the picker navigates with (topic = UUID-anchored key, per
    // the slot-alias display layer / ADR 1039); `label` is the operator-
    // facing display (the alias-projected key when present, else value). The
    // needle is matched case-insensitively against BOTH so an operator finds
    // a topic by its friendly name OR its UUID. Empty needle returns the
    // unfiltered set so the operator can browse.
    var needle = String(q || '').toLowerCase().trim();
    var pool = [];
    // Vendors/orgs arrive as {id, label?, aka?}: id is the navigate value (UUID
    // slot anchor or per-vendor handle), label is the friendly name shown, aka are
    // extra searchable names (display_name, slot handle). So typing a human
    // name finds the entity AND the dropdown shows that name, not the UUID.
    for (var i = 0; i < (idx.vendors || []).length; i++) {
      var v = idx.vendors[i];
      if (typeof v === 'string') { pool.push({ kind: 'vendor', value: v }); }
      else { pool.push({ kind: 'vendor', value: v.id, label: v.label || v.id, aka: v.aka || [] }); }
    }
    for (var i = 0; i < (idx.systems || []).length; i++) pool.push({ kind: 'system', value: idx.systems[i] });
    // Topics arrive as the {id, alias?} Labeled envelope; systems stay plain
    // strings (not the slot-alias namespace).
    for (var i = 0; i < (idx.topics  || []).length; i++) {
      var t = idx.topics[i];
      pool.push({ kind: 'topic', value: t.id, label: t.alias || t.id });
    }
    if (!needle) return pool;
    var out = [];
    for (var i = 0; i < pool.length; i++) {
      var hay = (pool[i].value + ' ' + (pool[i].label || '') + ' ' +
        ((pool[i].aka || []).join(' '))).toLowerCase();
      if (hay.indexOf(needle) !== -1) out.push(pool[i]);
    }
    return out;
  }

  function breadthFirst(items) {
    // Operator asked for branching, not alphabetical depth-first.
    // Group by (kind, first-segment) — vendor candidates count as
    // their own first-segment. Round-robin across groups so the
    // dropdown shows diverse roots before going deep into any one.
    var groups = {};   // groupKey -> [item, item, ...]
    var groupOrder = []; // group keys in first-seen order
    for (var i = 0; i < items.length; i++) {
      var it = items[i];
      var firstSeg = it.value.split('/')[0];
      var key = it.kind + ':' + firstSeg;
      if (!groups[key]) { groups[key] = []; groupOrder.push(key); }
      groups[key].push(it);
    }
    var out = [];
    var anyLeft = true;
    while (anyLeft) {
      anyLeft = false;
      for (var g = 0; g < groupOrder.length; g++) {
        var bucket = groups[groupOrder[g]];
        if (bucket.length) { out.push(bucket.shift()); anyLeft = true; }
      }
    }
    return out;
  }

  function renderTypeahead(container, items, total) {
    container.innerHTML = '';
    if (!total) {
      container.style.display = 'none';
      return;
    }
    container.style.display = 'flex';
    for (var i = 0; i < items.length; i++) {
      var it = items[i];
      var btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'sx-typeahead__item';
      btn.dataset.kind = it.kind;
      btn.dataset.value = it.value;
      // Relex the kind tag to the goatnet noun (vendor → "org"), same
      // display-only lexicon as the chips/eyebrow; topic/system are uniform.
      var kindLabel = it.kind === 'vendor' ? LEX.org : it.kind;
      var shown = it.label || it.value;
      // Secondary names: any aka the row goes by that ISN'T the shown label —
      // the slot handle ("pat-example"), so the operator sees what their needle
      // matched AND two same-named orgs (e.g. "Example Org" example-a vs
      // example-b) disambiguate by handle.
      var aka = (it.aka || []).filter(function (a) {
        return a && a.toLowerCase() !== String(shown).toLowerCase();
      });
      var akaHtml = aka.length
        ? '<span class="sx-typeahead__aka">' + escape(aka.join(' · ')) + '</span>'
        : '';
      // Show the operator-facing label (friendly name / alias-projected key when
      // present); navigation still uses the canonical `it.value` below.
      btn.innerHTML =
        '<span class="sx-typeahead__kind">' + escape(kindLabel) + '</span>' +
        '<span class="sx-typeahead__value">' + escape(shown) + '</span>' +
        akaHtml;
      btn.addEventListener('click', (function (kind, value) {
        return function () { navigateTo(kind, value); };
      })(it.kind, it.value));
      container.appendChild(btn);
    }
    if (total > items.length) {
      var hint = document.createElement('div');
      hint.className = 'sx-typeahead__more';
      hint.textContent = '+' + (total - items.length) + ' more — keep typing to narrow';
      container.appendChild(hint);
    }
  }

  function wirePicker(picker, opts) {
    opts = opts || {};
    var input = picker.querySelector('.sx-picker__input');
    var goBtn = picker.querySelector('.sx-picker__go');
    var kindBtns = picker.querySelectorAll('.sx-kind-btn');
    var presets = picker.querySelector('.sx-picker__presets');
    var typeahead = picker.querySelector('.sx-typeahead');

    for (var i = 0; i < kindBtns.length; i++) {
      kindBtns[i].addEventListener('click', (function (kind) {
        return function () {
          setKind(picker, kind, /*explicit=*/true);
          input && input.focus();
        };
      })(kindBtns[i].dataset.kind));
    }

    function submit() {
      if (!input) return;
      // Auto-detect from input shape unless the operator pinned
      // a kind via a chip click.
      var kind = picker.dataset.kindExplicit === '1'
        ? (input.dataset.kind || 'vendor')
        : autoDetectKind(input.value);
      navigateTo(kind, input.value);
    }

    if (goBtn) goBtn.addEventListener('click', submit);
    if (input) {
      input.addEventListener('keydown', function (e) {
        if (e.key === 'Enter') { e.preventDefault(); submit(); }
        if (e.key === 'Escape' && opts.onClose) { opts.onClose(); }
      });
      // Live typeahead: re-render on every keystroke against the
      // cached index. Keystrokes inside the closure are cheap;
      // /api/index is fetched once per page load and cached
      // server-side for 30s.
      if (typeahead) {
        var refreshTypeahead = function () {
          // Don't autopopulate when nothing has been typed. The
          // dropdown only appears once the operator gives it a
          // needle — surfacing the full index on focus is noise
          // (most of the index isn't what they were looking for) and
          // it covers the kind chips + Recent list below.
          var needle = String(input.value || '').trim();
          if (!needle) {
            typeahead.innerHTML = '';
            typeahead.style.display = 'none';
            return;
          }
          getIndex().then(function (idx) {
            if (idx.portal_disabled) {
              typeahead.innerHTML =
                '<div class="sx-typeahead__more">autocomplete unavailable — portal not configured</div>';
              typeahead.style.display = 'flex';
              return;
            }
            var matches = buildCandidates(idx, input.value);
            var ordered = breadthFirst(matches);
            renderTypeahead(typeahead, ordered.slice(0, TYPEAHEAD_CAP), ordered.length);
          });
        };
        input.addEventListener('input', refreshTypeahead);
        // Don't prime on focus or mount — wait for the operator to
        // actually type something.
      }
    }

    renderRecents(presets, opts.current);
  }

  // In-page picker (closed by default, opens via CHANGE button).
  var changeBtn = $('change-target-btn');
  var inPagePicker = document.querySelector('.sx-picker:not(.is-landing)');
  var inPageScrim = document.querySelector('.sx-picker-scrim:not(.is-landing)');
  if (changeBtn && inPagePicker) {
    var openPicker = function () {
      inPagePicker.classList.add('is-open');
      if (inPageScrim) inPageScrim.classList.add('is-open');
      var input = inPagePicker.querySelector('.sx-picker__input');
      if (input) { input.focus(); input.select(); }
    };
    var closePicker = function () {
      inPagePicker.classList.remove('is-open');
      if (inPageScrim) inPageScrim.classList.remove('is-open');
    };
    changeBtn.addEventListener('click', openPicker);
    if (inPageScrim) inPageScrim.addEventListener('click', closePicker);
    wirePicker(inPagePicker, {
      onClose: closePicker,
      current: inPagePicker.dataset.kind
        ? { kind: inPagePicker.dataset.kind, value: inPagePicker.dataset.value || '' }
        : null,
    });
  }

  // Landing picker (mounted open, no scrim, no CHANGE button).
  var landingPicker = document.querySelector('.sx-picker.is-landing');
  if (landingPicker) {
    wirePicker(landingPicker, { onClose: null, current: null });
    var landingInput = landingPicker.querySelector('.sx-picker__input');
    if (landingInput) { landingInput.focus(); }
  }

  // Back button — falls through to history.back() when there's a
  // previous entry, else navigates to /. Always present on the
  // diagnose pages (render.rs puts it in the topbar); landing skips
  // it because going back from the landing page would leave the app.
  var backBtn = $('back-btn');
  if (backBtn) {
    backBtn.addEventListener('click', function () {
      if (window.history.length > 1) {
        window.history.back();
      } else {
        window.location.href = '/';
      }
    });
  }

  // Theme toggle. Default dark; persist in localStorage.
  var themeBtn = $('theme-toggle');
  if (themeBtn) {
    var saved = null;
    try { saved = localStorage.getItem('stethoscope-theme'); } catch (e) {}
    if (saved === 'light') document.documentElement.setAttribute('data-theme', 'light');
    themeBtn.textContent = (saved === 'light') ? '☾ dark' : '☀ light';
    themeBtn.addEventListener('click', function () {
      var current = document.documentElement.getAttribute('data-theme') || 'dark';
      var next = current === 'light' ? 'dark' : 'light';
      document.documentElement.setAttribute('data-theme', next);
      themeBtn.textContent = next === 'light' ? '☾ dark' : '☀ light';
      try { localStorage.setItem('stethoscope-theme', next); } catch (e) {}
    });
  }
})();
