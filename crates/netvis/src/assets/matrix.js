/* netvis — who can talk to whom, at three levels of magnification.
 *
 * ONE renderer, three levels. A matrix is two axes and a "does this pair
 * connect" predicate; what changes between levels is only what the axes ARE.
 * Three separate matrices would be three things to keep in agreement.
 *
 *   L1  group → group   the default. The only level that survives the scope
 *                       envelope (ADR 0215: up to 6000 peers fleet-wide),
 *                       because the group set is the DESIGN and grows far
 *                       slower than the fleet.
 *   L2  peer → peer     bounded to one group pair from L1. The machines that
 *                       actually realise that relationship.
 *   L3  one pair        every rule that permits it, and both machines' full
 *                       identity mapping.
 *
 * Peer×peer across a whole goatnet is deliberately NOT a level: at 6000 peers
 * that is 36 million cells. It exists only inside a group pair.
 *
 * Axis labels are the derived SHORT name (identity.js). The raw NetBird name
 * is on hover and in L3 — it is the join key everything else uses, so it is
 * mapped, never replaced.
 */
(function () {
  'use strict';

  var NS = 'http://www.w3.org/2000/svg';
  function sv(n, a) {
    var e = document.createElementNS(NS, n);
    for (var k in a) if (a[k] !== undefined && a[k] !== null) {
      if (k === 'text') e.textContent = a[k]; else e.setAttribute(k, a[k]);
    }
    return e;
  }
  function el(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined && text !== null) e.textContent = text;
    return e;
  }

  /* ------------------------------------------------- PROGRESSIVE DISCLOSURE
     The rule this surface kept breaking: LEAD WITH THE NUMBER THAT MOVES,
     then the evidence beneath it. Every one of these pages had grown three
     paragraphs of rationale ABOVE the picture — all of it true, none of it
     the answer, and together taller than the matrix it was explaining. A
     reader who has to scroll past an essay to reach a 3x3 grid does not read
     the essay twice, and after the second visit does not read it at all.

     So: one sentence stays. The reasoning goes behind a disclosure that
     states what it will tell you, and stays shut until asked. Nothing is
     deleted — the explanations are the most valuable thing here and every one
     of them was written because somebody got it wrong once. They are just no
     longer in the way of the thing they explain. */
  function why(summary, paragraphs) {
    var d = el('details', 'why');
    d.appendChild(el('summary', null, summary));
    (Array.isArray(paragraphs) ? paragraphs : [paragraphs]).forEach(function (p) {
      d.appendChild(typeof p === 'string' ? el('p', null, p) : p);
    });
    return d;
  }

  /* Cell size is DERIVED, not fixed. It was a constant 20px, so a 3x3 site
     matrix drew a 60px picture adrift in a 2000px viewport while a 57-row
     group matrix drew one that overflowed. The matrix should use the space it
     is given: big when there is little to show, dense when there is a lot,
     never smaller than legible and never so large it stops reading as a grid. */
  var CELL_MIN = 14, CELL_MAX = 64;
  var CELL = 20, LEFT = 220, TOP = 220;

  /* Label gutters sized to the LONGEST LABEL ACTUALLY DRAWN, not a fixed 220.
     Short labels (GALAXY, SITE-A) left ~200px of dead margin on every side. */
  function geom(rows, cols, label) {
    function longest(a) {
      var m = 0;
      for (var i = 0; i < a.length; i++) m = Math.max(m, String(label(a[i])).length);
      return m;
    }
    var CH = 7.1;                                    // ~1ch of the mono face at 12px
    LEFT = Math.min(360, Math.max(90, longest(rows) * CH + 22));
    TOP  = Math.min(360, Math.max(90, longest(cols) * CH + 22));

    var availW = Math.max(320, (HOST.clientWidth || 1100) - LEFT - 48);
    var availH = Math.max(320, (window.innerHeight || 900) - TOP - 190);
    CELL = Math.round(Math.max(CELL_MIN, Math.min(CELL_MAX, Math.min(availW / cols.length, availH / rows.length))));
  }
  var D = null, ID = null, PERMITS = null, BYPAIR = null, FAMILIES = [];
  var GSITE = null, GCLASS = null;   // group index -> site / class
  var BRIDGE = null;                 // 'A>B' -> peer indices holding groups in BOTH
  var NOSITE = null;                 // axis label for groups in no scope family
  var SITES_REAL = 0;                // real scope families, NOSITE excluded
  var S = { level: 'site', sa: null, sb: null, ca: null, cb: null, ga: null, gb: null, pa: null, pb: null, q: '', nm: 'short' };

  /* THE FOUR NAME FORMS, and how each is reachable.
     NetBird names are whatever the enrolling machine called itself, so the
     axis shows the derived SHORT name — but the raw name is the join key for
     every other tool, and an operator who cannot get back to it cannot act on
     what they just saw. Short alone is a dead end.
       short    axis default
       full     hover, and the `full` axis mode
       netbird  the `netbird` axis mode, and the identity card
       mapping  the identity card — click any axis label */
  var NAME_MODES = ['short', 'full', 'netbird'];
  function peerLabel(i) {
    var id = ID[i];
    if (S.nm === 'netbird') return id.netbird;
    if (S.nm === 'full') return id.full || id.short;
    return id.short;
  }
  var HOST = null;

  /* Which of the three views this page is. The path decides, so each view has
     its own URL and can be linked, bookmarked and sent to someone. */
  function currentView() {
    var p = location.pathname.replace(/\/+$/, '');
    if (p === '/policy') return 'policy';
    if (p === '/findings') return 'findings';
    if (p === '/armed') return 'armed';
    if (p === '/assertions') return 'assertions';
    return 'fabric';
  }
  var VIEW = 'fabric';
  var REPORT = null;

  function boot() {
    HOST = document.getElementById('reach-root');
    if (!HOST) return;
    VIEW = currentView();
    /* The gate reads a different endpoint, because it is a different
       question. It needs no matrix, no peers and no geometry — asking for the
       reach payload here would be three management-API calls spent on a page
       that draws none of it. */
    if (VIEW === 'assertions') {
      fetch('/api/assertions', { headers: { accept: 'application/json' } })
        .then(function (r) { return r.json(); })
        .then(function (d) {
          if (d && d.error) { HOST.appendChild(el('p', 'warn', d.reason || d.error)); return; }
          REPORT = d; D = { findings: [] };
          HOST.appendChild(bar());
          LG();
        })
        .catch(function (e) { HOST.appendChild(el('p', 'warn', 'could not read /api/assertions: ' + e)); });
      return;
    }
    fetch('/api/reach' + (VIEW === 'policy' ? '?alt=policy' : ''),
          { headers: { accept: 'application/json' } })
      .then(function (r) { return r.json(); })
      .then(function (d) {
        if (d && d.error) { HOST.appendChild(el('p', 'warn', d.reason || d.error)); return; }
        D = d; prepare(); readUrl(); paint();
      })
      .catch(function (e) { HOST.appendChild(el('p', 'warn', 'could not read /api/reach: ' + e)); });
  }

  function prepare() {
    FAMILIES = (D.panels || []).map(function (p) { return p.family; })
      .filter(function (f) { return f && f !== '<unscoped>'; });
    var peers = D.peers || [];
    ID = peers.map(function (p) { return goatIdentity.resolve(p, D.nodes, FAMILIES); });
    /* Uniqueness is a property of the SET, not of one peer, so it is a second
       pass over the whole list — here, once, before any view narrows it. Run
       per view instead and a machine's label would change when the operator
       filtered, which is the same lie by a different route. */
    goatIdentity.disambiguate(ID, peers);

    /* Decompose every group into (site, class) once.
       site  = the scope family it belongs to (GALAXY-PROD, SITE-A, …)
       class = the SHARED SUFFIX's kind, from identity.js — so ZAGROS-PROD-INFRA
               and SITE-A-INFRA are the SAME class. That collapse is the whole
               point of the aggregate levels: the suffix vocabulary is shared
               across sites by design, so `infra` is one thing seen twice, not
               two things. */
    GSITE = {}; GCLASS = {};
    var offsite = [];
    D.nodes.forEach(function (nd, i) {
      var fam = null;
      for (var f = 0; f < FAMILIES.length; f++) {
        if (nd.name === FAMILIES[f] || nd.name.indexOf(FAMILIES[f] + '-') === 0) { fam = FAMILIES[f]; break; }
      }
      GSITE[i] = fam;
      if (!fam) offsite.push(nd.name);
      var sfx = fam && nd.name.length > fam.length ? nd.name.slice(fam.length + 1) : nd.name;
      GCLASS[i] = goatIdentity.classify(sfx)[0];
    });

    /* Groups belonging to no scope family are NOT a site, and the axis must not
       imply they are — calling the bucket `(unscoped)` invented a site that does
       not exist. One goatnet may have one such group (`All`), another
       has two (`All` and `mesh-dns`). So the label is derived, not assumed —
       name it after the groups it actually holds. `(no site)` told the reader
       only what the bucket is NOT, and the axis label is the one place with
       room to say what it IS — zagros's bucket is `All + mesh-dns`, which is a
       fact a viewer can act on. Past three the names stop fitting the axis, so
       the count carries it instead and the legend lists them.

       It cannot simply be dropped: `All` carries a live grant (zagros-prod's
       `SSH Access` sources from it), and a reachability surface that hides a
       live grant is worse than one with an odd row on its axis. */
    NOSITE = offsite.length === 0 ? '(no site)'
      : offsite.length <= 3 ? offsite.slice().sort().join(' + ')
      : '(no site prefix) · ' + offsite.length + ' groups';
    SITES_REAL = uniq(Object.values(GSITE).filter(Boolean)).length;
    Object.keys(GSITE).forEach(function (i) { if (!GSITE[i]) GSITE[i] = NOSITE; });

    BYPAIR = {};
    (D.edges || []).forEach(function (e, ei) {
      (BYPAIR[e.s + '>' + e.d] || (BYPAIR[e.s + '>' + e.d] = [])).push(ei);
    });

    // peer pair -> permitting edges. Built once; L2 slices it.
    var gset = peers.map(function (p) {
      var s = {}; (p.groups || []).forEach(function (g) { s[g] = 1; }); return s;
    });
    PERMITS = {};
    (D.edges || []).forEach(function (e, ei) {
      for (var a = 0; a < peers.length; a++) {
        if (!gset[a][e.s]) continue;
        for (var b = 0; b < peers.length; b++) {
          if (!gset[b][e.d]) continue;
          (PERMITS[a + '>' + b] || (PERMITS[a + '>' + b] = [])).push(ei);
        }
      }
    });

    /* ------------------------------------------------- SITES ARE BRIDGED
       The site matrix rolled up RULE edges and nothing else, so a site pair
       with no cross-family rule drew an empty cell — and an empty cell on
       this surface means "no rule permits it", which the viewer reads as a
       denial. On a goatnet where sites are bridged by DUAL MEMBERSHIP that
       denial is false, and it is false in the dangerous direction.

       The mechanism is not routing and must not be described as if it were:
       an L4 ACL does not forward, so a machine in A does not reach B THROUGH
       the bridge. What is true is simpler and still crosses the boundary — a
       machine that holds groups in both sites IS a machine of both sites, so
       anything in A that reaches it is talking to a machine of B.

       The server already knows: finding D6 says "<peer> holds groups in N
       scope families while no policy edge crosses a family boundary". It was
       computed, shipped, and had no cell to sit in. */
    BRIDGE = {};
    peers.forEach(function (p, i) {
      /* NOSITE is excluded: it is the bucket for groups in no scope family
         (`All`, `mesh-dns`), the axis already says it is not a site, and
         every peer is in `All` — so counting it would report the whole fleet
         as a bridge and bury the two rows that are one. */
      var sites = uniq((p.groups || []).map(function (g) { return GSITE[g]; })
                                       .filter(function (s) { return s && s !== NOSITE; }));
      for (var a = 0; a < sites.length; a++) {
        for (var b = 0; b < sites.length; b++) {
          if (sites[a] === sites[b]) continue;
          var k = sites[a] + '>' + sites[b];
          (BRIDGE[k] || (BRIDGE[k] = [])).push(i);
        }
      }
    });
  }

  /* How many machines reach this one at all. The bridge claim needs a size:
     a door nobody stands at is a different fact from one 41 machines use. */
  function reachedBy(i) {
    var n = 0;
    (D.peers || []).forEach(function (_, a) { if (a !== i && PERMITS[a + '>' + i]) n++; });
    return n;
  }

  // ------------------------------------------------------------- url state
  function readUrl() {
    var q = new URLSearchParams(location.search);
    S.level = q.get('level') || 'site';
    S.sa = q.get('sa'); S.sb = q.get('sb');
    S.ca = q.get('ca'); S.cb = q.get('cb');
    S.ga = n(q.get('ga')); S.gb = n(q.get('gb'));
    S.pa = n(q.get('pa')); S.pb = n(q.get('pb'));
    S.q = q.get('q') || '';
    S.nm = q.get('nm') || 'short';
    function n(v) { return v === null || v === '' ? null : +v; }
  }
  function pushUrl() {
    var q = new URLSearchParams();
    if (S.level !== 'site') q.set('level', S.level);
    ['sa','sb','ca','cb','ga','gb','pa','pb'].forEach(function (k) { if (S[k] !== null && S[k] !== undefined) q.set(k, S[k]); });
    if (S.q) q.set('q', S.q);
    if (S.nm !== 'short') q.set('nm', S.nm);
    history.pushState(null, '', location.pathname + (q.toString() ? '?' + q : ''));
  }

  // ----------------------------------------------------------------- paint
  function paint() {
    HOST.textContent = '';
    // The renderer refuses to draw a matrix it cannot vouch for. An absent
    // cell is only a denial while the policy set was actually read.
    if (!D.policies_read) {
      /* Nothing to draw. Say so once, in one sentence, then list what was
         refused and what to do — not the caveats run together into a
         paragraph, which is what this page showed for three days in
         September 2026 while its token was wrong and nobody could tell. */
      var box = el('div', 'refused');
      box.appendChild(el('h2', null, 'netvis cannot read the NetBird management API'));
      box.appendChild(el('p', null,
        'This page draws from the mesh policy set. It could not be read, so there is ' +
        'nothing to show and nothing below is a claim about what is permitted.'));
      var ul = el('ul', 'refused-notes');
      (D.notes || []).forEach(function (n) { ul.appendChild(el('li', null, n)); });
      box.appendChild(ul);
      box.appendChild(el('p', 'muted',
        'Usual cause: the API token netvis was deployed with is wrong or was rotated. ' +
        'Redeploy netvis with the current token; this page recovers on its own.'));
      HOST.appendChild(box);
      return;
    }
    HOST.appendChild(bar());
    body();
  }
  function body() {
    while (HOST.children.length > 1) HOST.removeChild(HOST.lastChild);
    /* A throw anywhere in a level used to escape paint() and leave the page
       blank UNDER a correct breadcrumb — which reads as "no data" rather than
       "this code is broken". L4 did not exist at all and failed exactly that
       way. A surface whose failure mode is indistinguishable from an empty
       result cannot be trusted to report an empty result. */
    try {
      if (VIEW === 'findings') { LF(); return; }
      if (VIEW === 'armed') { LA(); return; }
      /* The gate never ran prepare(), because it reads /api/assertions and
         needs no peers or geometry — so GSITE and the rest are undefined here.
         Without this branch a resize re-entered body() (which fires on any
         window resize, and D is the truthy {findings:[]} the gate sets) and
         fell through to L0, replacing the verdicts with a render fault. */
      if (VIEW === 'assertions') { LG(); return; }
      /* At the UNPOPULATED altitude there are no peers BY DESIGN. Falling
         through to the machine levels would print "this group has no members",
         which is a different claim and a false one — the peer list was never
         requested. */
      if (VIEW === 'policy' && (S.level === 'peers' || S.level === 'p' || S.level === 'bridge')) {
        /* A refusal that names the way out and does not provide it is a dead
           end with a polite sign on it. This one used to say "switch to the
           machines view" — and the nav link that does so returns to the SITE
           level, so following the instruction threw away the drill that got
           you here. The link below carries the whole state across. */
        var w = el('p', 'warn',
          'The groups-and-rules view does not read peers, so it cannot say which '
          + 'machines are in this group. That is not a claim that the group is empty — '
          + 'it is a different question, asked at a different altitude. ');
        var a = el('a', 'lk', 'Open this exact drill in the machines view →');
        a.href = '/' + location.search;
        w.appendChild(a);
        HOST.appendChild(w);
        return;
      }
      if (S.level === 'bridge' && S.sa && S.sb) LB();
      else if (S.level === 'p' && S.pa !== null && S.pb !== null) L4();
      else if (S.level === 'peers' && S.ga !== null && S.gb !== null) L3();
      else if (S.level === 'g') L2();
      else if (S.level === 'class') L1();
      else L0();
    } catch (err) {
      HOST.appendChild(el('p', 'warn',
        'This view failed to render, which is a fault in netvis — NOT a claim '
        + 'that nothing is permitted here. ' + (err && err.message ? err.message : err)));
      if (window.console) console.error('netvis level render failed', err);
    }
  }

  /* TWO ROWS, not one. The bar was a single flex line holding the five views,
     the breadcrumb, the name-form control and the filter — and a breadcrumb at
     the peer level carries two full group names, so on a real drill it wrapped
     every crumb onto its own stacked line, shoved the filter box off the right
     edge and gave the page a horizontal scrollbar. The views and the filter
     are FIXED-WIDTH furniture; the breadcrumb is the only part that grows with
     the drill. Giving it its own row is what stops one of them deforming the
     other. */
  function bar() {
    var wrap = el('div', 'barwrap');
    var b = el('div', 'bar');

    // The three views, always visible, so nobody has to know a URL exists.
    var vs = el('span', 'views');
    [['fabric', '/', 'machines'],
     ['policy', '/policy', 'groups + rules'],
     ['findings', '/findings', 'findings'],
     ['armed', '/armed', 'inert rules'],
     ['assertions', '/assertions', 'assertions']].forEach(function (v) {
      var a = el('a', 'vw' + (VIEW === v[0] ? ' cur' : ''), v[2]);
      a.href = v[1];
      if (v[0] === 'findings' && D && D.findings && D.findings.length) {
        var n = D.findings.filter(function (f) { return f.severity === 'alarm'; }).length;
        if (n) a.appendChild(el('span', 'vcount', String(n)));
      }
      vs.appendChild(a);
    });
    b.appendChild(vs);

    // The gate has no drill and nothing to filter; a breadcrumb whose links
    // repaint a matrix that was never fetched is a trap, not a convenience.
    if (VIEW === 'assertions') { wrap.appendChild(b); return wrap; }

    var c = el('span', 'crumb');
    c.appendChild(lk('sites', function () {
      S.level = 'site'; S.sa = S.sb = S.ca = S.cb = null; S.ga = S.gb = S.pa = S.pb = null;
      S.q = ''; pushUrl(); paint();
    }, S.level === 'site'));
    if (S.sa) {
      // A site pair reached because machines bridge it has no class matrix
      // under it. Its crumb must return to the bridge list, not to a level
      // that would render blank and read as "nothing here".
      var atBridge = S.level === 'bridge';
      c.appendChild(el('span', 'sep', '›'));
      c.appendChild(lk(S.sa + ' → ' + S.sb, function () {
        S.level = atBridge ? 'bridge' : 'class';
        S.ca = S.cb = null; S.ga = S.gb = S.pa = S.pb = null; S.q = ''; pushUrl(); paint();
      }, S.level === 'class' || atBridge));
    }
    if (S.ca) {
      c.appendChild(el('span', 'sep', '›'));
      c.appendChild(lk(S.ca + ' → ' + S.cb, function () {
        S.level = 'g'; S.ga = S.gb = S.pa = S.pb = null; S.q = ''; pushUrl(); paint();
      }, S.level === 'g'));
    }
    if (S.ga !== null && S.gb !== null) {
      c.appendChild(el('span', 'sep', '›'));
      c.appendChild(lk(D.nodes[S.ga].name + ' → ' + D.nodes[S.gb].name, function () {
        S.level = 'peers'; S.pa = S.pb = null; S.q = ''; pushUrl(); paint();
      }, S.level === 'peers'));
    }
    if (S.pa !== null && S.pb !== null) {
      c.appendChild(el('span', 'sep', '›'));
      c.appendChild(lk(ID[S.pa].short + ' → ' + ID[S.pb].short, null, true));
    }

    // Axis-name control. Only where the axes ARE machines; group and site
    // axes have one true name and a selector there would be noise.
    if (S.level === 'peers') {
      var nmw = el('span', 'names');
      nmw.appendChild(el('span', 'nl', 'names'));
      NAME_MODES.forEach(function (m) {
        nmw.appendChild(lk(m, function () { S.nm = m; pushUrl(); body(); }, S.nm === m));
      });
      b.appendChild(nmw);
    }

    var q = el('input', 'q');
    q.type = 'search';
    q.placeholder = S.level === 'peers' ? 'filter machines…'
                  : S.level === 'site' ? 'filter sites…'
                  : S.level === 'class' ? 'filter classes…' : 'filter groups…';
    q.value = S.q;
    q.addEventListener('input', function () { S.q = q.value; pushUrl(); body(); });
    b.appendChild(q);

    wrap.appendChild(b);
    // Row two: the only part that grows with the drill.
    if (c.children.length > 1) wrap.appendChild(c);
    return wrap;
  }
  function lk(text, fn, cur) {
    var a = el('a', 'lk' + (cur ? ' cur' : ''), text);
    if (fn) { a.href = '#'; a.addEventListener('click', function (e) { e.preventDefault(); fn(); }); }
    return a;
  }


  /* --------------------------------------------------------------- L0 site
     Sites as cells. The top of the hierarchy because the fleet grows by SITE
     (ADR 0215: 6 sites now, 5 forward + 1 rear, and more later) and a viewer
     opening this surface wants to know first whether sites are isolated from
     each other — which on this goatnet they are, entirely. */
  function L0() {
    var allSites = uniq(Object.keys(GSITE).map(function (i) { return GSITE[i]; })).sort();
    var sites = allSites;
    var q = S.q.toLowerCase();
    if (q) sites = sites.filter(function (s) { return s.toLowerCase().indexOf(q) >= 0; });

    var agg = aggregate(function (i) { return GSITE[i]; });
    var byRule = function (a, b) { return agg[a + '>' + b]; };
    /* A cell is drawn if EITHER source puts reach across it. `[]` is the
       bridged-only case: real reach, zero rules, and it must not render as
       the denial it used to. */
    var cell = function (a, b) {
      var k = a + '>' + b;
      return agg[k] || (a !== b && BRIDGE[k] ? [] : undefined);
    };
    var ruled = function (a, b) { return !!agg[a + '>' + b]; };
    var nOff = Object.keys(GSITE).filter(function (i) { return GSITE[i] === NOSITE; }).length;

    var bridged = [];
    sites.forEach(function (a) {
      sites.forEach(function (b) {
        if (a !== b && BRIDGE[a + '>' + b] && !agg[a + '>' + b]) bridged.push(a + ' → ' + b);
      });
    });

    HOST.appendChild(el('p', 'sum',
      SITES_REAL + ' sites' + (nOff ? ' + ' + NOSITE : '') + ' · '
      + (D.edges || []).length + ' permitted group pairs across them · click a cell'));
    var split = realizationSplit(sites, sites, byRule);
    var ll = ladderLine(split);
    if (ll) HOST.appendChild(ll);
    if (bridged.length) {
      HOST.appendChild(el('p', 'sum key',
        bridged.length + ' site pair(s) are crossed with NO RULE between them — '
        + bridged.join(', ') + '. Click one for the machines.'));
      HOST.appendChild(why('why a cell with no rule is still lit', [
        'Machines hold groups in both sites, so a machine of one is also a machine of '
        + 'the other. Those cells are outlined rather than filled.',
        'This surface used to draw them empty, which on this matrix reads as "no rule '
        + 'permits it" — true about the rules, false about the reach.']));
    }
    /* The declared altitude does not read peers, so it cannot see a bridge —
       and saying nothing would let a reader conclude two sites are isolated
       when the machines view shows they are not. */
    if (VIEW === 'policy') {
      HOST.appendChild(el('p', 'sum key',
        'This is the DECLARED altitude: it reads rules, not membership. A site pair '
        + 'crossed only by machines that hold groups in both will read as empty here. '
        + 'The machines view is where that shows.'));
    }
    HOST.appendChild(mx(sites, sites, cell,
      function (s) { return s; },
      /* The no-site row is the only one on this axis that has to explain
         itself. It is not a site, and on a goatnet where nothing references
         its groups it draws an empty row and column — which reads as a bug
         until you know the row is a TRIPWIRE. Every group created without a
         site prefix lands here, so the row's label (derived from the groups
         it holds, above) is what changes when one does. Hiding it while empty
         was considered and rejected: a stray group with no rule yet is
         exactly the case worth catching, and that case has no lit cell. */
      function (s) {
        var n = Object.keys(GSITE).filter(function (i) { return GSITE[i] === s; }).length;
        if (s !== NOSITE) return s + '\n' + n + ' groups';
        /* Over `allSites`, NOT the filtered list: a filter that hides the one
           site this bucket reaches would otherwise flip the tooltip to
           "empty on purpose" while the grant is still there. */
        var lit = allSites.some(function (t) { return cell(s, t) || cell(t, s); });
        return s + '\n' + n + (n === 1 ? ' group' : ' groups') + ', in no scope family — not a site.'
          + '\nCatch-all for groups with no site prefix.'
          + (lit ? '\nA rule references one of them, so this row is live — the cells are real grants.'
                 : '\nNo rule references them, so this row is empty on purpose.')
          + '\nIt stays on the axis so a group that lands here by accident shows up in this label.';
      },
      function (a, b) {
        S.sa = a; S.sb = b; S.q = '';
        // A bridged-only cell has no rule edges under it, so the class level
        // would be a blank page. The true answer there is WHICH MACHINES
        // cross, so that is where it goes.
        S.level = ruled(a, b) ? 'class' : 'bridge';
        pushUrl(); paint();
      },
      unrAny, null, ruled, null, true, null,
      function (a, b, es) {
        if (es.length) return null;
        var n = (BRIDGE[a + '>' + b] || []).length;
        return 'no rule connects these sites — ' + n + ' machine(s) hold groups in both, '
             + 'so a machine of ' + a + ' is also a machine of ' + b;
      }));
    HOST.appendChild(legend('site', split, bridged.length));
  }

  /* ------------------------------------------------------------ LB bridge
     The machines that are the door between two sites. Reached only from a
     site cell that no rule permits — the case that used to be drawn as a
     denial and had nowhere to go. */
  function LB() {
    var peers = D.peers || [];
    var list = BRIDGE[S.sa + '>' + S.sb] || [];
    if (!peers.length) {
      HOST.appendChild(el('p', 'warn',
        'The peer list was not read, so which machines bridge these sites is unknown.'));
      return;
    }
    /* Lead with the number that moves. "2 machines bridge these sites" is a
       fact; "and 100 machines reach one of them" is the reason to care, and
       it is the difference between a door nobody stands at and a door the
       whole goatnet walks through. */
    var reach = list.map(reachedBy);
    var worst = reach.length ? Math.max.apply(null, reach) : 0;
    HOST.appendChild(el('p', 'sum',
      list.length + ' machine(s) hold groups in BOTH ' + S.sa + ' and ' + S.sb
      + ' · no rule connects the two sites'
      + (worst ? ' · the most-reached of them is reachable by ' + worst + ' machines' : '')));
    HOST.appendChild(why('what "bridged" means here, exactly', [
      'An L4 ACL does not forward, so nothing routes through these machines.',
      'What crosses the boundary is membership: each row below IS a machine of both sites, '
      + 'so whatever reaches it inside one site is talking to a machine of the other.',
      'Removing a group from one of these machines closes that door. No policy change is '
      + 'involved — which is also why nothing announces it when one opens.']));

    if (!list.length) {
      HOST.appendChild(el('p', 'warn', 'Nothing bridges these two sites.'));
      return;
    }
    var t = el('table', 'rules armed');
    var hd = el('tr');
    ['machine', 'groups in ' + S.sa, 'groups in ' + S.sb, 'connected', 'machines that reach it']
      .forEach(function (h, i) { hd.appendChild(el('th', i === 4 ? 'n' : null, h)); });
    t.appendChild(hd);
    list.map(function (i) { return { i: i, n: reachedBy(i) }; })
      .sort(function (a, b) { return b.n - a.n; })
      .forEach(function (r) {
        var p = peers[r.i];
        var inSite = function (s) {
          return (p.groups || []).filter(function (g) { return GSITE[g] === s; })
            .map(function (g) { return D.nodes[g].name; }).join(', ') || '—';
        };
        var tr = el('tr', r.n ? 'hot' : null);
        var nm = el('td', null, ID[r.i].short);
        nm.style.cursor = 'pointer';
        nm.addEventListener('click', function () { identityCard(r.i); });
        tr.appendChild(nm);
        tr.appendChild(el('td', null, inSite(S.sa)));
        tr.appendChild(el('td', null, inSite(S.sb)));
        tr.appendChild(el('td', null, p.connected ? 'yes' : 'no'));
        tr.appendChild(el('td', 'n', String(r.n)));
        t.appendChild(tr);
      });
    HOST.appendChild(t);
  }

  /* -------------------------------------------------------------- L1 class
     Like groups collapsed. GALAXY-PROD-INFRA and SITE-A-INFRA are one class,
     `MGMT-HOST`; the suffix vocabulary is shared across sites BY DESIGN
     (that is what makes a site rung readable by someone who knows another
     site), so the class is the honest unit one level above the group. */
  /* A DRILL FROM AN OFF-DIAGONAL CELL IS DIRECTIONAL. Clicking
     `GALAXY-PROD → SITE-A` must not filter both axes to the UNION of the two
     sites, or the matrix that comes back contains all four blocks, most of it
     the two diagonals you just navigated past.
     A drill that widens the question is worse than no drill: it looks like an
     answer.

     The fix is that the axes stop being the same list. Rows are the source
     site, columns the destination site. On the diagonal (`sa === sb`) the two
     lists coincide and nothing changes, which is why this survived — the
     common case was already right. */
  function L1() {
    var inSite = function (s) { return function (i) { return !s || GSITE[i] === s; }; };
    var rowPick = inSite(S.sa), colPick = inSite(S.sb);
    var classesOf = function (pick) {
      var c = uniq(Object.keys(GCLASS).filter(pick).map(function (i) { return GCLASS[i]; })).sort();
      var q = S.q.toLowerCase();
      return q ? c.filter(function (x) { return x.toLowerCase().indexOf(q) >= 0; }) : c;
    };
    var rows = classesOf(rowPick), cols = classesOf(colPick);

    var agg = aggregate(function (i) { return GCLASS[i]; },
                        S.sa ? function (i, j) { return rowPick(i) && colPick(j); } : null);
    var cell = function (a, b) { return agg[a + '>' + b]; };
    HOST.appendChild(el('p', 'sum',
      (S.sa && S.sa !== S.sb
        ? rows.length + ' classes in ' + S.sa + ' reaching ' + cols.length + ' in ' + S.sb
        : rows.length + ' classes' + (S.sa ? ' in ' + S.sa : '')) +
      ' · like groups collapsed across sites · click a cell'));
    if (S.sa && S.sa !== S.sb) {
      HOST.appendChild(el('p', 'sum dens',
        'Rows are ' + S.sa + ', columns are ' + S.sb + ' — only the crossing you clicked. '
        + 'Neither site\'s internal reach is on this page.'));
    }
    var split = realizationSplit(rows, cols, cell);
    var ll = ladderLine(split);
    if (ll) HOST.appendChild(ll);
    HOST.appendChild(mx(rows, cols, cell,
      function (c) { return c.toLowerCase(); },
      function (c) {
        var g = Object.keys(GCLASS)
                  .filter(function (i) { return GCLASS[i] === c && (rowPick(i) || colPick(i)); })
                  .map(function (i) { return D.nodes[i].name; });
        return c + '\n' + g.length + ' group(s): ' + g.join(', ');
      },
      function (a, b) { S.ca = a; S.cb = b; S.level = 'g'; S.q = ''; pushUrl(); paint(); },
      unrAny, null, null, null, true));
    HOST.appendChild(legend('class', split));
  }

  /* Roll group-pair edges up to whatever bucket `key` names. A bucket pair is
     permitted if ANY underlying group pair is — an aggregate that hid a single
     permitted path would be worse than no aggregate at all. */
  function aggregate(key, allow) {
    var out = {};
    (D.edges || []).forEach(function (e, ei) {
      if (allow && !allow(e.s, e.d)) return;
      var k = key(e.s) + '>' + key(e.d);
      (out[k] || (out[k] = [])).push(ei);
    });
    return out;
  }
  function unrAny(es) { return es.some(function (i) { return D.edges[i].unrestricted; }); }
  function uniq(a) { var s = {}; return a.filter(function (x) { return !s[x] && (s[x] = 1); }); }

  /* ------------------------------------------------------- THE LADDER
     `Edge.realization` has been on the wire since netvis existed and was read
     NOWHERE, so 98 of zagros's 124 permitted group pairs — 79% of the strokes on
     the aggregate levels — were unpopulated design drawn identically to live
     reach. A picture that cannot tell those apart is not a reachability
     surface, it is a rulebook with a grid around it.

     Four states, and the distinction that matters most is the one that was
     invisible:
       realized  a machine is connected at both ends RIGHT NOW
       dormant   both groups have members, none up
       latent    an endpoint group has NO MEMBERS — nothing has ever traversed
                 this, and it is one enrolment away from carrying everything
       unknown   the peer list was not read. Not a rung: the absence of one.

     latent vs dormant is not pedantry, it is two different jobs: latent is
     membership bookkeeping (someone declared a group and never populated it),
     dormant is operational (the machines exist and are off). Narrating all
     non-live cells as "dormant" is wrong for 98 of the 103 that are not live. */
  var REAL_RANK = { latent: 0, dormant: 1, realized: 2 };
  /* A bucket cell is permitted if ANY underlying pair is, so its rung is the
     STRONGEST rung underneath — an aggregate that reported a live path as
     latent would understate reach, the dangerous direction. But one unread
     edge caps the whole cell at `unknown`: a rung claimed from partial
     evidence is the same lie the server-side fix removed. */
  function realizationOf(es) {
    // No rules under the cell — a bridged site pair. There is no rung to
    // report and inventing the default one would say "latent", which is the
    // opposite of true: these are live machines, not an empty group.
    if (!es.length) return null;
    var best = 'latent', unknown = false;
    for (var i = 0; i < es.length; i++) {
      var r = D.edges[es[i]].realization;
      if (r !== 'latent' && r !== 'dormant' && r !== 'realized') { unknown = true; continue; }
      if (REAL_RANK[r] > REAL_RANK[best]) best = r;
    }
    return unknown ? 'unknown' : best;
  }
  var REAL_WORD = {
    realized: 'both ends have a connected machine right now',
    dormant: 'both groups are populated, nothing connected',
    latent: 'an endpoint group has NO members — nothing traverses this yet',
    unknown: 'the peer list was not read, so this is UNKNOWN, not dead'
  };

  /* The split over the group pairs THIS matrix draws, not the goatnet total.
     A scoped drill quoting the fleet figure would be describing a different
     picture from the one on screen. Distinct EDGES, because that is the unit
     the sentence names — one group pair can appear in many bucket cells. */
  function realizationSplit(rows, cols, permits) {
    var seen = {}, n = { unknown: 0, latent: 0, dormant: 0, realized: 0 };
    rows.forEach(function (a) {
      cols.forEach(function (b) {
        (permits(a, b) || []).forEach(function (ei) {
          if (seen[ei]) return;
          seen[ei] = 1;
          var r = D.edges[ei].realization;
          n[r === 'latent' || r === 'dormant' || r === 'realized' ? r : 'unknown']++;
        });
      });
    });
    return n;
  }
  function ladderLine(n) {
    var total = n.unknown + n.latent + n.dormant + n.realized;
    if (!total) return null;
    if (n.unknown === total) {
      return el('p', 'sum dens',
        'All ' + total + ' permitted group pairs here are drawn at one strength because the peer '
        + 'list was NOT READ: whether any of them carries traffic is unknown, which is a different '
        + 'claim from dead. The rules are readable; the machines are not.');
    }
    var head = n.realized + ' of ' + total + ' permitted group pairs carry traffic today'
      + (n.latent ? '; ' + n.latent + ' are LATENT — an endpoint group has no members at all' : '')
      + (n.dormant ? ', ' + n.dormant + ' populated with nothing up' : '')
      + (n.unknown ? '. ' + n.unknown + ' could not be placed on the ladder (membership unread)' : '')
      + '.';
    var box = el('div', 'lead');
    box.appendChild(el('p', 'sum key', head));
    box.appendChild(why('how the three strengths are drawn', [
      'Cells are drawn at three strengths in that order: solid carries traffic today, '
      + 'half-filled is populated with nothing connected, hollow is design that has never '
      + 'been populated.',
      'latent and dormant are not the same problem. Latent is membership bookkeeping — a '
      + 'group was declared and never filled. Dormant is operational — the machines exist '
      + 'and are switched off. Calling them all dormant is wrong for most of them.']));
    return box;
  }

  // ---------------------------------------------------------- L2 group×group
  function L2() {
    var seen = {};
    (D.edges || []).forEach(function (e) { seen[e.s] = 1; seen[e.d] = 1; });
    var all = [];
    D.nodes.forEach(function (nd, i) { if (seen[i]) all.push(i); });

    /* Scoped by the pair we drilled from — and DIRECTIONALLY, for the same
       reason as L1. Both scopes used to be applied as `=== a || === b` to one
       shared axis list, so a cross-site or cross-class drill re-admitted the
       two diagonals it had just navigated past. Rows are the source side of
       the cell that was clicked, columns the destination side. */
    var q = S.q.toLowerCase();
    var side = function (site, cls) {
      return all.filter(function (i) {
        return (!site || GSITE[i] === site)
            && (!cls || GCLASS[i] === cls)
            && (!q || D.nodes[i].name.toLowerCase().indexOf(q) >= 0);
      });
    };
    var rows = side(S.sa, S.ca), cols = side(S.sb, S.cb);

    var out = {};
    rows.forEach(function (i) {
      out[i] = cols.filter(function (j) { return BYPAIR[i + '>' + j]; }).length;
    });
    rows.sort(function (a, b) { return out[b] - out[a] || D.nodes[a].name.localeCompare(D.nodes[b].name); });
    cols.sort(function (a, b) { return D.nodes[a].name.localeCompare(D.nodes[b].name); });

    var cell = function (a, b) { return BYPAIR[a + '>' + b]; };
    var directional = (S.sa && S.sa !== S.sb) || (S.ca && S.ca !== S.cb);
    HOST.appendChild(el('p', 'sum',
      (directional
        ? rows.length + ' groups reaching ' + cols.length + ' groups'
        : rows.length + ' groups')
      + ' · ' + (D.edges || []).length + ' permitted group pairs · click a cell for the machines'));
    if (directional) {
      HOST.appendChild(el('p', 'sum dens',
        'Rows are ' + (S.ca || 'any class') + ' in ' + (S.sa || 'any site')
        + ', columns are ' + (S.cb || 'any class') + ' in ' + (S.sb || 'any site')
        + ' — the crossing you clicked, and nothing else.'));
    }
    var split = realizationSplit(rows, cols, cell);
    var ll = ladderLine(split);
    if (ll) HOST.appendChild(ll);

    HOST.appendChild(mx(rows, cols, cell,
      function (i) { return short(D.nodes[i].name); },
      function (i) {
        var nd = D.nodes[i];
        return nd.name + '\n' + (nd.purpose || 'no declared purpose') +
               '\nmembers: ' + (nd.members === null || nd.members === undefined ? 'not read' : nd.members);
      },
      function (a, b) { S.ga = a; S.gb = b; S.level = 'peers'; S.q = ''; pushUrl(); paint(); },
      function (es) { return es.some(function (i) { return D.edges[i].unrestricted; }); },
      null, null, null, true));
    HOST.appendChild(legend('group', split));
  }
  /* Group names are already `<SITE>-<SUFFIX>`; drop the family prefix on the
     axis because the family is the block you are in, and repeating it on all
     69 labels costs width without adding information. Full name on hover. */
  function short(name) {
    for (var i = 0; i < FAMILIES.length; i++) {
      if (name.indexOf(FAMILIES[i] + '-') === 0) {
        return FAMILIES[i].split('-').map(function (w) { return w[0]; }).join('').toLowerCase() +
               '/' + name.slice(FAMILIES[i].length + 1).toLowerCase();
      }
    }
    return name.toLowerCase();
  }

  /* ------------------------------------------------------- L3 peer × peer
     Rows are the machines in the group you came from. COLUMNS ARE EVERY
     MACHINE THEY CAN ACTUALLY REACH — not merely the members of the group on
     the other side of the cell you clicked.

     This distinction is the whole correctness of the level. A peer belongs to
     several groups at once, so its reach is a property of THE PEER, not of the group pair
     you happened to arrive through. Restricting the columns to the selected
     destination group would hide everything these machines reach via their
     other memberships and would understate real reach — the dangerous
     direction for a security surface to be wrong in.

     So the cell says which it is:
       via-pair   permitted by the group pair you clicked (solid)
       via-other  permitted ONLY through some other membership (hatched)
     Both are real permissions. The second is the one that surprises people. */
  function L3() {
    var peers = D.peers || [];
    var q = S.q.toLowerCase();
    var keep = function (i) {
      return !q || ID[i].short.toLowerCase().indexOf(q) >= 0 || peers[i].name.toLowerCase().indexOf(q) >= 0;
    };

    var rows = [];
    peers.forEach(function (p, i) { if ((p.groups || []).indexOf(S.ga) >= 0 && keep(i)) rows.push(i); });

    // Columns: the union of everything the rows reach, however mediated.
    var colSet = {};
    rows.forEach(function (a) {
      /* Columns are NOT filtered. keep() used to apply here too, so typing a
         machine name — the one gesture this level exists for — removed every
         destination whose name did not also match, and the page then reported
         "1 machine reaching 0 machines" for a machine that reaches 108, over a
         universe of 0 cells (NaN%). The filter picks which machines you are
         ASKING ABOUT; it must never narrow the answer. */
      peers.forEach(function (p, b) { if (a !== b && PERMITS[a + '>' + b]) colSet[b] = 1; });
    });
    var cols = Object.keys(colSet).map(Number);

    // Was this pair permitted BY the group pair we drilled through?
    var viaPair = function (a, b) {
      var es = PERMITS[a + '>' + b];
      if (!es) return false;
      return es.some(function (ei) { return D.edges[ei].s === S.ga && D.edges[ei].d === S.gb; });
    };

    var inDest = 0, beyond = 0;
    rows.forEach(function (a) {
      cols.forEach(function (b) {
        if (!PERMITS[a + '>' + b]) return;
        if (viaPair(a, b)) inDest++; else beyond++;
      });
    });

    var anyport = 0, total = 0;
    rows.forEach(function (a) {
      cols.forEach(function (b) {
        var es = PERMITS[a + '>' + b];
        if (!es) return;
        total++;
        if (es.some(function (i) { return D.edges[i].unrestricted; })) anyport++;
      });
    });
    HOST.appendChild(el('p', 'sum',
      rows.length + ' machines in ' + D.nodes[S.ga].name + ' · reaching ' + cols.length + ' machines · ' +
      inDest + ' pair(s) via ' + D.nodes[S.gb].name + ', ' + beyond +
      ' via their other group memberships · ' +
      // Stated, not coloured: this is ~79% fleet-wide, so painting it would
      // make every cell the same and say nothing.
      anyport + ' of ' + total + ' on ANY PORT' +
      (total > anyport ? ' — the ' + (total - anyport) + ' port-scoped one(s) are marked' : '')));

    /* Say how full the grid is. A mostly-dark matrix is the honest answer here
       — 92 developer machines reaching 108 destinations is 85.8% NOT permitted,
       because developers reach the infra hosts and little else — but "mostly
       empty" and "mostly broken" look identical unless the page says which.
       The dormant share matters too: a permitted pair with one end offline is
       drawn dimmed, and it is still a GRANT. */
    /* Self-pairs are drawn inert, so counting them made the sentence whose job
       is trusting the picture disagree with the picture (measured: "648 of 648
       (100.0%)" over 642 lit + 6 inert). */
    var selfCells = 0;
    rows.forEach(function (a) { cols.forEach(function (b) { if (a === b) selfCells++; }); });
    var cells = rows.length * cols.length - selfCells;
    var liveN = 0;
    rows.forEach(function (a) {
      cols.forEach(function (b) {
        if (PERMITS[a + '>' + b] && peers[a].connected && peers[b].connected) liveN++;
      });
    });
    if (!cells) return;
    HOST.appendChild(el('p', 'sum dens',
      total + ' of ' + cells + ' cells permitted (' + (100 * total / cells).toFixed(1) + '%) — '
      + 'the rest are denials, not gaps in the data. '
      + liveN + ' of the ' + total + ' have both ends connected right now; the other '
      + (total - liveN) + ' are drawn dimmed and are STILL grants.'));

    if (!rows.length) {
      HOST.appendChild(el('p', 'warn',
        D.nodes[S.ga].name + ' has no members' + (q ? ' matching “' + S.q + '”' : '') +
        '. The permission is real and takes effect the moment a machine joins.'));
      return;
    }
    if (!cols.length) {
      HOST.appendChild(el('p', 'warn',
        'These machines can reach nothing' + (q ? ' matching “' + S.q + '”' : '') + '.'));
      return;
    }

    HOST.appendChild(mx(rows, cols,
      function (a, b) { return PERMITS[a + '>' + b]; },
      peerLabel,
      function (i) { return ID[i].full; },
      function (a, b) { S.pa = a; S.pb = b; S.level = 'p'; pushUrl(); paint(); },
      function (es) { return es.some(function (i) { return D.edges[i].unrestricted; }); },
      function (a, b) { return peers[a].connected && peers[b].connected; },
      viaPair,
      function (i, axis) {
        var g = axis === 'row' ? S.ga : S.gb;
        return (peers[i].groups || []).indexOf(g) >= 0;
      },
      undefined,
      identityCard));
    HOST.appendChild(legend());
  }

  /* The legend RENDERS REAL CELLS, with the same classes the matrix uses.
     It was hand-drawn HTML with hardcoded grey swatches, which made it a COPY
     of the encoding — and the copy drifted immediately: cells turn green when
     a pair is unrestricted, nearly every pair on some drills is, and the
     legend showed only the grey variants and never mentioned unrestricted at
     all. A legend that can disagree with the picture is worse than none,
     because it is believed. Built from the same class names, it cannot. */
  /* --------------------------------------------------------------- inert
     Rules that are evaluated and grant nothing, because an endpoint group is
     empty — and what each one does the moment a machine joins.

     VOCABULARY IS DELIBERATE AND BORROWED. "Inert" over the coined "armed but
     asleep"; "Empty Groups" is Tufin's own check name (C08), AlgoSec calls the
     same thing "Empty Objects", and AWS Security Hub's NetworkFirewall.6 says
     it best: "an empty stateless rule group, when present in a firewall
     policy, might give the impression that the rule group will process
     traffic. However, when the stateless rule group is empty, it does not
     process traffic." The academic term for the class is IRRELEVANCE
     (Al-Shaer & Hamed, Taxonomy of Conflicts in Network Security Policies,
     IEEE Comm. Mag. 2006) — good in a design doc, wrong in a UI.

     WHAT IS OURS: all three of those detect the empty OBJECT and none of them
     reports the resulting inert RULE. You must cross-reference the object
     finding against the rulebase yourself to learn "rule 17 cannot fire". The
     join is the contribution, and on this goatnet it is the headline: 51 of 63
     enabled rules are structurally unable to fire.

     This is a different question from the other three views. /policy is what
     is declared, / is what is live, /findings is what is wrong. This is what
     happens NEXT, with no deploy, no review and no announcement: an empty
     group is one enrolment away from every grant already written against it.

     The ratio at the top is the number to watch. Rules scale with SCOPES, not
     peers — which is the property that makes a large fleet tractable and also
     the property that makes a small goatnet look absurdly over-ruled. A high
     inert fraction is only a problem on a goatnet that is actually loaded;
     stating it is how that stays a judgement rather than a discovery. */
  function LA() {
    var nodes = D.nodes || [], edges = D.edges || [], rules = D.rules || [];
    var mem = function (i) { var m = (nodes[i] || {}).members; return m === null || m === undefined ? null : m; };

    // A rule is load-bearing when at least one edge it produces has members at
    // BOTH ends. Anything else is armed and asleep.
    var live = {};
    edges.forEach(function (e) {
      if (mem(e.s) && mem(e.d)) (e.rules || []).forEach(function (ri) { live[ri] = 1; });
    });
    var total = rules.length, bearing = Object.keys(live).length;
    var inert = total - bearing;
    var pct = total ? Math.round(100 * inert / total) : 0;

    HOST.appendChild(el('p', 'sum',
      bearing + ' of ' + total + ' enabled rules can currently fire. '
      + inert + ' are INERT (' + pct + '%) — an endpoint group has no members, so the '
      + 'rule is evaluated and grants nothing, until it grants everything it '
      + 'always said it would.'));

    /* HOW MUCH DECLARED MACHINERY IS BEING CARRIED, stated as an inventory
       rather than left to be discovered. Everything here is in `counts`, which
       the server has always shipped and the client never read. */
    var c = D.counts || {};
    var inv = el('table', 'rules carry');
    var ih = el('tr');
    /* A right-aligned number under a left-aligned header is two columns, not
       one — the eye has nothing to travel down. `count` sat ~110px left of
       every value beneath it. */
    ['carrying', 'count', 'of', 'share'].forEach(function (h, i) {
      ih.appendChild(el('th', i ? 'n' : null, h));
    });
    inv.appendChild(ih);
    function line(label, n, of) {
      if (n === undefined || n === null || of === undefined) return;
      var tr = el('tr', of && n / of >= 0.5 ? 'hot' : null);
      tr.appendChild(el('td', null, label));
      tr.appendChild(el('td', 'n', String(n)));
      tr.appendChild(el('td', 'n', String(of)));
      tr.appendChild(el('td', 'n', of ? Math.round(100 * n / of) + '%' : '—'));
      inv.appendChild(tr);
    }
    line('groups with no members', c.groups_empty, c.groups);
    line('groups referenced by no rule at all', c.groups_in_no_rule, c.groups);
    line('slots held for a family that lacks them', c.groups_reserved, c.groups);
    line('permitted pairs that are latent (an end is empty)', c.latent, c.pairs);
    line('permitted pairs realized (both ends connected)', c.realized, c.pairs);
    line('rules that grant nothing today', inert, total);
    HOST.appendChild(inv);

    /* The judgement that keeps the inventory from being read as an alarm.
       Rules scale with SCOPES, not peers — the property that makes a large
       fleet tractable, and the same property that makes a lightly-populated
       goatnet look absurdly over-ruled. An empty declared group costs one YAML
       row and one NetBird object; that is the cheap half. The expensive half
       is the rules written against it, which arm on population. */
    HOST.appendChild(why('is a high share here wrong?', [
      'Not automatically. Rules scale with SCOPES, not with peers, so a lightly-populated '
      + 'goatnet is expected to carry declared machinery ahead of its deployment.',
      'What matters is the direction of travel on a goatnet that IS loaded — and which of '
      + 'the sleeping rules are ANY-PORT rather than port-scoped. Those are the marked '
      + 'rows below.']));

    /* Wake-on-population, per empty group: what its FIRST member unlocks.
       `latent_paths` is the server's own count of unrestricted pairs that flip
       to realized when this group is populated — already computed, never
       drawn. */
    var rows = [];
    nodes.forEach(function (n, i) {
      if (mem(i)) return;                       // populated, or not read
      var out = edges.filter(function (e) { return e.s === i; });
      var inn = edges.filter(function (e) { return e.d === i; });
      if (!out.length && !inn.length) return;   // in no rule at all
      var pairs = 0, anyport = 0;
      out.forEach(function (e) { var m = mem(e.d) || 0; pairs += m; if (e.unrestricted) anyport += m; });
      inn.forEach(function (e) { var m = mem(e.s) || 0; pairs += m; if (e.unrestricted) anyport += m; });
      rows.push({ i: i, name: n.name, out: out.length, in: inn.length,
                  pairs: pairs, anyport: anyport, lp: n.latent_paths || 0 });
    });
    rows.sort(function (a, b) { return b.anyport - a.anyport || b.pairs - a.pairs; });

    if (!rows.length) {
      HOST.appendChild(el('p', 'sum', 'No empty group is referenced by any rule.'));
      return;
    }

    /* Deliberately NOT summed into a headline. Each row is what THAT group's
       first member unlocks, counted independently; two empty groups that face
       each other both count the pair they would create together, so a sum
       would overstate. A number that has to be walked back later is worse than
       no number — the per-group figures are the actionable ones anyway. */
    var hot = rows.filter(function (r) { return r.anyport > 0; }).length;
    HOST.appendChild(el('p', 'sum key',
      rows.length + ' empty groups are referenced by a rule, ' + hot + ' of them by an '
      + 'ANY-PORT rule. Each row is what that group\'s FIRST member unlocks.'));
    HOST.appendChild(why('why these are not added up', [
      'Each row is counted on its own — no policy change, no deploy, no announcement. '
      + 'They are deliberately not summed: two empty groups facing each other would each '
      + 'count the pair they create together, so a total would overstate.',
      'A number that has to be walked back later is worse than no number.']));

    var t = el('table', 'rules armed');
    var hd = el('tr');
    ['group', 'rules out', 'rules in', 'pairs on first member', 'of which ANY PORT', 'unrestricted paths woken']
      .forEach(function (h, i) { hd.appendChild(el('th', i ? 'n' : null, h)); });
    t.appendChild(hd);
    rows.forEach(function (r) {
      var tr = el('tr', r.anyport ? 'hot' : null);
      tr.appendChild(el('td', null, r.name));
      tr.appendChild(el('td', 'n', String(r.out)));
      tr.appendChild(el('td', 'n', String(r.in)));
      tr.appendChild(el('td', 'n', String(r.pairs)));
      tr.appendChild(el('td', 'n', String(r.anyport)));
      tr.appendChild(el('td', 'n', String(r.lp)));
      t.appendChild(tr);
    });
    HOST.appendChild(t);
  }

  /* ----------------------------------------------------------- LG the gate
     The declaration's assertions about itself, run against LIVE state.

     The other four lenses describe. This one JUDGES, and it is the half of
     the product nobody in the category has: Tailscale's ACL tests are a gate
     on POLICY WRITES, so removing a tag can silently violate a passing test
     (their #8324, open since 2023). Membership change IS the out-of-band
     change, and no write-time gate of any design fires when nothing is being
     written. So this page recomputes on every read, and a timer on the mgmt
     host reads it whether or not anyone is looking.

     Three verdicts, and UNDETERMINED caps the roll-up: an assertion nobody
     could decide is not an assertion anybody passed. */
  function LG() {
    var R = REPORT.report || {};
    var out = R.outcomes || [];
    var word = { satisfied: 'holds', baselined: 'known, recorded', violated: 'VIOLATED', undetermined: 'UNDETERMINED' };

    if (!out.length) {
      HOST.appendChild(el('p', 'warn',
        'This goatnet declares no assertions. That is not a pass — nothing is being '
        + 'checked. Add a `netbird_acl_tests` block for ' + (R.site || 'this site')
        + ' in ops/ansible/netbird_acl_matrix.yml.'));
      return;
    }

    /* Lead with the number that moves. Not a spreadsheet of outcomes — the
       category is littered with those and nobody opens them twice. */
    HOST.appendChild(el('p', 'sum gv-' + R.verdict,
      R.verdict === 'satisfied'
        ? 'Every one of the ' + out.length + ' assertions this goatnet declares holds against live state.'
        : R.verdict === 'baselined'
          ? out.length + ' assertions checked. ' + R.baselined + ' are known violations, already recorded and not counted against the gate; nothing new is broken.'
          : R.verdict === 'violated'
            ? R.violated + ' of ' + out.length + ' assertions are VIOLATED against live state.'
            : R.undetermined + ' of ' + out.length + ' assertions could not be DECIDED, which caps the whole result — an unknown is not a pass.'));

    HOST.appendChild(el('p', 'sum dens',
      R.satisfied + ' hold · ' + R.violated + ' violated · ' + R.baselined
      + ' baselined · ' + R.undetermined + ' undetermined  ·  checked against '
      + (REPORT.report && REPORT.report.source === 'live' ? 'LIVE state' : 'the declaration')));
    HOST.appendChild(why('what these assertions are, and when they run', [
      'The quantifier is ' + R.quantifier + '.',
      'The same assertions gate the pull request against the declaration. This page is the '
      + 'half that fires when MEMBERSHIP changes and nothing is being written — no file is '
      + 'edited, no review happens, and a write-time gate cannot see it.',
      'They are declared in ops/ansible/netbird_acl_matrix.yml under netbird_acl_tests, and '
      + '`goat acl test` runs them without a network.']));

    if ((R.baseline_now_passing || []).length) {
      HOST.appendChild(el('p', 'warn',
        'The ratchet can tighten: ' + R.baseline_now_passing.length
        + ' baselined assertion(s) now pass — ' + R.baseline_now_passing.join('; ')
        + '. Delete the baseline in netbird_acl_matrix.yml. A baseline nobody removes '
        + 'is a baseline that has become permission.'));
    }

    var rank = { undetermined: 0, violated: 1, baselined: 2, satisfied: 3 };
    out.slice().sort(function (a, b) { return rank[a.verdict] - rank[b.verdict]; })
      .forEach(function (o) {
        var card = el('article', 'find gv-' + o.verdict);
        var h = el('div', 'fh');
        h.appendChild(el('span', 'sev', word[o.verdict] || o.verdict));
        h.appendChild(el('span', 'fid', o.check));
        h.appendChild(el('span', 'blast', o.checked_pairs + ' pair(s) checked'));
        card.appendChild(h);
        card.appendChild(el('p', 'claim', o.name));

        /* An undetermined outcome says WHOSE fault it is. A malformed row and
           a broken mesh are different jobs and grading them the same is how a
           typo becomes a security finding. */
        if (o.reason) {
          card.appendChild(el('p', 'ameta', 'undecidable — ' + o.reason.replace(/_/g, ' ')));
        }
        if (o.baseline) {
          card.appendChild(el('p', 'ameta',
            'recorded as a known violation since ' + o.baseline.violated_since + ' — ' + o.baseline.why));
        }

        (o.counterexamples || []).forEach(function (c) {
          var box = el('div', 'act');
          box.appendChild(el('pre', 'dy',
            c.src + '  →  ' + c.dst + ' : ' + c.port + '   (' + c.proto + ')'
            + (c.granted_by ? '\n  granted by rule: ' + c.granted_by : '')
            /* Contrast is what localises the fault. "Nothing reaches the
               breakglass" is a fact; "and this rule reaches it on 443" is a
               diagnosis. */
            + (c.nearest ? '\n  nearest: ' + c.nearest : '')));
          card.appendChild(box);
        });
        if (o.verdict === 'satisfied' && !(o.counterexamples || []).length) {
          card.appendChild(el('p', 'ameta',
            'no counterexample over the whole space — the search was for a violation, '
            + 'and empty is the pass.'));
        }
        HOST.appendChild(card);
      });
  }

  /* ------------------------------------------------------------- findings
     The surface's own judgement. Every one of these was computed on every
     request since netvis existed and thrown away before it reached a pixel,
     so the page reported no problems because it drew none — not because there
     were none. Each carries a claim, a blast radius and a remediation. */
  function LF() {
    var F = (D.findings || []).slice();
    if (!F.length) {
      HOST.appendChild(el('p', 'sum',
        'No findings — and that is a computed result, not an empty render.'));
      return;
    }
    var rank = { alarm: 0, review: 1, accepted: 2 };
    F.sort(function (a, b) {
      return (rank[a.severity] - rank[b.severity]) || (b.blast_radius - a.blast_radius);
    });
    var q = S.q.toLowerCase();
    if (q) F = F.filter(function (f) { return (f.claim || '').toLowerCase().indexOf(q) >= 0; });

    var n = function (sv) { return (D.findings || []).filter(function (f) { return f.severity === sv; }).length; };
    HOST.appendChild(el('p', 'sum',
      n('alarm') + ' alarm · ' + n('review') + ' review · ' + n('accepted') + ' accepted'
      + ' · ordered by blast radius, which is how many machine pairs the claim covers'));

    var wrap = el('div', 'finds');
    F.forEach(function (f) {
      var card = el('article', 'find sev-' + (f.severity || 'review'));
      var h = el('div', 'fh');
      h.appendChild(el('span', 'sev', f.severity || '?'));
      h.appendChild(el('span', 'fid', f.id || ''));
      /* Blast radius is the variable that varies most and matters most; it is
         the reason these are ordered rather than listed. */
      if (f.blast_radius !== undefined && f.blast_radius !== null) {
        h.appendChild(el('span', 'blast',
          f.blast_radius + ' ' + (f.blast_unit || 'affected')));
      }
      card.appendChild(h);
      card.appendChild(el('p', 'claim', f.claim || ''));

      var a = f.action;
      if (a) {
        var act = el('div', 'act');
        if (a.declare_yaml) {
          /* The fast track: the remediation is already computed as data, so a
             decision is one paste rather than hand-authored YAML. Rendered
             verbatim — this surface must never paraphrase a remediation. */
          var pre = el('pre', 'dy', a.declare_yaml);
          var cp = el('button', 'copy', 'copy');
          cp.addEventListener('click', function () {
            if (navigator.clipboard) navigator.clipboard.writeText(a.declare_yaml);
            cp.textContent = 'copied';
            setTimeout(function () { cp.textContent = 'copy'; }, 1200);
          });
          act.appendChild(cp);
          act.appendChild(pre);
        }
        var meta = [];
        if (a.recheck) meta.push('re-check: ' + a.recheck);
        if (a.doc) meta.push('doc: ' + a.doc);
        if (meta.length) act.appendChild(el('p', 'ameta', meta.join('   ·   ')));
        card.appendChild(act);
      }
      wrap.appendChild(card);
    });
    HOST.appendChild(wrap);
  }

  /* ------------------------------------------------------------- L4 one pair
     The end of the drill, and it did not exist. `paint()` called L4() and no
     such function was ever written, so the deepest level threw a ReferenceError
     and rendered a blank page under a correct breadcrumb — the failure looked
     like "no data" rather than "no code", which is why it survived.

     This is the level that answers the question the whole surface is for, for
     ONE pair: can these two talk, in which direction, on what protocol and
     ports, and WHICH rule permits it. Everything here is already in the payload
     the matrix drew from — no extra read. */
  /* Render one machine's responsible person, with the PROVENANCE of that claim
     stated on the row rather than implied by its presence.

     The contract comes from panoscope_core::peerowner and it is not decoration:
     a peer name is EVIDENCE about ownership, never a record of it. Four states,
     four different things a reader should do:

       registry  — roster handle AND a live identity. The recorded linkage.
                   Deep-links to the person page. Act on it.
       name      — handle parsed, no live identity matched. Org is known, the
                   person is not. A lead. Rendered muted and labelled
                   "unconfirmed" so it can never be mistaken for the first case.
       unmatched — the name encodes no principal. NOT "unowned": it means this
                   machine was never onboarded through the operator path (24 of
                   82 peers on ZAGROS's first live pass, all legitimately infra).
       unread    — we did not look. Says nothing about the machine.

     The two that a careless render would collapse are `unmatched` and `unread`,
     and collapsing them is exactly the mistake that hid netvis itself for three
     days: "we could not measure" shown as if it were a measurement. */

  /* WHO TO ASK WHEN THE NAME RESOLVES TO NOBODY.

     A machine owns no person: on zagros-prod 16 of 17 peers carry no principal
     in the name — Orins, relays, breakglass, gear at a forward scope — and
     each rendered `no principal in the name`. True, and useless, because the
     viewer still had nobody to contact.

     A machine is never scopeless though. Its groups name the scope it belongs
     to, and the mesh already names who administers a scope:
     <SCOPE>-PLATFORM-ADMIN. ADR 1124 makes that suffix the ONLY qualified
     cross-scope source, so it is definitionally the group permitted to act on
     another scope's machines.

     DRAWN ONLY WHEN THE PERSON LENS DID NOT ANSWER. A resolved owner is the
     stronger claim and stands alone; appending "and here are 4 admins" under a
     named person invites someone to page the wrong one. This is the fallback,
     and it is labelled `scope` — never `registry`, because it is derived from
     the mesh plane and not from the registry. */
  function stewardRow(sec, st, owner, consoleBase) {
    if (!st) return;
    if (owner && owner.attribution === 'registry') return;
    var box = el('div', 'owner-row steward');

    /* SAY WHEN THE SCOPE IS EVERYTHING. On a single-scope goatnet "scope ZAGROS"
       is tautological — every machine is in it and every machine gets the same
       admins — and the word "scope" implies a subdivision that does not exist
       there. On a goatnet with a forward site the same row genuinely narrows,
       so the label earns its keep; the difference has to be stated, not inferred. */
    box.appendChild(el('span', 'owner-role',
      st.scope ? (st.scope_is_whole_goatnet ? st.scope + ' (whole goatnet)' : 'scope ' + st.scope)
               : 'scope'));

    var ps = st.principals || [];
    if (!ps.length) {
      box.appendChild(el('span', 'owner-val muted', st.basis || 'no administering group'));
      box.appendChild(el('span', 'owner-prov', 'scope'));
      sec.appendChild(box);
      return;
    }

    /* PEOPLE, NOT DEVICES. The admin group's members are peers, so the raw list
       answers "who do I ask" with hardware — 11 entries on ZAGROS that are at
       most 5 humans, three of them one person's desktop, laptop and phone.
       Lead with the count of PEOPLE; the devices are detail. */
    var named = ps.filter(function (p) { return !p.unresolved; });
    var unres = ps.filter(function (p) { return p.unresolved; });
    var online = 0, devices = 0;
    ps.forEach(function (p) { online += p.devices_online || 0; devices += (p.devices || []).length; });

    var lead = named.slice(0, 3).map(function (p) {
      return p.person_name || p.handle || 'unnamed';
    }).join(', ');
    var extra = named.length - Math.min(3, named.length);
    if (extra > 0) lead += ' +' + extra;
    if (!named.length) lead = 'nobody resolved';
    box.appendChild(el('span', 'owner-val', lead));
    box.appendChild(el('span', 'owner-org',
      named.length + (named.length === 1 ? ' person' : ' people')
      + ' · ' + online + '/' + devices + ' devices online'));
    box.appendChild(el('span', 'owner-prov', 'scope'));
    sec.appendChild(box);

    /* THE FULL LISTING, REVEALABLE. Progressive disclosure, the same rule the
       rest of this page follows: lead with the number that moves, evidence
       beneath, shut until asked. Someone chasing a machine wants the roster
       right here rather than in another tab, but not in the way of the
       reachability answer they came for. */
    var body = [];
    named.forEach(function (p) {
      var line = el('p', 'steward-person');
      var who = p.person_name || p.handle || 'unnamed';
      if (p.id_console_path && consoleBase) {
        var a = el('a', 'owner-val', who);
        a.setAttribute('href', consoleBase.replace(/\/+$/, '') + p.id_console_path);
        a.setAttribute('rel', 'noopener');
        line.appendChild(a);
      } else {
        line.appendChild(el('span', 'owner-val', who));
      }
      line.appendChild(el('span', 'owner-org',
        p.devices_online + '/' + p.devices.length + ' online'));
      line.appendChild(el('span', 'steward-devices', p.devices.join(', ')));
      body.push(line);
    });
    if (unres.length) {
      /* Never folded into a named person: "we could not name who holds these"
         is a different fact from "this person holds them", and merging them
         would attribute machines to someone who may not hold them. */
      var u = unres[0];
      var line = el('p', 'steward-person');
      line.appendChild(el('span', 'owner-val muted', 'no principal resolved'));
      line.appendChild(el('span', 'owner-org', u.devices_online + '/' + u.devices.length + ' online'));
      line.appendChild(el('span', 'steward-devices', u.devices.join(', ')));
      body.push(line);
    }
    body.push(el('p', 'muted', st.basis));
    sec.appendChild(why('the ' + (named.length + (unres.length ? 1 : 0))
      + ' administering principals, and the devices they hold', body));

    /* A MACHINE IN TWO SCOPES HAS TWO SETS OF PEOPLE. A machine holding
       <HOST>-EXTERNAL-INFRA and EX-DEMO-POD both means the exercise scope's admins are as answerable as the goatnet's. Showing
       only the first was arbitrary group ordering deciding a responsibility
       answer — the exact defect this row exists to remove, reintroduced one
       level down. Each extra scope gets its own row. */
    (st.also_scopes || []).forEach(function (ex) {
      var b2 = el('div', 'owner-row steward');
      b2.appendChild(el('span', 'owner-role', 'also ' + ex.scope));
      var eps = ex.principals || [];
      var enamed = eps.filter(function (p) { return !p.unresolved; });
      if (!eps.length) {
        /* AN UNAPPOINTED SCOPE-LOCAL ADMIN IS NOT A GAP. The canonical group
           set is complete-by-prefix, and a scope-local PLATFORM-ADMIN exists so
           an activity CAN appoint an admin for its duration without making
           anyone a permanent goatnet-wide one. Most never appoint one. Saying
           only "has no members" reads as an accusation; saying where authority
           does sit is the answer someone came for. A scope nothing hosts still
           reads bare, because that one IS unanswerable. */
        b2.appendChild(el('span', 'owner-val muted',
          ex.admin_group
            ? (ex.host_scope
                ? ex.admin_group + ' unappointed — hosted on ' + ex.host_scope
                : ex.admin_group + ' has no members')
            : (ex.host_scope
                ? 'no local admin group — hosted on ' + ex.host_scope
                : 'no PLATFORM-ADMIN group declared')));
      } else {
        var eon = 0, edev = 0;
        eps.forEach(function (p) { eon += p.devices_online || 0; edev += (p.devices || []).length; });
        b2.appendChild(el('span', 'owner-val', enamed.slice(0, 3).map(function (p) {
          return p.person_name || p.handle || 'unnamed';
        }).join(', ') || 'nobody resolved'));
        b2.appendChild(el('span', 'owner-org',
          enamed.length + (enamed.length === 1 ? ' person' : ' people')
          + ' · ' + eon + '/' + edev + ' devices online'));
      }
      b2.appendChild(el('span', 'owner-prov', 'scope'));
      sec.appendChild(b2);
    });
  }

  function ownerRow(role, o, consoleBase) {
    var box = el('div', 'owner-row');
    box.appendChild(el('span', 'owner-role', role));
    if (!o) {
      box.appendChild(el('span', 'owner-val muted', 'not looked up'));
      return box;
    }
    var cls = 'owner-val' + (o.attribution === 'registry' ? '' : ' muted');
    /* The deep-link the relay-health card has had since July, now here. Only
       for the RECORDED linkage, and only when the server supplied a console
       base — an unconfirmed handle must not become a clickable person, and a
       guessed host must not become a link at all. */
    if (o.attribution === 'registry' && o.id_console_path && consoleBase) {
      var a = el('a', cls, o.person_name || o.person_email || o.handle || 'person');
      a.setAttribute('href', consoleBase.replace(/\/+$/, '') + o.id_console_path);
      a.setAttribute('rel', 'noopener');
      box.appendChild(a);
    } else {
      box.appendChild(el('span', cls,
        o.attribution === 'registry' ? (o.person_name || o.handle || 'resolved')
        : o.attribution === 'name' ? (o.handle || '?') + ' (unconfirmed)'
        : o.attribution === 'unmatched' ? 'no principal in the name'
        : 'not looked up'));
    }
    if (o.org) box.appendChild(el('span', 'owner-org', o.org));
    /* An owner who has been offboarded is the case a viewer most needs
       flagged: the machine is still on the mesh and the person is gone. */
    if (o.owner_disabled) box.appendChild(el('span', 'owner-flag', 'account disabled'));
    box.appendChild(el('span', 'owner-prov', o.attribution));
    return box;
  }

  function ownership(na, nb) {
    var sec = el('section', 'owners');
    sec.appendChild(el('h2', null, 'Who is responsible'));
    var pending = el('p', 'muted', 'resolving…');
    sec.appendChild(pending);
    HOST.appendChild(sec);

    fetch('/api/pair/' + encodeURIComponent(na) + '/' + encodeURIComponent(nb),
          { headers: { accept: 'application/json' } })
      .then(function (r) { return r.json(); })
      .then(function (d) {
        sec.removeChild(pending);
        if (!d || d.error) {
          sec.appendChild(el('p', 'warn', 'ownership unread: ' + ((d && (d.reason || d.error)) || 'no response')));
          return;
        }
        sec.appendChild(ownerRow(na, d.source_owner, d.id_console_base));
        stewardRow(sec, d.source_stewardship, d.source_owner, d.id_console_base);
        sec.appendChild(ownerRow(nb, d.destination_owner, d.id_console_base));
        stewardRow(sec, d.destination_stewardship, d.destination_owner, d.id_console_base);
        (d.ownership_notes || []).forEach(function (n) {
          sec.appendChild(el('p', 'muted', n));
        });
        sec.appendChild(why('what "registry" and "unconfirmed" mean here', [
          'A peer name is EVIDENCE about ownership, not a record of it. Under setup-key enrollment NetBird leaves peer.user_id empty, so the link from a machine to a person is the peer NAME (peer-<handle>-<site>-<date>). Stamping user_id was tried and is a dead end — NetBird validates it against its own users table and these goatnets have no person-scoped NetBird users, so binding an identity there breaks enrollment.',
          'registry means the handle matched the roster AND a live identity matched its email-hash — the recorded linkage, safe to act on. unconfirmed means the handle parsed and the org is known but no live identity matched; treat it as a lead and confirm before acting.',
          'no principal in the name is not "unowned" — it means the machine was never onboarded through the operator path, which is true of most infrastructure hosts. not looked up means the resolver was unreachable or off, which says nothing at all about the machine.',
        ]));
      })
      .catch(function (e) {
        if (sec.contains(pending)) sec.removeChild(pending);
        sec.appendChild(el('p', 'warn', 'ownership unread: ' + e));
      });
  }

  function L4() {
    var peers = D.peers || [], rules = D.rules || [];
    var a = S.pa, b = S.pb;
    if (!peers[a] || !peers[b]) {
      HOST.appendChild(el('p', 'warn', 'that pair is not in the current payload.'));
      return;
    }

    // Both directions, answered SEPARATELY. Rules are largely one-way, so
    // "can they talk" has two different answers and collapsing them lies.
    function paths(x, y) {
      return (PERMITS[x + '>' + y] || []).map(function (ei) { return D.edges[ei]; });
    }
    var fwd = paths(a, b), rev = paths(b, a);

    /* THE SHORT NAME IS AN AXIS AFFORDANCE AND IT DOES NOT BELONG HERE.
       It is the discriminator trimmed to 12 characters so 109 labels fit a
       gutter — which is right on a matrix and absurd on the one page whose
       entire purpose is to inspect these two machines. `relay-health-zagros-a`
       was rendering as `e/mgmt-host·relay-healt…` in its own heading. At the
       end of a drill, space is the one thing there is plenty of. */
    var NA = peers[a].name, NB = peers[b].name;
    HOST.appendChild(el('p', 'sum',
      NA + ' → ' + NB + ' · ' +
      (fwd.length ? fwd.length + ' path(s)' : 'NOT permitted') + '   ·   ' +
      NB + ' → ' + NA + ' · ' +
      (rev.length ? rev.length + ' path(s)' : 'NOT permitted')));

    /* WHO DO I ASK ABOUT THIS MACHINE.
       The matrix answers "can these two talk". A viewer who has drilled this
       far has a specific machine in hand and a follow-up question, and until
       now the page could not tell them the one thing needed to act on it:
       who is responsible for it.

       Fetched HERE rather than carried in /api/reach on purpose. The main
       payload covers the whole goatnet — 109 peers on zagros-prod — and baking
       a person, an email and an identity id into every one of them would make
       every page view a roster-wide PII read to answer a question about two
       machines. One drill, one lookup, two peers. */
    ownership(NA, NB);

    var wrap = el('div', 'pair');

    function direction(title, from, to, es) {
      var sec = el('section', 'dir');
      sec.appendChild(el('h2', null, title));
      if (!es.length) {
        /* An absent path is a real denial only because GET /api/policies is
           complete and we state that it was read. Same rule as the matrix. */
        sec.appendChild(el('p', 'none', D.policies_read
          ? 'No rule permits this direction.'
          : 'The policy set was not read — this is UNKNOWN, not a denial.'));
        wrap.appendChild(sec);
        return;
      }
      var t = el('table', 'rules');
      var hd = el('tr');
      ['permitting rule', 'source group', 'destination group', 'protocol · ports',
       'port-set', 'direction']
        .forEach(function (h) { hd.appendChild(el('th', null, h)); });
      t.appendChild(hd);

      /* One rule, one row. A bidirectional rule reaches a pair as both its
         forward and its reverse leg, so `infra ↔ infra (any port)` was listed
         twice, identically, in both directions — four rows saying one thing.
         Deduped on what the row actually DISPLAYS, so two genuinely different
         rules that happen to share a name still both appear. */
      var seenRow = {};
      es.forEach(function (e) {
        (e.rules || []).forEach(function (ri, k) {
          var r = rules[ri] || {};
          var rowKey = [ri, e.s, e.d].join('|');
          if (seenRow[rowKey]) return;
          seenRow[rowKey] = 1;
          var tr = el('tr', r.enabled === false ? 'off' : null);
          var nm = el('td', null, r.rule_name || r.policy || 'unnamed rule');
          if (r.enabled === false) nm.textContent += '  (disabled)';
          tr.appendChild(nm);
          tr.appendChild(el('td', null, (D.nodes[e.s] || {}).name || '?'));
          tr.appendChild(el('td', null, (D.nodes[e.d] || {}).name || '?'));
          /* Protocol and ports come off the RULE, never off a policy NAME —
             a name saying "any port" is prose, the rule is the fact. */
          tr.appendChild(el('td', null, r.constraint ||
            ((r.protocol || 'all') + ' · ' +
             (r.ports && r.ports.length ? r.ports.join(', ') : 'all ports'))));
          /* The warrant lens (ADR 1129). The ports above say WHAT is permitted;
             this says which named set that shape IS, so a reader sees
             `substrate` instead of recognising four numbers. Blank when the
             registry does not name the shape — that is a real answer, meaning
             the rule is not expressible in the registry yet, and inventing a
             label for it would be worse than the gap.

             `expires` is the reason this column earns its place: it is
             registry-only, stripped before the ACL is applied, so it appears
             NOWHERE else. Without it a grant that lapses on Friday is
             indistinguishable from a permanent one. */
          var ps = el('td', 'portset');
          if (r.port_set) {
            var nm2 = el('span', 'ps-name', r.port_set);
            if (r.port_set_purpose) nm2.title = r.port_set_purpose;
            ps.appendChild(nm2);
            if (r.port_set_owner) ps.appendChild(el('span', 'ps-own', ' · ' + r.port_set_owner));
            if (r.expires) {
              var ex = el('span', 'ps-exp', ' · expires ' + r.expires);
              ex.title = 'Registry-only (ADR 1129) — stripped before the ACL is applied, '
                + 'so this appears on no other surface.'
                + (r.grant_owner ? ' Ask: ' + r.grant_owner : '');
              ps.appendChild(ex);
            }
          } else {
            ps.appendChild(el('span', 'muted', '—'));
            ps.title = 'No port-set names this (protocol, ports) shape. Not expressible '
              + 'in the warrant registry yet.';
          }
          tr.appendChild(ps);
          tr.appendChild(el('td', null,
            r.bidirectional ? 'both ways'
              : ((e.rules_reverse || [])[k] ? 'reverse leg' : 'one way')));
          t.appendChild(tr);
        });
      });
      sec.appendChild(t);
      wrap.appendChild(sec);
    }

    direction(NA + '  →  ' + NB, a, b, fwd);
    direction(NB + '  →  ' + NA, b, a, rev);

    // Both machines in full: the derived name is for reading, the NetBird name
    // is the join key everything else uses, so both are shown.
    [a, b].forEach(function (i) {
      var sec = el('section', 'dir');
      sec.appendChild(el('h2', null, peers[i].name));
      var t = el('table', 'rules');
      function kv(k, v) {
        var tr = el('tr');
        tr.appendChild(el('th', null, k));
        tr.appendChild(el('td', null, v));
        t.appendChild(tr);
      }
      kv('netbird name', peers[i].name);
      kv('derived', ID[i].full || ID[i].short);
      kv('address', peers[i].ip || 'unknown');
      kv('connected now', peers[i].connected ? 'yes' : 'no');
      kv('operating system', peers[i].os || 'unknown');
      /* `groups 19, 49` — the raw indices into D.nodes, which are an artefact
         of how the payload is packed and mean nothing to anybody. The identity
         card had always resolved them; this table, on the deeper page, had
         not. */
      kv('groups', (peers[i].groups || []).map(function (g) {
        return (D.nodes[g] || {}).name || ('#' + g);
      }).join(', ') || 'none');
      (ID[i].unresolved || []).forEach(function (u) {
        kv(u, 'not resolvable on the mesh plane — it lives in the portal registry');
      });
      sec.appendChild(t);
      wrap.appendChild(sec);
    });

    HOST.appendChild(wrap);
  }

  /* One machine, every name it has. This is the "click in to see the long
     derived name and the real NetBird name" affordance: the axis shows the
     short form because 92 full names do not fit, but the short form is a
     display convenience and must never be the only thing on offer. */
  function identityCard(i) {
    var peers = D.peers || [], p = peers[i], id = ID[i];
    var old = document.querySelector('.idcard');
    if (old) old.parentNode.removeChild(old);

    var card = el('aside', 'idcard');
    var head = el('div', 'idhead');
    head.appendChild(el('h2', null, id.short));
    var x = el('button', 'idx', '\u00d7');
    x.setAttribute('aria-label', 'close');
    x.addEventListener('click', function () { card.parentNode.removeChild(card); });
    head.appendChild(x);
    card.appendChild(head);

    var t = el('table', 'rules');
    function kv(k, v, cls) {
      var tr = el('tr', cls);
      tr.appendChild(el('th', null, k));
      tr.appendChild(el('td', null, v));
      t.appendChild(tr);
    }
    // The three forms, always all three, in the order they are trusted:
    // the raw name is the join key, so it is first.
    kv('netbird name', id.netbird);
    kv('derived short', id.short);
    kv('derived full', id.full || id.short);
    kv('address', p.ip || 'unknown');
    kv('connected now', p.connected ? 'yes' : 'no');
    kv('operating system', p.os || 'unknown');
    kv('groups', (p.groups || []).map(function (g) {
      return (D.nodes[g] || {}).name || ('#' + g);
    }).join(', ') || 'none');

    /* WHAT THIS BOX IS, not what it is tagged with.
       ADR 1116 §2: a class is a FUNCTION and a box holds one class per
       function it performs; §3 says every gear family answers the same five
       questions — element / data-plane / telemetry / command-intake /
       arbiter. The card used to list the raw groups and stop, so a machine
       holding SENSOR-FEED and SENSOR-TASKING read as two unrelated strings
       with nothing saying they are two facets of ONE sensor.

       The "does not" half is deliberate wording. §2 means an absent slot is a
       function the box does not perform — the ordinary case, NOT a gap. This
       surface has already been corrected once this week for treating
       declared-and-empty as an alarm; "missing" would repeat it. */
    (p.gear || []).forEach(function (fam) {
      var parts = (fam.facets || []).map(function (f) { return f.slot; });
      var line = parts.join(' + ');
      if ((fam.slots_absent || []).length) {
        line += '   ·   does not: ' + fam.slots_absent.join(', ');
      }
      kv('gear · ' + fam.family, line);
    });

    // Every dimension of the grammar, each marked resolved or NOT — an
    // unresolved dimension is reported, never guessed (ADR 1043).
    var m = id.mapping || {};
    Object.keys(m).forEach(function (k) {
      var v = m[k];
      var unresolved = v === null || v === undefined || v === '' || v === 'unresolved';
      kv(k, unresolved ? 'not resolvable on the mesh plane — it lives in the portal registry' : v,
         unresolved ? 'off' : null);
    });
    card.appendChild(t);
    HOST.appendChild(card);
  }

  /* THE LEGEND IS A REFERENCE, NOT A PREAMBLE. It is read once, on a
     reader's first visit, and never again — but it was eleven rows of prose
     permanently occupying more vertical space than the matrix it describes.
     Shut by default, one click away, and it still renders REAL CELLS with the
     matrix's own classes so it cannot drift from the picture. */
  function legend(level, split, bridged) {
    var box = el('details', 'why legend-box');
    box.appendChild(el('summary', null, 'legend — what the cells mean'));
    var L = el('div', 'legend');
    box.appendChild(L);
    function swatch(cls) {
      var s = document.createElementNS(NS, 'svg');
      s.setAttribute('width', 15); s.setAttribute('height', 15);
      s.setAttribute('class', 'netvis-sw');
      s.appendChild(sv('rect', { class: cls, x: 1, y: 1, width: 13, height: 13 }));
      return s;
    }
    function row(cls, text) {
      var k = el('span', 'key');
      k.appendChild(swatch(cls));
      k.appendChild(el('span', 'kt', text));
      L.appendChild(k);
    }
    function note(text) {
      var a = el('span', 'key');
      a.appendChild(el('span', 'kt', text));
      L.appendChild(a);
    }

    /* AGGREGATE LEVELS carry three encodings: permitted or not, how REALIZED
       the permit is, and whether any underlying rule is unrestricted.
       Realization is the ladder that was on the wire and drawn nowhere —
       without it 79% of these strokes were unpopulated design rendered
       identically to live reach. Mediation is a peer fact and is still not
       drawn here, so the legend must not claim it. */
    if (level === 'site' || level === 'class' || level === 'group') {
      row('c on m-pair unr r-realized', 'permitted, ANY port — a machine is connected at both ends');
      row('c on m-pair unr r-dormant',  'permitted, ANY port — both groups populated, nothing up');
      row('c on m-pair unr r-latent',   'permitted, ANY port — LATENT: an endpoint group has no members, so nothing traverses it yet');
      row('c on m-pair r-realized',     'PORT-SCOPED and live — colour marks the rare case where a rule restricts protocol or ports');
      row('c on m-pair r-latent',       'PORT-SCOPED and latent');
      if (split && split.unknown) {
        row('c on m-pair unr r-unknown',
            'permitted — membership was NOT READ, so where this sits on the ladder is unknown, not dead');
      }
      if (bridged) {
        row('c on m-other',
            'NO RULE connects these sites — machines hold groups in both, so a machine of one is also a machine of the other. Click for the machines.');
      }
      row('c',               'no rule permits it');
      note('Strength is the ladder, not importance: solid carries traffic today, hollow is written and unpopulated. A latent cell is a real grant — it fires the moment the group gains a member.');
      note(level === 'group'
        ? 'A cell lights if any rule permits that group pair. Click it for the machines.'
        : 'A cell lights if ANY underlying group pair is permitted — an aggregate that hid one permitted path would be worse than no aggregate. Its rung is the STRONGEST rung underneath, for the same reason. Click to drill in.');
      return box;
    }

    /* The peer level, where mediation and liveness are both real.

       These four rows used to read `connected now` / `not connected`, and
       `not connected` was the wrong words for the state it names. Liveness
       here is a property of the two MACHINES — `isLive` is
       `peers[a].connected && peers[b].connected`, each one's netbird session
       — but a cell on this matrix is read as a claim about the PAIR, so
       "not connected" says "these two cannot reach each other". That is
       precisely what an EMPTY cell means, and a dimmed cell means the
       opposite: the grant is real and one end happens to be switched off.
       The legend was handing the same words to a denial and to a grant.

       The aggregate levels above already avoid this by naming the STATE
       ("both groups populated, nothing up") rather than describing the
       connection. These now do the same. */
    row('c on m-pair live unr',    'via the selected group pair · both ends online');
    row('c on m-pair dormant unr', 'via the selected group pair · PERMITTED, an end is offline');
    row('c on m-other live unr',   'via another membership · both ends online');
    row('c on m-other dormant unr','via another membership · PERMITTED, an end is offline');
    row('c on m-pair live',        'PORT-SCOPED — the rare case, coloured because it is rare');
    row('c on m-other live',       'PORT-SCOPED, via another membership');
    note('A dimmed cell is a GRANT. Only an EMPTY cell is a denial — dimmed means the rule '
       + 'permits the pair and one of the two machines is not on the mesh right now.');
    note('\u00b7  a machine marked with a dot is NOT a member of the selected group — it is here transitively');
    return box;
  }

  // ---------------------------------------------------------------- matrix
  /* `selfIsReal` — whether the diagonal carries data.
     At the aggregate levels it DOES and it is the main event: nearly every
     permitted pair on this goatnet is within one site and within one class, so
     ZAGROS-PROD→ZAGROS-PROD is the cell you most want to open. Treating the
     diagonal as an inert "self" cell there left the only populated cells in the
     matrix unclickable — the whole site view was a dead end.
     At peer level it is genuinely inert: a machine reaching itself means
     nothing, and a lit diagonal would be noise across the whole grid. */
  function mx(rows, cols, permits, label, tip, onClick, isUnr, isLive, viaPair, selected, selfIsReal, onIdent, cellNote) {
    if (!rows.length || !cols.length) return el('p', 'warn', 'nothing matches that filter.');
    geom(rows, cols, label);
    var W = LEFT + cols.length * CELL + 40, H = TOP + rows.length * CELL + 40;
    var svg = sv('svg', { viewBox: '0 0 ' + W + ' ' + H, width: W, height: H,
                          role: 'img', 'aria-label': 'reachability matrix' });
    /* Axis states: a machine is on this axis either BECAUSE IT IS A MEMBER of
       the group that was selected, or TRANSITIVELY — pulled in only because a
       selected machine can reach it through some additional group. Reading the
       second as if it were the first is how a drill-down silently overstates
       what the selected group contains. */
    /* Clicking an axis label opens that machine's FULL identity — the raw
       NetBird name, the derived long form, and every dimension marked resolved
       or not. Hover alone was the only route to it, which meant the long name
       and the join key were effectively lost on a touch device and easy to
       miss anywhere. */
    var idAxis = onIdent ? ' ident' : '';
    cols.forEach(function (ci, c) {
      var x = LEFT + c * CELL + CELL / 2;
      var sel = !selected || selected(ci, 'col');
      var t = sv('text', { class: 'rl' + idAxis + (sel ? '' : ' transitive'), x: x, y: TOP - 8,
                           transform: 'rotate(-90 ' + x + ' ' + (TOP - 8) + ')',
                           text: (sel ? '' : '· ') + label(ci) });
      if (onIdent) t.addEventListener('click', function () { onIdent(ci); });
      t.appendChild(sv('title', { text: tip(ci) + (sel ? '' : '\n\nnot a member of the selected group — present because a selected machine can reach it through another group') }));
      svg.appendChild(t);
    });
    rows.forEach(function (ri, r) {
      var sel = !selected || selected(ri, 'row');
      var t = sv('text', { class: 'rl' + idAxis + (sel ? '' : ' transitive'), x: LEFT - 10,
                           y: TOP + r * CELL + CELL / 2 + 4,
                           'text-anchor': 'end', text: label(ri) + (sel ? '' : ' ·') });
      if (onIdent) t.addEventListener('click', function () { onIdent(ri); });
      t.appendChild(sv('title', { text: tip(ri) })); svg.appendChild(t);
    });
    rows.forEach(function (ri, r) {
      cols.forEach(function (ci, c) {
        var inert = (ri === ci) && !selfIsReal;
        var es = inert ? null : permits(ri, ci);
        /* TWO INDEPENDENT DIMENSIONS, never folded into one channel.
             mediation  pair | other   — which policy relationship permits it
             liveness   live | dormant — are both ends connected right now
           Four states, and each dimension gets its own visual channel so a
           reader can answer either question without decoding the other:
           mediation is FILL (solid vs hatched), liveness is BORDER.
           `unrestricted` is a third, separate fact and rides the fill's hue —
           it is not part of this 2x2 and must not be confused with it. */
        /* The liveness channel applies ONLY where liveness is a fact. It is a
           property of a PEER being connected, so at the aggregate levels it
           was passed as a function returning false — which stamped `dormant`
           on every cell and rendered the entire site and class matrices at 32%
           opacity. A whole picture uniformly dimmed does not encode anything;
           it just looks broken, which is exactly how it looked. */
        /* Liveness has TWO altitudes and they must not both draw at once.
           At peer level it is a fact about the two machines in the cell, so
           `isLive` decides. At the aggregate levels there are no machines in
           the cell — the honest equivalent is the group pair's REALIZATION,
           which is the same question one rung up and was drawn nowhere. One
           channel, whichever altitude is in view. */
        var rung = es && !isLive ? realizationOf(es) : null;
        var cls = inert ? 'c self'
                : es ? 'c on'
                       + (viaPair && !viaPair(ri, ci) ? ' m-other' : ' m-pair')
                       + (isLive ? (isLive(ri, ci) ? ' live' : ' dormant') : '')
                       + (rung ? ' r-' + rung : '')
                       + (isUnr(es) ? ' unr' : '')
                : 'c';
        var cell = sv('rect', { class: cls, x: LEFT + c * CELL + 1, y: TOP + r * CELL + 1,
                                width: CELL - 2, height: CELL - 2 });
        if (es) {
          cell.appendChild(sv('title', {
            text: label(ri) + ' → ' + label(ci) + '\n' +
                  (es.length ? es.length + ' path(s)'
                             : (cellNote && cellNote(ri, ci, es)) || 'permitted') +
                  (rung ? '\n' + rung + ' — ' + REAL_WORD[rung] : '') +
                  (viaPair ? (viaPair(ri, ci) ? '\nvia the group pair you selected'
                                              : '\nNOT via the selected pair — through another membership') : '') +
                  '\nclick for the rules'
          }));
          cell.style.cursor = 'pointer';
          cell.addEventListener('click', function () { onClick(ri, ci); });
        }
        svg.appendChild(cell);
      });
    });
    var w = el('div', 'mx'); w.appendChild(svg); return w;
  }

  window.addEventListener('popstate', function () { readUrl(); paint(); });

  /* Cell size is derived from the viewport, and nothing was recomputing it.
     A matrix drawn in a narrow window stayed narrow after maximising, and one
     drawn wide overflowed after shrinking — measured: 854px picture still 854px
     in a 685px window. Deriving the geometry and then never re-deriving it is
     half a fix, and it is part of why the surface looked "rendered so small". */
  var rz;
  window.addEventListener('resize', function () {
    clearTimeout(rz);
    rz = setTimeout(function () { if (D) body(); }, 120);
  });
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', boot);
  else boot();
})();
