// Copy buttons, and the first screen's stage: real captures of the surfaces, one at a
// time. Each tab stays up for as long as its capture runs (data-dwell), then the next
// one shows. The stage pauses while a pointer or keyboard focus is on it and when the
// tab is hidden. Reduced motion: no advancing, the panoscope still instead of the GIF
// (chosen in the HTML), and videos wait for a press of play.
(function () {
  "use strict";
  document.querySelectorAll("button.copy").forEach(function (b) {
    b.addEventListener("click", function () {
      var text = document.getElementById(b.dataset.copy).textContent;
      if (navigator.clipboard) navigator.clipboard.writeText(text).then(function () {
        b.textContent = "Copied"; setTimeout(function () { b.textContent = "Copy"; }, 1600);
      });
    });
  });

  var stage = document.getElementById("stage");
  if (!stage) return;
  var reduce = window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  var tabs = Array.prototype.slice.call(stage.querySelectorAll('[role="tab"]'));
  var prompt = document.getElementById("prompt-text");
  var current = 0, timer = null, held = false, started = 0, left = 0;

  tabs.forEach(function (t) { var bar = document.createElement("span"); bar.className = "bar"; bar.setAttribute("aria-hidden", "true"); t.appendChild(bar); });

  function videos(panel) { return Array.prototype.slice.call(panel.querySelectorAll("video")); }

  function show(i, focus) {
    current = (i + tabs.length) % tabs.length;
    tabs.forEach(function (t, k) {
      var on = k === current, panel = document.getElementById(t.getAttribute("aria-controls"));
      t.setAttribute("aria-selected", on ? "true" : "false");
      t.tabIndex = on ? 0 : -1;
      panel.hidden = !on;
      panel.classList.toggle("on", on);
      videos(panel).forEach(function (v) {
        if (on && !reduce && getComputedStyle(v).display !== "none") { v.preload = "auto"; v.currentTime = 0; var p = v.play(); if (p) p.catch(function () {}); }
        else v.pause();
      });
      var bar = t.querySelector(".bar");
      bar.style.transition = "none"; bar.style.transform = "scaleX(0)";
    });
    if (prompt) prompt.textContent = tabs[current].dataset.prompt;
    if (focus) tabs[current].focus();
    schedule(Number(tabs[current].dataset.dwell) || 7000);
  }

  function schedule(ms) {
    clearTimeout(timer);
    if (reduce) return;
    left = ms; started = Date.now();
    var bar = tabs[current].querySelector(".bar");
    void bar.offsetWidth;
    if (held) return;
    bar.style.transition = "transform " + ms + "ms linear"; bar.style.transform = "scaleX(1)";
    timer = setTimeout(function () { show(current + 1, false); }, ms);
  }

  function hold() {
    if (held || reduce) return;
    held = true; clearTimeout(timer);
    left = Math.max(800, left - (Date.now() - started));
    var bar = tabs[current].querySelector(".bar");
    var w = getComputedStyle(bar).transform; bar.style.transition = "none"; bar.style.transform = w;
  }
  function release() {
    if (!held || reduce) return;
    held = false;
    var bar = tabs[current].querySelector(".bar");
    started = Date.now();
    bar.style.transition = "transform " + left + "ms linear"; bar.style.transform = "scaleX(1)";
    timer = setTimeout(function () { show(current + 1, false); }, left);
  }

  tabs.forEach(function (t, k) {
    t.addEventListener("click", function () { show(k, false); });
    t.addEventListener("keydown", function (e) {
      if (e.key === "ArrowRight") { e.preventDefault(); show(current + 1, true); }
      if (e.key === "ArrowLeft") { e.preventDefault(); show(current - 1, true); }
    });
  });
  stage.addEventListener("mouseenter", hold);
  stage.addEventListener("mouseleave", release);
  stage.addEventListener("focusin", hold);
  stage.addEventListener("focusout", function (e) { if (!stage.contains(e.relatedTarget)) release(); });
  document.addEventListener("visibilitychange", function () { if (document.hidden) hold(); else release(); });

  // The playground section's own recording plays only while it is on screen.
  var own = document.querySelectorAll("#playground video");
  if (!reduce && "IntersectionObserver" in window) {
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (en) {
        var v = en.target;
        if (en.isIntersecting && getComputedStyle(v).display !== "none") { v.preload = "auto"; var p = v.play(); if (p) p.catch(function () {}); }
        else v.pause();
      });
    }, { threshold: 0.3 });
    own.forEach(function (v) { io.observe(v); });
  }

  show(0, false);
})();
