
(function () {
  'use strict';

  var slides   = Array.from(document.querySelectorAll('#deck .slide'));
  var TOTAL    = slides.length;
  var current  = 0;

  var navSection   = document.getElementById('nav-section');
  var navCounter   = document.getElementById('nav-counter');
  var navJump      = document.getElementById('nav-jump');
  var progressFill = document.getElementById('progress-fill');
  var btnPrev      = document.getElementById('btn-prev');
  var btnNext      = document.getElementById('btn-next');
  var nextFloat    = document.getElementById('next-float');
  var lightbox     = document.getElementById('lightbox');
  var lbImg        = document.getElementById('lb-img');

  // ── UI UPDATE ───────────────────────────────────────
  function updateUI() {
    var s = slides[current];
    navSection.textContent   = s.dataset.section || '';
    navCounter.textContent   = (current + 1) + ' / ' + TOTAL;
    progressFill.style.width = ((current + 1) / TOTAL * 100) + '%';
    btnPrev.disabled = (current === 0);
    btnNext.disabled = (current === TOTAL - 1);
    nextFloat.style.display = s.dataset.interactive === 'true' ? 'block' : 'none';
    if (sidebar && sidebar.classList.contains('open')) updateTocActive();
  }

  // ── ACTIVATE SLIDE ─────────────────────────────────
  function goTo(idx) {
    slides[current].classList.remove('active');
    current = Math.max(0, Math.min(TOTAL - 1, idx));

    var s = slides[current];
    s.classList.add('active');

    // Lazy-load iframe: swap data-src → src on first visit
    var iframe = s.querySelector('iframe[data-src]');
    if (iframe) {
      iframe.src = iframe.dataset.src;
      iframe.removeAttribute('data-src');
      // Once the iframe content paints, measure its real scrollHeight and
      // update nativeSizes so the scale calc fits the actual content
      // (not whatever h was declared statically). Cross-origin guard the
      // contentDocument access — we own all iframe HTML files so this
      // succeeds, but defensive anyway.
      iframe.addEventListener('load', function () {
        try {
          var doc = iframe.contentDocument;
          if (!doc || !doc.body) return;
          var realH = doc.body.scrollHeight;
          var realW = doc.body.scrollWidth;
          var key = iframe.dataset.scaleKey ||
                    (iframe.src ? iframe.src.split('/').pop().split('?')[0] : '');
          if (key && nativeSizes[key] && nativeSizes[key].w !== 0 && realH > 0) {
            nativeSizes[key].h = Math.max(nativeSizes[key].h, realH);
            // Width grows similarly if the inner layout is wider than declared.
            if (realW > nativeSizes[key].w) nativeSizes[key].w = realW;
            scaleActiveIframe();
          }
        } catch (e) {
          /* cross-origin or detached — leave declared sizes alone */
        }
      });
    }

    // Auto-dismiss iframe hint
    var hint = s.querySelector('.iframe-hint');
    if (hint) {
      hint.classList.remove('fade');
      clearTimeout(hint._fadeTimer);
      hint._fadeTimer = setTimeout(function () { hint.classList.add('fade'); }, 4500);
    }

    updateUI();
  }

  // ── BULLET REVEAL ──────────────────────────────────
  function revealNext(s) {
    var bullets  = s.querySelectorAll('.bullet');
    var revealed = parseInt(s.dataset.revealed || '0', 10);
    if (revealed < bullets.length) {
      bullets[revealed].classList.add('visible');
      s.dataset.revealed = revealed + 1;
      return true;
    }
    return false;
  }

  // ── ADVANCE ────────────────────────────────────────
  function advance() {
    var s = slides[current];
    // Try to reveal the next bullet first; only move slides when exhausted
    var hasBullets = parseInt(s.dataset.bullets || '0', 10) > 0;
    if (hasBullets && revealNext(s)) return;
    if (current < TOTAL - 1) goTo(current + 1);
  }

  // ── RETREAT ────────────────────────────────────────
  function retreat() {
    if (current > 0) goTo(current - 1);
  }

  // ── BUTTON / KEYBOARD ──────────────────────────────
  btnNext.addEventListener('click', advance);
  btnPrev.addEventListener('click', retreat);
  nextFloat.addEventListener('click', advance);

  // ── SLIDE JUMP ─────────────────────────────────────
  navCounter.addEventListener('click', function () {
    navCounter.style.display = 'none';
    navJump.style.display    = 'block';
    navJump.max   = TOTAL;
    navJump.value = current + 1;
    navJump.select();
  });

  function commitJump() {
    var val = parseInt(navJump.value, 10);
    navJump.style.display    = 'none';
    navCounter.style.display = '';
    if (!isNaN(val) && val >= 1 && val <= TOTAL) goTo(val - 1);
  }

  navJump.addEventListener('keydown', function (e) {
    if (e.key === 'Enter')  { e.preventDefault(); commitJump(); }
    if (e.key === 'Escape') { navJump.style.display = 'none'; navCounter.style.display = ''; }
  });
  navJump.addEventListener('blur', commitJump);

  document.addEventListener('keydown', function (e) {
    if (document.activeElement === navJump) return;
    if (lightbox.classList.contains('open')) {
      if (e.key === 'Escape') closeLightbox();
      return;
    }
    if (e.key === 'ArrowRight' || e.key === ' ') { e.preventDefault(); advance(); }
    if (e.key === 'ArrowLeft')                    { e.preventDefault(); retreat(); }
    if (e.key === 'f' || e.key === 'F')           toggleFullscreen();
    if (e.key === 'Escape')                        { /* nothing */ }
  });

  // Click anywhere on a non-interactive slide to advance
  document.getElementById('deck').addEventListener('click', function (e) {
    if (lightbox.classList.contains('open')) return;
    var s = slides[current];
    if (s.dataset.interactive === 'true') return;
    if (e.target.closest('.diagram-img')) return; // handled by lightbox
    advance();
  });

  // ── LIGHTBOX ───────────────────────────────────────
  function openLightbox(src, alt) {
    lbImg.src = src;
    lbImg.alt = alt || '';
    lightbox.classList.add('open');
  }
  function closeLightbox() {
    lightbox.classList.remove('open');
  }

  document.getElementById('lb-overlay').addEventListener('click', closeLightbox);
  document.getElementById('lb-close').addEventListener('click', closeLightbox);

  document.querySelectorAll('.diagram-img').forEach(function (img) {
    img.addEventListener('click', function (e) {
      e.stopPropagation();
      openLightbox(img.src, img.alt);
    });
  });

  // ── FULLSCREEN ─────────────────────────────────────
  function toggleFullscreen() {
    if (!document.fullscreenElement) {
      document.documentElement.requestFullscreen().catch(function () {});
    } else {
      document.exitFullscreen();
    }
  }

  // ── IFRAME AUTO-SCALE ──────────────────────────────
  // Scale interactive slides so their content fits the available area
  var nativeSizes = {
    'pattern-internal.html': { w: 1000, h: 640 },
    'pattern-icd.html':      { w: 1000, h: 640 },
    'pattern-api.html':      { w: 1000, h: 640 },
    'pattern-pubsub.html':   { w: 1000, h: 640 }
  };

  function scaleActiveIframe() {
    var s = slides[current];
    if (s.dataset.interactive !== 'true') return;
    var iframe  = s.querySelector('iframe');
    var wrapper = s.querySelector('.iframe-wrapper');
    if (!iframe || !wrapper) return;

    // Get the src (or data-src if not yet loaded)
    var src = iframe.src || iframe.dataset.src || '';
    var fname = (src ? src.split('/').pop().split('?')[0] : '') || (iframe.dataset.scaleKey || '');
    var nat = nativeSizes[fname];
    if (!nat || nat.w === 0) return; // responsive file — leave alone

    var aw = wrapper.clientWidth;
    var ah = wrapper.clientHeight;
    if (!aw || !ah) return;

    var scale = Math.min(aw / nat.w, ah / nat.h, 1);

    iframe.style.width  = nat.w + 'px';
    iframe.style.height = nat.h + 'px';
    iframe.style.transformOrigin = 'top center';
    iframe.style.transform = 'scale(' + scale + ')';

    // Negative margin to pull content up so scaled iframe doesn't leave gap
    var scaledH = nat.h * scale;
    iframe.style.marginBottom = (scaledH - nat.h) + 'px';
  }

  window.addEventListener('resize', function () { scaleActiveIframe(); });

  // ── SIDEBAR / TOC ──────────────────────────────────
  var menuBtn        = document.getElementById('menu-btn');
  var sidebar        = document.getElementById('sidebar');
  var sidebarOverlay = document.getElementById('sidebar-overlay');
  var toc            = document.getElementById('toc');

  // Build TOC entries from slide data
  var lastSection = null;
  slides.forEach(function (s, i) {
    var section = s.dataset.section || '';
    if (section !== lastSection) {
      var sec = document.createElement('div');
      sec.className   = 'toc-section-label';
      sec.textContent = section;
      toc.appendChild(sec);
      lastSection = section;
    }
    var titleEl = s.querySelector('.slide-title') ||
                  s.querySelector('.title-main')  ||
                  s.querySelector('.questions-big');
    var label = titleEl ? titleEl.textContent.replace(/\s+/g, ' ').trim() : 'Slide ' + (i + 1);
    var item = document.createElement('div');
    item.className    = 'toc-item';
    item.dataset.idx  = i;
    // Build the spans as nodes: the label is slide text, and assigning it
    // through innerHTML would re-parse any < or & in a title as markup.
    var num = document.createElement('span');
    num.className   = 'toc-num';
    num.textContent = String(i + 1);
    var text = document.createElement('span');
    text.className   = 'toc-label';
    text.textContent = label;
    item.appendChild(num);
    item.appendChild(text);
    item.addEventListener('click', function () {
      goTo(parseInt(this.dataset.idx, 10));
      closeSidebar();
    });
    toc.appendChild(item);
  });

  function updateTocActive() {
    toc.querySelectorAll('.toc-item').forEach(function (item) {
      item.classList.toggle('active', parseInt(item.dataset.idx, 10) === current);
    });
    var active = toc.querySelector('.toc-item.active');
    if (active) active.scrollIntoView({ block: 'nearest' });
  }

  function openSidebar() {
    sidebar.classList.add('open');
    sidebarOverlay.classList.add('open');
    updateTocActive();
  }
  function closeSidebar() {
    sidebar.classList.remove('open');
    sidebarOverlay.classList.remove('open');
  }

  menuBtn.addEventListener('click', function () {
    sidebar.classList.contains('open') ? closeSidebar() : openSidebar();
  });
  document.getElementById('sidebar-close').addEventListener('click', closeSidebar);
  sidebarOverlay.addEventListener('click', closeSidebar);

  // ── INIT ───────────────────────────────────────────
  goTo(0);
  scaleActiveIframe();
})();
