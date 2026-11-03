// SPDX-License-Identifier: Apache-2.0
//
// Readiness view — the chain from "kit should be here" to "producing output".
//
// This is a MODE of the map, not a second page. The track picture answers what
// we detect; this answers what exists and how far it is actually working, which
// is the useful answer when nothing is being detected — the normal state at a
// site whose sensor is powered only when there is something to watch for.
//
// Rendering rules that carry meaning, not decoration:
//   - a SOLID connector means the chain carried;
//   - a DOTTED one means it stopped on purpose, so an at-rest site reads as
//     intent rather than failure;
//   - past a break the line fades out, because we cannot see a sensor through a
//     host that is offline and drawing it as failed would invent an observation;
//   - attested rungs carry a visible marker and are never styled like measured
//     ones — a person's word and a machine's reading are different claims.

(function () {
  "use strict";

  var POLL_MS = 20000;
  var els = {};
  var timer = null;
  var openSites = {}; // site_id -> expanded, so a refresh does not collapse the view

  function el(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  // A rung's visual state. `expected` is what separates "stopped on purpose"
  // from "broken", and it is the whole reason a radar-off site stays calm.
  function rungClass(r, breakAt) {
    // `derived` keeps an inferred rung visibly weaker than a measured one, so
    // a chain of inferences never reads as a chain of readings.
    if (r.state === "attained") return "rung att" + (r.source === "implied" ? " derived" : "");
    if (r.name === breakAt) return "rung brk";
    if (r.state === "unattained" && r.expected === false) return "rung rest";
    return "rung unk";
  }

  function renderChain(chain) {
    var wrap = el("div");
    if (chain.subject) wrap.appendChild(el("div", "rd-chain-name", chain.subject));

    var rungs = chain.rungs || [];

    // SAY WHY A CHAIN IS BLANK. A node that has never published health renders
    // as `declared` plus a column of empty circles, which looks like the panel
    // failed rather than like the node is silent. Those are different problems
    // with different owners, and the surface knows which one this is: every
    // rung above `declared` unknown, with the adapters reporting no health, can
    // only mean nothing has arrived. Naming it costs one line and saves the
    // reader from concluding the readiness panel is broken.
    var measured = rungs.filter(function (r) {
      return r.name !== 'declared' && r.state !== 'unknown';
    }).length;
    var noHealth = rungs.some(function (r) {
      return (r.detail || '').indexOf('no recent node health') !== -1;
    });
    if (rungs.length && measured === 0 && noHealth) {
      var why = el("div", "rd-silent");
      why.textContent = 'No health has reached goat-map from this node — '
        + 'nothing above "Declared" is being measured. This is the node being '
        + 'silent, not the check failing.';
      wrap.appendChild(why);
    }

    if (rungs.length) {
      var c = el("div", "chain");
      rungs.forEach(function (r) {
        var n = el("div", rungClass(r, chain.break_at));
        n.appendChild(el("span", "dot"));
        // Label and provenance share a line; the evidence gets its own, so it
        // is never the thing that gets clipped.
        var body = el("div", "body");
        body.appendChild(el("span", "lab", r.label || r.name));
        // Provenance is load-bearing, not a badge. "We read the generator" and
        // "we inferred it from the node being alive" support different
        // decisions, so they must never look the same.
        if (r.source === "attested") body.appendChild(el("span", "src", "attested"));
        else if (r.source === "implied") body.appendChild(el("span", "src imp", "inferred"));
        n.appendChild(body);
        if (r.detail) n.appendChild(el("span", "det", r.detail));
        c.appendChild(n);
      });
      wrap.appendChild(c);
    }

    // Say what it means in words. A colour an operator has to decode is a
    // colour they will eventually decode wrongly.
    if (chain.break_at) {
      wrap.appendChild(el("div", "rd-note brk", chain.remedy ||
        ("Stops at " + chain.break_at + ".")));
    } else if (chain.verdict === "at-rest") {
      wrap.appendChild(el("div", "rd-note",
        "Nothing is wrong — this is the resting state. Everything expected right now is running."));
    }
    return wrap;
  }

  function renderAttestations(rows) {
    if (!rows || !rows.length) return null;
    var box = el("div", "rd-attest");
    box.appendChild(el("h4", null, "Attested — no digital signal yet"));
    rows.forEach(function (r) {
      var row = el("div", "rd-arow");
      row.appendChild(el("span", "nm", r.label || r.subject_id));
      var a = r.attestation;
      var state = a ? a.state : null;
      var pill = el("span", "rd-astate " + (state ? "a-" + state : "a-none"),
        state || "never reported");
      row.appendChild(pill);
      if (a) {
        var age = el("span", "rd-age" + (a.expired ? " exp" : ""), fmtAge(a.age_s));
        row.appendChild(age);
      }
      box.appendChild(row);
    });
    return box;
  }

  function fmtAge(s) {
    if (s == null) return "";
    if (s < 90) return Math.round(s) + "s ago";
    if (s < 5400) return Math.round(s / 60) + "m ago";
    if (s < 172800) return Math.round(s / 3600) + "h ago";
    return Math.round(s / 86400) + "d ago";
  }

  function renderSite(site) {
    var box = el("li", "rd-site");

    var hd = el("button", "rd-hd");
    hd.type = "button";
    hd.setAttribute("aria-expanded", openSites[site.site_id] ? "true" : "false");
    hd.appendChild(el("span", "rd-name", site.label || site.site_id));
    // A badge alone cannot distinguish "one auxiliary node is unreadable" from
    // "this site is dark", and those call for very different responses. The
    // reason sits next to the badge while collapsed, so the distinction
    // survives without a click.
    if (site.reason) hd.appendChild(el("span", "rd-reason", site.reason));
    hd.appendChild(el("span", "rd-verdict v-" + (site.verdict || "unknown"),
      (site.verdict || "unknown").replace("-", " ")));
    box.appendChild(hd);

    var body = el("div", "rd-body");
    body.hidden = !openSites[site.site_id];
    (site.chains || []).forEach(function (c) { body.appendChild(renderChain(c)); });
    var att = renderAttestations(site.attestations);
    if (att) body.appendChild(att);
    box.appendChild(body);

    hd.addEventListener("click", function () {
      var open = !body.hidden;
      body.hidden = open;
      openSites[site.site_id] = !open;
      hd.setAttribute("aria-expanded", String(!open));
    });
    return box;
  }

  function render(data) {
    els.list.innerHTML = "";
    var sites = data.sites || [];
    if (!sites.length) {
      els.empty.hidden = false;
      els.empty.textContent = data.scope === "floor"
        // Say WHY it is empty. "No sites" and "you may not see these sites"
        // are different facts and must not look identical.
        ? "No sites are visible at your access level."
        : "No sites declared yet.";
      return;
    }
    els.empty.hidden = true;
    sites.forEach(function (s) { els.list.appendChild(renderSite(s)); });

    // Never imply a partial picture is the whole estate.
    if (data.scope && data.scope !== "full") {
      els.scope.hidden = false;
      els.scope.textContent = "Showing the estate you are scoped to — this is not the full picture.";
    } else {
      els.scope.hidden = true;
    }
  }

  function poll() {
    // `credentials: "same-origin"` is EXPLICIT on purpose. It is meant to be
    // the fetch() default, but this request was reaching the oauth2-proxy
    // fence with no session cookie while document requests from the same page
    // carried one — the proxy logged `id=-` and answered 401 on every poll,
    // which the panel rendered as "Readiness unavailable (401)" beside a map
    // that was still streaming tracks. Stating it removes the assumption from
    // the failure surface.
    //
    // The relative URL is also load-bearing: this page is served from "/" and
    // the fence only issues cookies for its own host, so any accidental
    // absolute/cross-origin form would silently drop the session.
    fetch("api/readiness", {
      credentials: "same-origin",
      headers: { Accept: "application/json" },
    })
      .then(function (r) { return r.ok ? r.json() : Promise.reject(r.status); })
      .then(render)
      .catch(function (err) {
        // A failed fetch must not leave a stale picture looking current.
        els.empty.hidden = false;
        els.empty.textContent = "Readiness unavailable (" + err + ") — this is not a healthy site list.";
        els.list.innerHTML = "";
      });
  }

  function setMode(mode) {
    var readiness = mode === "readiness";
    els.panel.hidden = readiness;
    els.rdPanel.hidden = !readiness;
    els.tabTracks.setAttribute("aria-selected", String(!readiness));
    els.tabReadiness.setAttribute("aria-selected", String(readiness));
    if (readiness) {
      poll();
      if (!timer) timer = setInterval(poll, POLL_MS);
    } else if (timer) {
      clearInterval(timer);
      timer = null;
    }
  }

  document.addEventListener("DOMContentLoaded", function () {
    els.panel = document.getElementById("panel");
    els.rdPanel = document.getElementById("rd-panel");
    els.list = document.getElementById("rd-list");
    els.empty = document.getElementById("rd-empty");
    els.scope = document.getElementById("rd-scope");
    els.tabTracks = document.getElementById("tab-tracks");
    els.tabReadiness = document.getElementById("tab-readiness");
    if (!els.rdPanel || !els.tabReadiness) return;

    els.tabTracks.addEventListener("click", function () { setMode("tracks"); });
    els.tabReadiness.addEventListener("click", function () { setMode("readiness"); });
    setMode("tracks");
  });
})();
