/* identity — one derived name per peer, in four forms.
 *
 * WHY THIS EXISTS
 *
 * NetBird names are whatever the enrolling machine happened to call itself:
 * `DESKTOP-ABC1234`, `peer-someone-galaxy-2030-01-05-Someones-Laptop.local`,
 * `gw-svn-001`. They are not a naming scheme, they collide,
 * they carry a hostname's accidents, and on an axis label they are both
 * unreadable and uninformative. But they ARE the key everything else joins on,
 * so they cannot be replaced — only mapped.
 *
 * FOUR FORMS, per the operator's requirement:
 *
 *   1. netbird  — the raw name. Never altered; it is the join key.
 *   2. short    — for an axis label. Terse, ~16 chars, and UNIQUE across the
 *                 goatnet: resolve() proposes it, disambiguate() (below)
 *                 guarantees it over the whole peer set.
 *   3. full     — for hover. Every dimension we resolved, spelled out.
 *   4. mapping  — the complete identity record, dimension by dimension,
 *                 each marked resolved or NOT RESOLVABLE FROM THIS SOURCE.
 *
 * THE VOCABULARY IS NOT INVENTED.
 *
 * Every term below comes from ops/identity/grammar.yaml (ADR 1067), the
 * generated source of truth for the identity grammar's controlled
 * vocabularies — `node_class` (SENSOR / EFFECTOR / PROCESSOR / PLATFORM /
 * MGMT-HOST / ZENOH / SNITCH / SERVICES / NETWORK), `role_family` (ADMIN /
 * MONITOR / DEVELOPER / OPERATOR / OBSERVER / VENDOR), `identity.type`
 * (person / system) — plus the ADR 1116 gear classes. An earlier version of
 * this surface labelled itself with words it made up (AUTHORITY, SUBSTRATE,
 * FEEDERS), which an operator had no way to look up because they existed
 * nowhere but in that layout. Nothing here is a new word.
 *
 * WHAT IS HONESTLY UNRESOLVABLE
 *
 * The grammar has five orthogonal dimensions: role, affiliation, echelon,
 * node-class, site. From NetBird membership alone we can resolve SITE and
 * NODE-CLASS/ROLE, because group names are `<SITE>-<SUFFIX>` and the suffix
 * vocabulary is the shared taxonomy. AFFILIATION and ECHELON live in the
 * portal registry and the operator manifests, not on the mesh plane, and are
 * reported as unresolved rather than guessed. A derived name that quietly
 * invented an echelon would be worse than one that admits it does not know.
 */
(function (global) {
  'use strict';

  /* Site prefix -> short tag. Derived from the goatnet's own scope families,
     not hardcoded per site: any `<PREFIX>-<SUFFIX>` group contributes. */
  function siteTag(p) {
    return p.split('-').map(function (w) { return w[0]; }).join('').toLowerCase();
  }

  /* Group suffix -> (class, kind). `kind` says which dimension the suffix is
     speaking about, because they are NOT the same axis: KID is a gear class,
     DEVELOPER is a person role, P2P is a graduated capability. Collapsing them
     into one token is the exact defect ADR 1043 exists to remove. */
  var SUFFIX = {
    'INFRA':                  ['MGMT-HOST', 'class'],
    'BACKEND-INFRA':          ['SERVICES',  'class'],
    'EXTERNAL-INFRA':         ['SERVICES',  'class'],
    'NETWORK':                ['NETWORK',   'class'],
    'POD':                    ['PLATFORM',  'class'],
    'MOON':                   ['PLATFORM',  'class'],
    'KID':                    ['KID',       'class'],
    'EXTERNAL-KID':           ['KID',       'class'],
    'CONTROL':                ['CONTROL',   'class'],
    'FUSION':                 ['PROCESSOR', 'class'],
    'EXTERNAL-INTERMEDIARY':  ['PROCESSOR', 'class'],
    'BREAKGLASS':             ['BREAKGLASS','class'],
    'QUARANTINED':            ['QUARANTINED','state'],
    'PROBATION-GEAR':         ['PROBATION', 'state'],
    'P2P':                    ['P2P',       'capability'],
    'PLATFORM-ADMIN':         ['ADMIN',     'role'],
    'DEVELOPER':              ['DEVELOPER', 'role'],
    'MONITOR':                ['MONITOR',   'role'],
    'OBSERVER':               ['OBSERVER',  'role'],
    'OPERATOR':               ['OPERATOR',  'role']
  };
  function classify(sfx) {
    if (SUFFIX[sfx]) return SUFFIX[sfx];
    if (/^SENSOR(-|$)/.test(sfx))   return ['SENSOR',   'class'];
    if (/^EFFECTOR(-|$)/.test(sfx)) return ['EFFECTOR', 'class'];
    if (/^PROBATION-/.test(sfx))    return ['PROBATION','state'];
    return [sfx, 'class'];
  }

  /* A person peer is one enrolled through the onboarding path, which stamps
     `peer-<handle>-<goatnet>-<date>-<device>`. That prefix is a real, checkable
     convention — not a guess from the name looking human. */
  var PERSON = /^peer-([a-z0-9]+)-/i;

  /* The same stamp, taken apart. The DATE is the only reliable landmark in it:
     handles contain hyphens (`unit-pl-008`), goatnet names contain hyphens
     (`zagros-prod`), and some cohorts carry a letter (`2026-07-08b`). Splitting
     on the first date is what makes the device tail recoverable. */
  var STAMP = /^peer-(.+?)-(\d{4}-\d{2}-\d{2}[a-z]?)(?:-([\s\S]*))?$/i;

  /* Strip the accidents off a machine name to get a stable discriminator:
     the onboarding stamp, the goatnet, the date, the trailing .local. */
  function discriminator(name, families) {
    var s = name;
    var m = PERSON.exec(s);
    if (m) return m[1];                                  // the person's handle
    families.forEach(function (f) {
      s = s.replace(new RegExp(f.replace(/[-]/g, '[-]'), 'ig'), '');
    });
    s = s.replace(/\.local$/i, '')
         .replace(/^mgmt-/i, '')
         .replace(/\d{4}-\d{2}-\d{2}/g, '')
         .replace(/-{2,}/g, '-')
         .replace(/^-|-$/g, '');
    return s || name;
  }

  /* The onboarding slug is written twice in most stamps — `unit-pl-008-unit-pl-008`,
     `pat-example-pat-example` — or ends by repeating its own first
     token, `pat-example-org-pat`. Both are the stamp's doing, not a fact
     about the machine, so the repetition is dropped before the handle is shown. */
  function dedupeHandle(h) {
    for (var pass = 0; pass < 3; pass++) {
      var t = h.split('-');
      if (t.length >= 2 && t.length % 2 === 0) {
        var a = t.slice(0, t.length / 2).join('-');
        if (a.toLowerCase() === t.slice(t.length / 2).join('-').toLowerCase()) { h = a; continue; }
      }
      if (t.length > 2 && t[0].toLowerCase() === t[t.length - 1].toLowerCase()) {
        h = t.slice(0, -1).join('-'); continue;
      }
      break;
    }
    return h;
  }

  /* The device half of the stamp, with the DNS accidents off: `.local`,
     `.lan`, `.users.troie.ia`. What is left is the name the machine's own
     owner chose, which is the only thing that tells four of one person's
     laptops apart. */
  function deviceTail(dev) {
    if (!dev) return null;
    var d = dev.split('.')[0] || dev;
    d = d.replace(/-{2,}/g, '-').replace(/^-|-$/g, '');
    return d || null;
  }

  /* handle + device, from the raw name, for one peer. Returns nulls when the
     name is not stamped — a system peer has no handle and no device half. */
  function stampParts(name, families) {
    var m = STAMP.exec(name);
    if (!m) return { handle: null, tail: null };
    var h = m[1];
    families.forEach(function (f) {
      h = h.replace(new RegExp('[-]' + f.replace(/[-]/g, '[-]') + '$', 'i'), '');
    });
    return { handle: dedupeHandle(h) || null, tail: deviceTail(m[3]), tail_raw: m[3] || null };
  }

  /**
   * Resolve one peer into all four forms.
   * @param peer  a peer record from /api/netvis (name, os, ip, groups, connected)
   * @param nodes the payload's node array, so group indices become names
   * @param families the goatnet's scope families, e.g. ['GALAXY-PROD','SITE-A']
   */
  function resolve(peer, nodes, families) {
    var groups = (peer.groups || []).map(function (i) { return nodes[i].name; });

    var sites = {}, classes = {}, roles = {}, states = {}, caps = {}, undecl = [];
    groups.forEach(function (g) {
      if (g === 'All') return;
      var fam = null;
      for (var i = 0; i < families.length; i++) {
        if (g === families[i] || g.indexOf(families[i] + '-') === 0) { fam = families[i]; break; }
      }
      if (!fam) { undecl.push(g); return; }
      sites[fam] = 1;
      var sfx = g.slice(fam.length + 1);
      if (!sfx) return;
      var c = classify(sfx), val = c[0], kind = c[1];
      if (kind === 'class') classes[val] = 1;
      else if (kind === 'role') roles[val] = 1;
      else if (kind === 'state') states[val] = 1;
      else caps[val] = 1;
    });

    var siteList = Object.keys(sites).sort();
    var classList = Object.keys(classes).sort();
    var roleList = Object.keys(roles).sort();
    var stateList = Object.keys(states).sort();
    var capList = Object.keys(caps).sort();

    var isPerson = PERSON.test(peer.name);
    var disc = discriminator(peer.name, families);

    /* SHORT — what goes on an axis.
       `<sites>/<what it is>·<which one>`; sites joined by + when a machine is
       enrolled into more than one, because that is a fact worth seeing at a
       glance and is precisely how a machine comes to hold two families. */
    var tag = siteList.map(siteTag).join('+') || '?';
    var what = isPerson ? (roleList[0] || 'person')
                        : (classList[0] || roleList[0] || '?');
    var pre = tag + '/' + what.toLowerCase() + '·';
    var post = stateList.length ? '!' + stateList[0].toLowerCase() : '';
    var short = pre + trim(disc, 12) + post;

    /* The pieces `disambiguate()` needs to lengthen this label if — and only
       if — it turns out not to be unique. resolve() sees ONE peer, so it
       cannot know; it can only leave the raw material where the whole-set
       pass can reach it. */
    var stamp = stampParts(peer.name, families);

    /* FULL — hover. Everything resolved, in words. */
    var full = [
      peer.name,
      'site: ' + (siteList.join(' + ') || 'none'),
      (isPerson ? 'person' : 'system') +
        (classList.length ? ' · class: ' + classList.join(', ') : '') +
        (roleList.length ? ' · role: ' + roleList.join(', ') : ''),
      capList.length ? 'capability: ' + capList.join(', ') : null,
      stateList.length ? 'STATE: ' + stateList.join(', ') : null,
      (peer.os || 'os unknown') + ' · ' + (peer.ip || 'no ip') +
        ' · ' + (peer.connected ? 'connected' : 'enrolled, not connected'),
      undecl.length ? 'groups outside any declared family: ' + undecl.join(', ') : null
    ].filter(Boolean).join('\n');

    return {
      netbird: peer.name,
      short: short,
      full: full,
      /* NOT a fifth form — scaffolding for the uniqueness pass below. */
      parts: {
        pre: pre, post: post,
        base: trim(disc, 12),
        handle: stamp.handle,
        tail: stamp.tail,
        tail_raw: stamp.tail_raw,
        raw: peer.name
      },
      short_level: 0,
      mapping: {
        site: siteList,
        node_class: classList,
        role: roleList,
        capability: capList,
        state: stateList,
        identity_type: isPerson ? 'person' : 'system',
        handle: isPerson ? disc : null,
        groups: groups,
        undeclared_groups: undecl,
        /* Stated, not omitted. These are real dimensions of the grammar that
           this data source cannot answer; a blank would read as "none". */
        affiliation: null,   // portal registry / operator manifest
        echelon: null        // operator manifest
      },
      unresolved: ['affiliation', 'echelon']
    };
  }

  function trim(s, n) { return s.length > n ? s.slice(0, n - 1) + '…' : s; }

  /* ------------------------------------------------------------------ *
   * UNIQUENESS
   *
   * A derived name whose job is to map to a NetBird name fails that job the
   * moment it maps to four of them. The SHORT form collides when many machines
   * all label `e/developer·unit`. Two causes, both here:
   *   - the handle regex takes only the FIRST hyphen-delimited token, so
   *     unit-pl-008 / unit-pl-011 / unit-pl-312 / unit-pl-500 are all `unit`;
   *   - a handle names a PERSON, and a person owns several machines.
   *
   * The disambiguator is REAL INFORMATION off the raw name, never a counter:
   * first the rest of the handle, then the device half of the stamp
   * (`someone-mac`, `ubuntu`) which the old discriminator threw away. Only
   * when the raw names are genuinely identical — three peers all called
   * `peer-someone-…-Someones-Laptop.local` — does it fall
   * back to the NetBird peer id, which is still a real key an operator can
   * look up, and still stable.
   *
   * MINIMUM: a label is lengthened only if it collides, so the common case
   * stays scannable. STABLE: uniqueness is computed over the WHOLE peer list
   * once, so a name never changes because the view was filtered.
   * ------------------------------------------------------------------ */

  /* Discriminator budget per rung: 12 in the common case (unchanged), 16 for
     the whole handle, 10 + '/' + 12 = 23 for handle/device, 27 with the DNS
     suffix restored, and +5 for a 4-char peer-id fragment. matrix.js sizes the
     axis gutter to the longest label and stops at 360px (~47 chars), which is
     the ceiling ops/ci/check-netvis-short-names.js enforces. */
  var W_HANDLE = 16, W_HANDLE_SHORT = 10, W_TAIL = 12, W_TAIL_RAW = 16, W_RAW = 22;

  function idKey(peer, i, n) {
    var k = peer && peer.id ? String(peer.id) : ('p' + i);
    return n && k.length > n ? k.slice(-n) : k;
  }

  /* The rungs for one peer, shortest first. Each is a whole discriminator, and
     the rungs are FIXED SLOTS, not a compacted list: slot 2 is always "the
     device", whether or not this particular peer's device adds anything. That
     is what lets a colliding set climb in lockstep and stay legible as a set —
     `ab12/Mac` beside `ab12/Mac.lan`, not `ab12` beside `ab12/Mac`. A
     slot with nothing new to say simply repeats the one before it. */
  function rungs(id, peer, i) {
    var p = id.parts || {};
    var lead = trim(p.handle || p.base || '', W_HANDLE_SHORT);
    var s0 = p.base || '';
    var s1 = p.handle ? trim(p.handle, W_HANDLE) : (p.raw && !p.tail ? trim(p.raw, W_RAW) : s0);
    var s2 = p.tail ? lead + '/' + trim(p.tail, W_TAIL) : s1;
    /* `Mac` and `Mac.lan` are two machines; the DNS suffix is dropped for
       readability but put back before falling through to an id fragment. */
    var s3 = p.tail_raw ? lead + '/' + trim(p.tail_raw, W_TAIL_RAW) : s2;
    /* Past slot 3 the id is doing the distinguishing, so the id rungs carry the
       SHORTER readable device rung — spelling out `.local` next to a peer id
       costs width and settles nothing. */
    return [s0, s1, s2, s3,
            s2 + '#' + idKey(peer, i, 4),
            s2 + '#' + idKey(peer, i, 8),
            s2 + '#' + idKey(peer, i, 0)];
  }

  /**
   * Make `short` unique across the whole peer set. Call ONCE, on the full
   * list, immediately after resolve(); never per view.
   * @param ids   the resolve() records, in peer order
   * @param peers the payload's peer array, same order
   * @returns the same array, `short` lengthened only where it collided
   */
  function disambiguate(ids, peers) {
    peers = peers || [];
    var ladder = ids.map(function (id, i) { return rungs(id, peers[i], i); });
    var lvl = ids.map(function () { return 0; });
    function labelAt(i, l) {
      var p = ids[i].parts || {};
      return (p.pre || '') + ladder[i][l] + (p.post || '');
    }
    function label(i) { return labelAt(i, lvl[i]); }

    /* Refinement, one rung at a time. Each pass regroups by the CURRENT label
       and lengthens only the labels still tied — so a peer stops climbing the
       moment it is distinguishable and its neighbours climb on without it. The
       nine `unit` peers settle as `unit-pl-011`, `unit-pl-312`, `unit-pl-500/…`
       and four `unit-pl-008/BOX…`: each says exactly as much as it has to.

       Nothing here is decided by array position, so the same peer set yields
       the same labels whatever order the API returns it in. */
    for (var pass = 0; pass < 8; pass++) {
      var seen = {}, moved = false;
      for (var i = 0; i < ids.length; i++) {
        var k = label(i);
        (seen[k] || (seen[k] = [])).push(i);
      }
      Object.keys(seen).forEach(function (k) {
        var g = seen[k];
        if (g.length < 2) return;
        g.forEach(function (j) { if (lvl[j] < ladder[j].length - 1) { lvl[j]++; moved = true; } });
      });
      if (!moved) break;
    }

    /* Belt and braces: the ladder ends in the peer id, so this should be
       unreachable — but a surface that silently shows two rows the same name
       is exactly the defect being fixed, so it is not left to trust. */
    var used = {};
    for (var n = 0; n < ids.length; n++) {
      var s = label(n);
      if (used[s]) s += '#' + n;
      used[s] = 1;
      ids[n].short = s;
      ids[n].short_level = lvl[n];
    }
    return ids;
  }

  global.goatIdentity = { resolve: resolve, classify: classify, disambiguate: disambiguate };
})(typeof globalThis !== 'undefined' ? globalThis : window);
