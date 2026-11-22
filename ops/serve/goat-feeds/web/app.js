// SPDX-License-Identifier: Apache-2.0
// Fabric feeds — plain JavaScript, no framework, no build step.
// Every value from the server is placed with textContent; nothing is parsed as HTML.
"use strict";

const state = { me: null, catalog: [], intake: { enabled: false }, feeds: [], filter: "mine", search: "", timer: null, req: null };

// ---------- small helpers ----------
const $ = (sel, root = document) => root.querySelector(sel);
function el(tag, attrs = {}, ...kids) {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === undefined || v === null || v === false) continue;
    if (k === "class") n.className = v;
    else if (k === "text") n.textContent = v;
    else if (k.startsWith("on")) n.addEventListener(k.slice(2), v);
    else n.setAttribute(k, v === true ? "" : v);
  }
  for (const kid of kids.flat()) if (kid !== null && kid !== undefined && kid !== false) n.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
  return n;
}
function toast(msg) {
  const t = $("#toast");
  t.textContent = msg;
  t.hidden = false;
  clearTimeout(toast._t);
  toast._t = setTimeout(() => (t.hidden = true), 4000);
}
async function api(path, opts = {}) {
  const init = { headers: { Accept: "application/json" }, credentials: "same-origin", ...opts };
  if (opts.body !== undefined) {
    init.method = init.method || "POST";
    init.headers["Content-Type"] = "application/json";
    init.headers["X-Goat-Feeds"] = "1";
    init.body = JSON.stringify(opts.body);
  }
  const r = await fetch(path, init);
  let data = {};
  try { data = await r.json(); } catch { /* not JSON */ }
  if (r.status === 401) { location.href = "/oauth2/sign_in?rd=" + encodeURIComponent(location.pathname + location.hash); throw new Error("sign in"); }
  if (!r.ok) throw new Error(data.error || `${r.status} ${r.statusText}`);
  return data;
}
const fmtTime = (s) => { if (!s) return "—"; const d = new Date(s); return isNaN(d) || d.getFullYear() < 2000 ? "—" : d.toISOString().replace("T", " ").slice(0, 16); };
function ago(s) {
  if (!s) return "never"; const d = new Date(s); if (isNaN(d) || d.getFullYear() < 2000) return "never";
  const sec = Math.max(0, Math.round((Date.now() - d) / 1000));
  if (sec < 60) return `${sec}s ago`; if (sec < 3600) return `${Math.round(sec / 60)} min ago`;
  if (sec < 86400) return `${Math.round(sec / 3600)} h ago`; return `${Math.round(sec / 86400)} d ago`;
}
function daysLeft(s) { const d = Math.round((new Date(s) - Date.now()) / 86400000); return d < 0 ? "ended" : d === 0 ? "ends today" : `${d} d left`; }
const health = (h) => el("span", { class: `h ${h}`, text: h });
const cls = (c) => el("span", { class: `cls ${c}`, text: c === "real" ? "Real data" : "Synthetic" });
const dest = (f) => `${f.dest_name} · ${f.dest_ip}:${f.port}/${f.transport}`;
const isIn = (f) => f.direction === "in";
const title = (f) => isIn(f) ? `${f.topic_label}  ←  ${f.dest_name}` : `${f.topic_label || f.topic_id}  →  ${f.dest_name}`;
const verb = { approve: "Approve", decline: "Decline", pause: "Pause", resume: "Resume", renew: "Renew", remove: "Remove", check: "Check destination" };

// ---------- routing ----------
function route() {
  clearInterval(state.timer);
  const h = (location.hash || "#feeds").slice(1).split("?")[0];
  const [view, id] = h.split("/");
  document.querySelectorAll("nav a").forEach((a) => a.classList.toggle("on", a.dataset.view === view || (view === "feed" && a.dataset.view === "feeds")));
  const main = $("#main");
  main.replaceChildren();
  if (view === "request") return showRequest(main);
  if (view === "approvals") return showApprovals(main);
  if (view === "audit") return showAudit(main);
  if (view === "feed" && id) return showFeed(main, id);
  return showFeeds(main);
}
const mount = (main, id) => { const frag = $(id).content.cloneNode(true); main.append(frag); return main; };

// ---------- header ----------
async function loadMe() {
  const me = await api("/api/me");
  state.me = me;
  $("#site").textContent = `${me.site} · ${me.goatnet}`;
  const v = me.viewer;
  const roleText = { "platform-admin": "Platform admin", "tactical-admin": "Tactical admin", member: "Member" }[v.role] || v.role;
  $("#who").replaceChildren(el("b", { text: v.email }), el("span", { class: "role", text: roleText }), el("br"),
    el("a", { href: "/oauth2/sign_out", text: "Sign out" }));
  $("#nav-approvals").hidden = !v.is_admin;
  await loadCatalog();
}
async function loadCatalog() {
  const [cat, intake] = await Promise.all([api("/api/catalog"), api("/api/intake")]);
  state.catalog = cat.topics || [];
  state.intake = intake;
}
async function refreshFeeds() {
  const d = await api("/api/feeds");
  state.feeds = d.feeds || [];
  const waiting = state.feeds.filter((f) => f.status === "pending").length;
  const c = $("#approvals-count");
  c.textContent = waiting ? String(waiting) : "";
  c.hidden = !waiting;
  return state.feeds;
}

// ---------- feeds list ----------
function matches(f) {
  const me = state.me.viewer.email.toLowerCase();
  if (state.filter === "mine" && f.owner.toLowerCase() !== me) return false;
  if (state.filter === "waiting" && f.status !== "pending") return false;
  if (state.filter === "live" && !["active", "paused"].includes(f.status)) return false;
  if (state.filter === "all" && f.status === "removed") return false;
  const q = state.search.toLowerCase();
  if (!q) return true;
  return [f.topic_label, f.dest_name, f.dest_ip, String(f.port), f.owner, f.output_label, f.id, f.label].some((x) => (x || "").toLowerCase().includes(q));
}
function feedCard(f) {
  const d = f.delivery;
  const side = f.status === "active" && d && d.stats ? (isIn(f) ? `${d.stats.in} received · ${d.stats.converted} published` : `${d.stats.in} in · ${d.sent_estimate} sent`)
    : f.status === "pending" ? `asked ${ago(f.created_at)}` : "";
  const line2 = isIn(f) ? `${f.output_label} from ${f.dest_ip}, received on ${f.listen_at || "port " + f.port} · ${f.owner} · ${daysLeft(f.end_at)}`
    : `${f.output_label || f.output_id} to ${f.dest_ip}:${f.port}/${f.transport} · ${f.owner} · ${daysLeft(f.end_at)}`;
  return el("a", { class: "card", href: `#feed/${f.id}` },
    health(f.health),
    el("div", { class: "main" },
      el("div", { class: "line1" }, el("span", { class: "dir", text: isIn(f) ? "IN" : "OUT" }), title(f)),
      el("div", { class: "line2", text: line2 }),
      f.health_note ? el("div", { class: "line2", text: f.health_note }) : null),
    el("div", { class: "side" }, cls(f.data_class), el("br"), side));
}
function renderList() {
  const list = $("#feed-list");
  if (!list) return;
  const rows = state.feeds.filter(matches);
  list.replaceChildren(...(rows.length ? rows.map(feedCard) : [el("p", { class: "empty", text:
    state.filter === "mine" ? "You have no feeds yet. Request one to send a topic to a system, or to bring a system's data into the fabric." : "No feeds match." })]));
  document.querySelectorAll(".chip").forEach((c) => c.classList.toggle("on", c.dataset.filter === state.filter));
}
async function showFeeds(main) {
  mount(main, "#tpl-feeds");
  if (state.me.viewer.is_admin && state.filter === "mine" && !showFeeds._set) { state.filter = "all"; showFeeds._set = true; }
  document.querySelectorAll(".chip").forEach((c) => c.addEventListener("click", () => { state.filter = c.dataset.filter; renderList(); }));
  const s = $("#feed-search");
  s.value = state.search;
  s.addEventListener("input", () => { state.search = s.value; renderList(); });
  await refreshFeeds();
  renderList();
  state.timer = setInterval(async () => { try { await refreshFeeds(); renderList(); } catch { /* shown next time */ } }, 5000);
}

// ---------- request flow ----------
function newReq(dir) {
  return { dir: dir || "out", step: 1, topic: null, output: null, format: null, label: "", peer: null, manual: false, dest_name: "", dest_ip: "",
           transport: "udp", port: "", days: state.me.default_days, purpose: "", error: "" };
}
const STEP_NAMES = { out: ["Topic", "Destination", "Delivery", "Review"], in: ["What it sends", "Sending system", "Receive port", "Review"] };
async function showRequest(main) {
  mount(main, "#tpl-request");
  try { await loadCatalog(); } catch { /* keep the last copy */ }
  const want = new URLSearchParams(location.hash.split("?")[1] || "").get("dir");
  if (!state.req || (want && want !== state.req.dir)) state.req = newReq(want);
  const seg = $("#dir-seg");
  seg.className = "seg dirseg";
  seg.replaceChildren(...[["out", "Send a topic to a system"], ["in", "Receive a system's data"]].map(([d, label]) => el("button", {
    type: "button", class: state.req.dir === d ? "on" : "", text: label,
    onclick: () => { if (state.req.dir !== d) { state.req = newReq(d); seg.querySelectorAll("button").forEach((b, i) => b.classList.toggle("on", ["out", "in"][i] === d)); renderStep(); } } })));
  $("#step-back").addEventListener("click", () => { state.req.error = ""; state.req.step = Math.max(1, state.req.step - 1); renderStep(); });
  $("#step-next").addEventListener("click", nextStep);
  renderStep();
}
function stepValid(r) {
  if (r.dir === "in") {
    if (r.step === 1) {
      if (!r.format) return "Choose the standard the system sends.";
      return r.label.trim().length >= 3 ? "" : "Name what the system sends, for example \"Blue C-UAS drone position\".";
    }
    if (r.step === 2) {
      if (!r.dest_name.trim()) return "Choose the sending system, or enter its name.";
      if (!/^\d{1,3}(\.\d{1,3}){3}$/.test(r.dest_ip.trim())) return "Enter the sending system's mesh IP address.";
      return "";
    }
    if (r.step === 3) return r.port ? "" : "Choose a free receive port.";
    if (r.step === 4) return r.purpose.trim().length >= 3 ? "" : "Say what the data is and who uses it, so an approver can decide.";
    return "";
  }
  if (r.step === 1) return r.topic ? "" : "Choose a topic.";
  if (r.step === 2) {
    if (!r.dest_name.trim()) return "Choose the receiving system, or enter its name.";
    if (!/^\d{1,3}(\.\d{1,3}){3}$/.test(r.dest_ip.trim())) return "Enter the system's mesh IP address.";
    return "";
  }
  if (r.step === 3) {
    if (!r.output) return "Choose an output format.";
    const p = Number(r.port);
    if (!Number.isInteger(p) || p < state.me.port_min || p > state.me.port_max) return `Enter a port between ${state.me.port_min} and ${state.me.port_max}.`;
    return "";
  }
  if (r.step === 4) return r.purpose.trim().length >= 3 ? "" : "Say what the feed is for, so an approver can decide.";
  return "";
}
async function nextStep() {
  const r = state.req;
  const problem = stepValid(r);
  if (problem) { r.error = problem; return renderStep(); }
  r.error = "";
  if (r.step < 4) { r.step++; return renderStep(); }
  $("#step-next").disabled = true;
  try {
    const common = { dest_name: r.dest_name.trim(), dest_ip: r.dest_ip.trim(), dest_peer_id: r.peer ? r.peer.id : "",
      port: Number(r.port), days: Number(r.days), purpose: r.purpose.trim() };
    const body = r.dir === "in" ? { ...common, direction: "in", format_id: r.format.id, label: r.label.trim(), transport: "udp" }
      : { ...common, direction: "out", topic_id: r.topic.id, output_id: r.output.id, transport: r.transport };
    const f = await api("/api/feeds", { body });
    state.req = null;
    toast(f.status === "active" ? (isIn(f) ? "Receiving is on. The system can send now." : "Feed is on. Delivery starts within a few seconds.") : "Request sent. It waits for an approver.");
    location.hash = `#feed/${f.id}`;
  } catch (e) {
    r.error = e.message;
    $("#step-next").disabled = false;
    renderStep();
  }
}
function renderStep() {
  const r = state.req;
  document.querySelectorAll("#steps li").forEach((li) => {
    const n = Number(li.dataset.step);
    li.textContent = STEP_NAMES[r.dir][n - 1];
    li.classList.toggle("on", n === r.step);
    li.classList.toggle("done", n < r.step);
  });
  $("#step-back").disabled = r.step === 1;
  $("#step-next").textContent = r.step === 4 ? "Submit" : "Next";
  $("#step-next").disabled = false;
  const body = $("#step-body");
  const err = r.error ? el("div", { class: "err", role: "alert", text: r.error }) : null;
  const steps = r.dir === "in" ? [stepInWhat, stepDest, stepInPort, stepInReview] : [stepTopic, stepDest, stepDelivery, stepReview];
  if (r.dir === "in" && !state.intake.enabled) {
    return body.replaceChildren(el("p", { class: "empty", text: "This site does not receive data from systems yet. A platform admin sets its receive ports." }));
  }
  body.replaceChildren(...[err, ...steps[r.step - 1](r)].filter(Boolean));
}
function stepTopic(r) {
  const groups = [["processed", "Processed pictures (correlated, fused, filtered)"], ["raw", "Raw sensor topics"], ["published", "Received from systems (published by goat-feeds)"]];
  const out = [];
  for (const [kind, label] of groups) {
    const topics = state.catalog.filter((t) => t.kind === kind);
    if (!topics.length) continue;
    out.push(el("div", { class: "group-label", text: label }));
    out.push(el("div", { class: "picks" }, topics.map((t) => el("button", {
      type: "button", class: "pick" + (r.topic && r.topic.id === t.id ? " on" : ""),
      onclick: () => { r.topic = t; r.output = t.outputs.length === 1 ? t.outputs[0] : null; renderStep(); } },
      el("span", { class: "t" }, t.label, cls(t.data_class)),
      t.description ? el("span", { class: "d", text: t.description }) : null,
      el("span", { class: "d" }, "Offered as: ", t.outputs.map((o) => o.label).join(", ")),
      el("span", { class: "d mono", text: t.key })))));
  }
  if (!out.length) out.push(el("p", { class: "empty", text: "No topics are offered at this site yet. A platform admin adds them to the site's catalog." }));
  return out;
}
function stepDest(r) {
  const out = [];
  if (state.me.peers_enabled && !r.manual) {
    const results = el("div", { class: "picks" });
    const search = el("input", { type: "search", placeholder: "Search systems by name, host or address", "aria-label": "Search mesh systems" });
    const run = async () => {
      try {
        const d = await api("/api/peers?q=" + encodeURIComponent(search.value));
        if (d.error) results.replaceChildren(el("div", { class: "err", text: "The mesh directory answered with an error: " + d.error }));
        else results.replaceChildren(...(d.peers.length ? d.peers.map((p) => el("button", {
          type: "button", class: "pick" + (r.peer && r.peer.id === p.id ? " on" : ""),
          onclick: () => { r.peer = p; r.dest_name = p.name; r.dest_ip = p.ip; renderStep(); } },
          el("span", { class: "t" }, el("span", { class: "dot" + (p.connected ? " on" : ""), title: p.connected ? "online" : "offline" }), p.name),
          el("span", { class: "d" }, `${p.ip} · ${p.hostname || "—"} · ${p.connected ? "online" : "offline, last seen " + ago(p.last_seen)}`),
          p.groups && p.groups.length ? el("span", { class: "d", text: p.groups.join(", ") }) : null)) : [el("p", { class: "empty", text: "No system matches." })]));
      } catch (e) { results.replaceChildren(el("div", { class: "err", text: e.message })); }
    };
    let t;
    search.addEventListener("input", () => { clearTimeout(t); t = setTimeout(run, 250); });
    out.push(el("div", { class: "field" }, el("label", { text: r.dir === "in" ? "Sending system" : "Receiving system" }), search,
      el("span", { class: "hint", text: "Systems on this goatnet's mesh. The address fills in from the one you choose." })));
    if (r.peer) out.push(el("div", { class: "next-box" }, "Selected: ", el("b", { text: r.peer.name }), ` (${r.peer.ip})`));
    out.push(results);
    out.push(el("p", {}, el("a", { href: "#request", onclick: (e) => { e.preventDefault(); r.manual = true; renderStep(); }, text: "Enter a name and address instead" })));
    run();
  } else {
    const name = el("input", { type: "text", value: r.dest_name, id: "dest-name", oninput: (e) => (r.dest_name = e.target.value) });
    const ip = el("input", { type: "text", value: r.dest_ip, id: "dest-ip", placeholder: "100.64.x.y", oninput: (e) => { r.dest_ip = e.target.value; r.peer = null; } });
    out.push(el("div", { class: "field" }, el("label", { for: "dest-name", text: r.dir === "in" ? "Sending system's name" : "Receiving system's name" }), name,
      el("span", { class: "hint", text: "As people know it, for example the NetBird peer name." })));
    out.push(el("div", { class: "field" }, el("label", { for: "dest-ip", text: "Its mesh (NetBird) IP address" }), ip));
    if (state.me.peers_enabled) out.push(el("p", {}, el("a", { href: "#request", onclick: (e) => { e.preventDefault(); r.manual = false; renderStep(); }, text: "Search the mesh instead" })));
  }
  return out;
}
function stepDelivery(r) {
  const out = [el("div", { class: "group-label", text: "Output format" })];
  out.push(el("div", { class: "picks" }, r.topic.outputs.map((o) => el("button", {
    type: "button", class: "pick" + (r.output && r.output.id === o.id ? " on" : ""), onclick: () => { r.output = o; renderStep(); } },
    el("span", { class: "t", text: o.label }),
    el("span", { class: "d mono" }, o.to_format, o.frame ? ` · framing ${o.frame}` : ""),
    o.set && Object.keys(o.set).length ? el("span", { class: "d mono", text: Object.entries(o.set).map(([k, v]) => `${k}=${v}`).join("  ") }) : null))));
  const seg = el("div", { class: "seg", role: "group", "aria-label": "Transport" }, ["udp", "tcp"].map((t) => el("button", {
    type: "button", class: r.transport === t ? "on" : "", onclick: () => { r.transport = t; renderStep(); }, text: t.toUpperCase() })));
  out.push(el("div", { class: "field" }, el("label", { text: "Transport" }), seg,
    el("span", { class: "hint", text: r.transport === "udp" ? "UDP: datagrams to the receiver. The receiver must be listening; nothing comes back." : "TCP: the core node connects to the receiver and keeps the connection open." })));
  out.push(el("div", { class: "field" }, el("label", { for: "port", text: "Port on the receiving system" }),
    el("input", { type: "number", id: "port", min: state.me.port_min, max: state.me.port_max, value: r.port, oninput: (e) => (r.port = e.target.value) }),
    el("span", { class: "hint", text: "The port the receiving system listens on, as its owner configured it." })));
  return out;
}
function stepReview(r) {
  const me = state.me;
  const onNow = me.viewer.is_admin || (r.topic.data_class === "synthetic" && me.self_service_synthetic);
  const sum = el("dl", { class: "summary" },
    el("dt", { text: "Topic" }), el("dd", {}, r.topic.label, " ", cls(r.topic.data_class)),
    el("dt", { text: "Send to" }), el("dd", { text: `${r.dest_name} (${r.dest_ip})` }),
    el("dt", { text: "As" }), el("dd", { text: `${r.output.label} over ${r.transport.toUpperCase()} to port ${r.port}` }));
  const purpose = el("textarea", { id: "purpose", rows: 3, oninput: (e) => (r.purpose = e.target.value) });
  purpose.value = r.purpose;
  const days = el("input", { type: "number", id: "days", min: 1, max: me.max_days, value: r.days, oninput: (e) => (r.days = e.target.value) });
  const next = el("div", { class: "next-box" }, onNow
    ? (me.viewer.is_admin ? `As a ${me.viewer.role.replace("-", " ")}, this switches on as soon as you submit.` : "This topic is synthetic, so it switches on as soon as you submit.")
    : `This sends real data, so it waits for ${me.waits_for}. You can follow it on the feed's page.`);
  return [sum,
    el("div", { class: "field" }, el("label", { for: "purpose", text: "What is it for?" }), purpose,
      el("span", { class: "hint", text: "The approver reads this. Name the system's task and who uses it." })),
    el("div", { class: "field" }, el("label", { for: "days", text: "Run for (days)" }), days,
      el("span", { class: "hint", text: `Up to ${me.max_days} days. It stops on its end date unless an admin renews it.` })),
    next];
}

// ---------- receive steps ----------
function stepInWhat(r) {
  const label = el("input", { type: "text", id: "in-label", value: r.label, placeholder: "Blue C-UAS drone position", oninput: (e) => (r.label = e.target.value) });
  return [
    el("div", { class: "field" }, el("label", { for: "in-label", text: "What does the system send?" }), label,
      el("span", { class: "hint", text: "This becomes the topic's name in the catalog, so others can find it and request it." })),
    el("div", { class: "group-label", text: "The standard it sends" }),
    el("div", { class: "picks" }, state.intake.formats.map((f) => el("button", {
      type: "button", class: "pick" + (r.format && r.format.id === f.id ? " on" : ""), onclick: () => { r.format = f; renderStep(); } },
      el("span", { class: "t", text: f.label }),
      f.note ? el("span", { class: "d", text: f.note }) : null,
      el("span", { class: "d mono", text: f.from_format })))),
    el("p", { class: "hint", text: "Not sure what it sends? A platform admin can run a capture receiver to identify it first." })];
}
function stepInPort(r) {
  const it = state.intake;
  const free = it.ports.filter((p) => p.free);
  if (!r.port && free.length) r.port = String(free[0].port);
  return [
    el("div", { class: "group-label", text: `Receive port on this site's core node (${it.listen_ip})` }),
    el("div", { class: "seg wrap", role: "group", "aria-label": "Receive port" }, it.ports.map((p) => el("button", {
      type: "button", class: String(p.port) === String(r.port) ? "on" : "", disabled: !p.free,
      title: p.free ? "free" : "taken by another feed", onclick: () => { r.port = String(p.port); renderStep(); },
      text: p.free ? String(p.port) : `${p.port} (taken)` }))),
    free.length ? null : el("div", { class: "err", text: "Every receive port is taken. Remove a feed you no longer need, or ask a platform admin for more ports." }),
    el("div", { class: "next-box" }, "The system will send ", el("b", { text: r.format ? r.format.label : "its data" }),
      " over UDP to ", el("b", { class: "mono", text: `${it.listen_ip}:${r.port || "…"}` }), ".")];
}
function stepInReview(r) {
  const me = state.me, it = state.intake;
  const sum = el("dl", { class: "summary" },
    el("dt", { text: "Receive" }), el("dd", {}, r.label, " ", cls("real")),
    el("dt", { text: "From" }), el("dd", { text: `${r.dest_name} (${r.dest_ip})` }),
    el("dt", { text: "Standard" }), el("dd", { text: r.format.label }),
    el("dt", { text: "Sent to" }), el("dd", { class: "mono", text: `${it.listen_ip}:${r.port} UDP` }),
    el("dt", { text: "Published as" }), el("dd", { text: `a topic in the catalog, offered as ${it.outputs.map((o) => o.label).join(", ")}` }));
  const purpose = el("textarea", { id: "purpose", rows: 3, oninput: (e) => (r.purpose = e.target.value) });
  purpose.value = r.purpose;
  const days = el("input", { type: "number", id: "days", min: 1, max: me.max_days, value: r.days, oninput: (e) => (r.days = e.target.value) });
  const next = el("div", { class: "next-box" }, me.viewer.is_admin
    ? `As a ${me.viewer.role.replace("-", " ")}, receiving switches on as soon as you submit. Then tell the system's operator to send to ${it.listen_ip}:${r.port} over UDP.`
    : `Data from a system is real data, so this waits for ${me.waits_for}. Once approved, the system sends to ${it.listen_ip}:${r.port} over UDP.`);
  return [sum,
    el("div", { class: "field" }, el("label", { for: "purpose", text: "What is it, and who uses it?" }), purpose,
      el("span", { class: "hint", text: "The approver reads this." })),
    el("div", { class: "field" }, el("label", { for: "days", text: "Run for (days)" }), days,
      el("span", { class: "hint", text: `Up to ${me.max_days} days. It stops on its end date unless an admin renews it.` })),
    next];
}

// ---------- feed page ----------
async function showFeed(main, id) {
  mount(main, "#tpl-feed");
  let pending = null;
  const load = async () => {
    const d = await api(`/api/feeds/${id}`);
    renderFeed(d.feed, d.events, (action) => { pending = action; askNote(d.feed, action); });
  };
  const askNote = (f, action) => {
    const box = $("#note-box");
    if (["approve", "decline", "remove", "pause", "renew"].includes(action)) {
      box.hidden = false;
      $("#note-label").textContent = action === "decline" ? "Why is it declined? The requester sees this." :
        action === "renew" ? "Days to run from today (number), and an optional note" : `Note for the audit log (optional)`;
      $("#note-input").value = action === "renew" ? String(state.me.default_days) : "";
      $("#note-input").focus();
    } else doAction(f, action, "");
  };
  const doAction = async (f, action, note) => {
    try {
      const body = action === "renew" ? { days: parseInt(note, 10) || state.me.default_days, note: note.replace(/^\s*\d+\s*/, "") } : { note };
      await api(`/api/feeds/${f.id}/${action}`, { body });
      toast(`${verb[action]}: done.`);
      $("#note-box").hidden = true;
      await load();
    } catch (e) { toast(e.message); }
  };
  $("#note-ok").addEventListener("click", async () => {
    const d = await api(`/api/feeds/${id}`);
    doAction(d.feed, pending, $("#note-input").value);
  });
  $("#note-cancel").addEventListener("click", () => ($("#note-box").hidden = true));
  $("#check-btn").addEventListener("click", async () => {
    const out = $("#check-result");
    out.textContent = "Checking…";
    try {
      const r = await api(`/api/feeds/${id}/check`, { body: {} });
      const lines = [r.ping_ok ? "Ping: the system answers." : `Ping: no answer (${r.ping_detail || "timeout"}). The system may be off, or block ping.`];
      if (r.tcp_ok !== undefined) lines.push(r.tcp_ok ? "TCP: the port accepts a connection." : `TCP: the port refused or timed out (${r.tcp_detail}).`);
      if (r.receive_note) lines.push(r.receive_note);
      else if (r.udp_note) lines.push(r.udp_note);
      out.replaceChildren(...lines.map((l) => el("div", { text: l })));
    } catch (e) { out.textContent = e.message; }
  });
  try { await load(); } catch (e) { main.replaceChildren(el("p", { class: "err", text: e.message })); return; }
  state.timer = setInterval(() => load().catch(() => {}), 4000);
}
function renderFeed(f, events, onAction) {
  $("#feed-head").replaceChildren(
    el("h1", { text: title(f) }),
    el("div", { class: "meta" }, health(f.health), cls(f.data_class), el("span", { class: "mono", text: f.id })),
    f.health_note ? el("p", { class: "why", text: f.health_note }) : null);
  $("#feed-actions").replaceChildren(...f.actions.filter((a) => a !== "check").map((a) =>
    el("button", { type: "button", class: "btn" + (a === "approve" ? " primary" : ""), onclick: () => onAction(a), text: verb[a] })));
  const d = f.delivery;
  const stat = (k, v) => el("div", {}, el("dt", { text: k }), el("dd", { text: v }));
  $("#check-btn").textContent = isIn(f) ? "Check sender" : "Check destination";
  $("#flow-title").textContent = isIn(f) ? "Receiving" : "Delivery";
  $("#feed-stats").replaceChildren(
    stat(isIn(f) ? "Received" : "In from topic", d ? String(d.stats.in) : "—"),
    stat(isIn(f) ? "Published" : "Sent", d ? String(isIn(f) ? d.stats.converted : d.sent_estimate) : "—"),
    stat(isIn(f) ? "Not readable" : "Failed to convert", d ? String(d.stats.failed) : "—"),
    stat("Last message", d ? ago(d.last_in_change_at) : "—"),
    stat("Running since", d && d.running ? ago(d.started_at).replace(" ago", "") : "not running"),
    stat("Restarts", d ? String(d.restarts) : "—"));
  const kv = (k, v) => [el("dt", { text: k }), el("dd", { text: v })];
  const where = isIn(f)
    ? [...kv("Sender", `${f.dest_name} · ${f.dest_ip}`), ...kv("Sends to", `${f.listen_at || f.port} (${f.output_label}, ${f.to_format})`),
       ...kv("Published on", f.published_key || "—"), ...kv("Offered as topic", f.published_topic_id ? `${f.topic_label} (${f.published_topic_id})` : "—")]
    : [...kv("Destination", dest(f)), ...kv("Format", `${f.output_label} (${f.to_format})`)];
  $("#feed-details").replaceChildren(
    ...where,
    ...kv("Owner", f.owner),
    ...kv("Purpose", f.purpose),
    ...kv("State", f.status),
    ...kv("Decided by", f.decided_by ? `${f.decided_by}${f.decision_note ? " — " + f.decision_note : ""}` : "—"),
    ...kv("Requested", fmtTime(f.created_at)),
    ...kv("Ends", `${fmtTime(f.end_at)} (${daysLeft(f.end_at)})`));
  const tr = $("#trouble");
  if (!d) {
    tr.replaceChildren(el("p", { class: "empty", text: f.status === "active" ? "The delivery loop has not picked this feed up yet; it runs every few seconds." : "Nothing is being delivered: the feed is " + f.status + "." }));
  } else {
    tr.replaceChildren(
      isIn(f) ? el("p", { text: `This feed has its own receiving process. "Received" counts datagrams from the sender; "Not readable" counts ones that did not read as ${f.output_label}. The receive port accepts from any address the mesh lets through.` }) :
      el("p", {}, d.shared_with ? `This feed shares one delivery process with ${d.shared_with} other feed(s) of the same topic and format. Counters are for that process; "Sent" is its conversions divided by its outputs.` : "This feed has its own delivery process."),
      d.last_error ? el("div", { class: "err" }, el("b", { text: "Last error " + ago(d.last_error_at) + ": " }), d.last_error) : el("p", { text: isIn(f) ? "No error lines from the receiving process." : "No error lines from the delivery process." }),
      el("div", { class: "group-label", text: "Command" }),
      el("pre", { class: "cmd", text: (d.command || []).join(" ") }),
      el("div", { class: "group-label", text: isIn(f) ? "Recent output of the receiving process" : "Recent output of the delivery process" }),
      el("pre", { class: "log", text: (d.lines || []).join("\n") || "(no output yet)" }));
  }
  $("#timeline").replaceChildren(...events.map((e) => el("li", {},
    el("span", { class: "when", text: fmtTime(e.at) }),
    el("span", {}, el("b", { text: e.action }), ` by ${e.actor}`, e.detail ? ` — ${e.detail}` : ""))));
}

// ---------- approvals ----------
async function showApprovals(main) {
  mount(main, "#tpl-approvals");
  const render = async () => {
    await refreshFeeds();
    const rows = state.feeds.filter((f) => f.status === "pending");
    $("#approval-list").replaceChildren(...(rows.length ? rows.map((f) => {
      const note = el("input", { type: "text", placeholder: "Note (needed to decline)", "aria-label": "Decision note" });
      const act = async (action) => {
        try { await api(`/api/feeds/${f.id}/${action}`, { body: { note: note.value } }); toast(`${verb[action]}: done.`); render(); }
        catch (e) { toast(e.message); }
      };
      return el("div", { class: "card" }, health(f.health),
        el("div", { class: "main" },
          el("div", { class: "line1" }, el("span", { class: "dir", text: isIn(f) ? "IN" : "OUT" }), el("a", { href: `#feed/${f.id}`, text: title(f) })),
          el("div", { class: "line2", text: isIn(f) ? `${f.output_label} from ${f.dest_ip}, received on port ${f.port} · asked by ${f.owner} ${ago(f.created_at)}`
            : `${f.output_label} to ${f.dest_ip}:${f.port}/${f.transport} · asked by ${f.owner} ${ago(f.created_at)}` }),
          el("div", { class: "line2", text: `Purpose: ${f.purpose}` })),
        el("div", { class: "side" }, cls(f.data_class)),
        el("div", { class: "acts" }, note,
          el("button", { type: "button", class: "btn primary small", onclick: () => act("approve"), text: "Approve" }),
          el("button", { type: "button", class: "btn small", onclick: () => act("decline"), text: "Decline" })));
    }) : [el("p", { class: "empty", text: "Nothing is waiting for approval." })]));
  };
  await render();
  state.timer = setInterval(() => render().catch(() => {}), 8000);
}

// ---------- audit ----------
async function showAudit(main) {
  mount(main, "#tpl-audit");
  const d = await api("/api/audit");
  $("#audit-body").replaceChildren(...(d.events || []).map((e) => el("tr", {},
    el("td", { text: fmtTime(e.at) }), el("td", { text: e.actor }), el("td", { text: e.action }),
    el("td", {}, el("a", { href: `#feed/${e.feed_id}`, text: e.feed_id })), el("td", { text: e.detail }))));
}

// ---------- start ----------
window.addEventListener("hashchange", route);
(async () => {
  try { await loadMe(); route(); }
  catch (e) { if (e.message !== "sign in") $("#main").replaceChildren(el("div", { class: "err", text: "Could not load: " + e.message })); }
})();
