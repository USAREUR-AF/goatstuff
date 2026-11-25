// SPDX-License-Identifier: Apache-2.0
// Fabric feeds — plain JavaScript, no framework, no build step.
// Every value from the server is placed with textContent; nothing is parsed as HTML.
"use strict";

const state = { me: null, catalog: [], intake: { enabled: false }, feeds: [], filter: "mine", search: "", timer: null, req: null,
  site: "", role: "", group: "", view: "list", open: new Set(), naming: null, tidyCount: 0 };

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
// Two separate facts per feed, never one word: is the link set up, and is data going through.
const signal = (kind, label, sg) => el("div", { class: `sig ${kind} ${sg && sg.on ? "yes" : "no"}` },
  el("span", { class: "k", text: label }),
  el("span", { class: "w", text: sg ? sg.word : "—" }),
  el("span", { class: "n", text: sg ? sg.note : "" }));
const signals = (f) => [signal("int", "Integrated", f.integrated), signal("flow", isIn(f) ? "Receiving" : "Sending", f.flowing)];
const cls = (c) => el("span", { class: `cls ${c}`, text: c === "real" ? "Real data" : "Synthetic" });
const dest = (f) => `${f.dest_name} · ${f.dest_ip}:${f.port}/${f.transport}`;
const isIn = (f) => f.direction === "in";
const isArchived = (f) => !!(f.facets && f.facets.archived);
const title = (f) => isIn(f) ? `${f.topic_label}  ←  ${f.dest_name}` : `${f.topic_label || f.topic_id}  →  ${f.dest_name}`;
const verb = { approve: "Approve", decline: "Decline", pause: "Pause", resume: "Resume", renew: "Renew", remove: "Remove",
  "sim-status": "Real or simulated", check: "Check destination" };
// Real or simulated: what a received feed's own team declares its source is doing. Shown beside
// the data; it never changes what is sent. One sentence, the same everywhere it appears.
const simWord = { real: "real", simulated: "simulated", unknown: "unknown" };
function simText(sm) {
  if (!sm || !sm.mode) return "Not declared";
  return `Declared ${simWord[sm.mode] || sm.mode} by ${sm.set_by}, ${fmtTime(sm.set_at)}` + (sm.note ? `: ${sm.note}` : "");
}
const simLine = (sm, prefix = "") => el("div", { class: "line2 simline" + (sm && sm.mode ? " " + sm.mode : " none"), text: prefix + simText(sm) });

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
  if (view === "tidy") return showTidy(main);
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
  $("#nav-tidy").hidden = !v.is_admin;
  await loadCatalog();
  refreshTidyCount();
}
// The Tidy up badge: how many systems need organising (admins only).
async function refreshTidyCount() {
  if (!state.me || !state.me.viewer.is_admin) return;
  try {
    const d = await api("/api/tidy");
    state.tidyCount = d.count || 0;
    const c = $("#tidy-count");
    c.textContent = state.tidyCount ? String(state.tidyCount) : "";
    c.hidden = !state.tidyCount;
  } catch { /* shown next time */ }
}
async function loadNaming() {
  try { state.naming = await api("/api/naming"); } catch { state.naming = null; }
  return state.naming;
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
  refreshTidyCount();
  return state.feeds;
}

// ---------- facets, grouping and the page address (fabric-feeds.md §11) ----------
const uniqv = (xs) => [...new Set(xs.filter((x) => x))];
function facetValues(f, facet) {
  const fc = f.facets || {};
  switch (facet) {
    case "site": return uniqv([...(fc.source_sites || []), fc.dest_site]);
    case "region": return uniqv([...(fc.source_regions || []), fc.dest_region]);
    case "source": return uniqv(fc.source_systems || []);
    case "dest_site": return uniqv([fc.dest_site]);
    case "dest_system": return uniqv([fc.dest_system || f.dest_name]);
    case "dest_role": return uniqv([fc.dest_role]);
    case "format": return uniqv([f.output_label]);
    case "owner": return uniqv([f.owner]);
  }
  return [];
}
function countText(rows) {
  // Independent counts: a link can be sending and still need attention.
  const c = { attention: 0, sending: 0, "at-rest": 0, idle: 0, off: 0, archived: 0 };
  rows.forEach((f) => {
    if (isArchived(f)) { c.archived++; return; }
    if (f.facets && f.facets.attention) c.attention++;
    c[f.flowing && f.flowing.on ? "sending" : f.facets && f.facets.at_rest ? "at-rest" : f.integrated && f.integrated.on ? "idle" : "off"]++;
  });
  const parts = [`${rows.length} ${rows.length === 1 ? "link" : "links"}`];
  if (c.sending) parts.push(`${c.sending} sending`);
  if (c.attention) parts.push(`${c.attention} need attention`);
  if (c["at-rest"]) parts.push(`${c["at-rest"]} at rest`);
  if (c.off) parts.push(`${c.off} off`);
  if (c.archived) parts.push(`${c.archived} archived`);
  return parts.join(" · ");
}
// The page address carries every filter: #feeds?filter=attention&site=Alpha&group=site&view=flow&q=...
function readAddress() {
  const p = new URLSearchParams(location.hash.split("?")[1] || "");
  for (const k of ["filter", "site", "role", "group", "view"]) if (p.has(k)) state[k] = p.get(k);
  if (p.has("q")) state.search = p.get("q");
  return p.has("filter");
}
function writeAddress() {
  const p = new URLSearchParams();
  for (const k of ["filter", "site", "role", "group", "view"]) if (state[k] && !(k === "view" && state[k] === "list")) p.set(k, state[k]);
  if (state.search) p.set("q", state.search);
  const h = "#feeds" + (p.toString() ? "?" + p.toString() : "");
  if (location.hash !== h) history.replaceState(null, "", h);
}
function fillSelect(sel, values, current, allLabel) {
  const keep = sel.value;
  sel.replaceChildren(el("option", { value: "", text: allLabel }), ...values.map((v) => el("option", { value: v, text: v })));
  sel.value = values.includes(current) ? current : "";
  if (sel.value !== keep && !current) sel.value = "";
}
function renderFacetBar() {
  const all = state.feeds.filter((f) => f.status !== "removed");
  fillSelect($("#f-site"), uniqv(all.flatMap((f) => facetValues(f, "site"))).sort(), state.site, "All sites");
  fillSelect($("#f-role"), uniqv(all.flatMap((f) => facetValues(f, "dest_role"))).sort(), state.role, "All roles");
  $("#f-group").value = state.group;
}
function groupRows(rows, facet) {
  const groups = new Map();
  for (const f of rows) {
    const vals = facetValues(f, facet);
    for (const v of vals.length ? vals : ["(undeclared)"]) {
      if (!groups.has(v)) groups.set(v, []);
      groups.get(v).push(f);
    }
  }
  return [...groups.entries()].sort((a, b) => a[0].localeCompare(b[0]));
}

// ---------- feeds list ----------
function matches(f) {
  const me = state.me.viewer.email.toLowerCase();
  if (state.filter === "mine" && (f.kind === "declared" || f.owner.toLowerCase() !== me)) return false;
  if (state.filter === "attention" && !(f.facets && f.facets.attention)) return false;
  if (state.filter === "archived" && !isArchived(f)) return false;
  if (state.site && !facetValues(f, "site").some((v) => v.toLowerCase() === state.site.toLowerCase())) return false;
  if (state.role && (facetValues(f, "dest_role")[0] || "") !== state.role) return false;
  if (state.filter === "waiting" && f.status !== "pending") return false;
  if (state.filter === "live" && !["active", "paused"].includes(f.status)) return false;
  if (state.filter === "all" && f.status === "removed") return false;
  const q = state.search.toLowerCase();
  if (!q) return true;
  const fc = f.facets || {};
  return [f.topic_label, f.dest_name, f.dest_ip, String(f.port), f.owner, f.output_label, f.id, f.label, fc.dest_system, ...(fc.source_systems || [])]
    .some((x) => (x || "").toLowerCase().includes(q));
}
function declaredCard(f) {
  const d = f.delivery;
  const side = d && d.stats ? `${d.stats.in} in · ${d.stats.converted} out` : "";
  return el("div", { class: "card feed-row declared" + (isArchived(f) ? " archived" : "") },
    ...signals(f),
    el("div", { class: "main" },
      el("div", { class: "line1" }, el("span", { class: "dir", text: "OUT" }), el("span", { class: "dir decl", text: "DECLARED" }), title(f)),
      el("div", { class: "line2", text: `${f.output_label} to ${f.dest_ip}:${f.port}/${f.transport} · ${f.owner}` }),
      el("div", { class: "line2", text: `Controlled by ${f.controlled_by || "its deployment file"} (manifest ${f.manifest}). Change it there, not here.` }),
      facetLine(f)),
    el("div", { class: "side" }, cls(f.data_class), el("br"), side));
}
function feedCard(f) {
  if (f.kind === "declared") return declaredCard(f);
  const d = f.delivery;
  const side = f.status === "active" && d && d.stats ? (isIn(f) ? `${d.stats.in} received · ${d.stats.converted} published` : `${d.stats.in} in · ${d.sent_estimate} sent`)
    : f.status === "pending" ? `asked ${ago(f.created_at)}` : "";
  const line2 = isIn(f) ? `${f.output_label} from ${f.dest_ip}, received on ${f.listen_at || "port " + f.port} · ${f.owner} · ${daysLeft(f.end_at)}`
    : `${f.output_label || f.output_id} to ${f.dest_ip}:${f.port}/${f.transport} · ${f.owner} · ${daysLeft(f.end_at)}`;
  return el("a", { class: "card feed-row" + (isArchived(f) ? " archived" : ""), href: `#feed/${f.id}` },
    ...signals(f),
    el("div", { class: "main" },
      el("div", { class: "line1" }, el("span", { class: "dir", text: isIn(f) ? "IN" : "OUT" }), title(f)),
      el("div", { class: "line2", text: line2 }),
      isIn(f) ? simLine(f.sim_status, "Real or simulated: ") : null,
      facetLine(f)),
    el("div", { class: "side" }, cls(f.data_class), el("br"), side));
}
// Where the destination is: its site, kind and role, or for a boundary node where the
// data goes beyond it. The bare words "boundary node" never appear without the far side.
function destText(fc) {
  if (fc.boundary) return [fc.dest_site, fc.boundary].filter((x) => x).join(" · ");
  return [fc.dest_site || "site not in the estate", fc.dest_class, fc.dest_role].filter((x) => x).join(" · ");
}
function facetLine(f) {
  const fc = f.facets || {};
  const from = (fc.source_sites || []).join(" + ") || (isIn(f) ? "" : "source not in the estate");
  const to = destText(fc) + (fc.dest_placed_by ? ` (placed by ${fc.dest_placed_by})` : "");
  const why = fc.attention ? el("span", { class: "why", text: "Needs attention: " + fc.reasons.join("; ") + "." }) : null;
  const arch = fc.archived ? el("span", { class: "arch", text: fc.archive_note }) : null;
  return el("div", { class: "line2 facetline" }, isIn(f) ? `From ${to}` : `${from} → ${to}`,
    why ? el("br") : null, why, arch ? el("br") : null, arch);
}
function renderList() {
  const list = $("#feed-list");
  if (!list) return;
  const rows = state.feeds.filter(matches);
  const flow = $("#feed-flow");
  const view = state.view || "list";
  document.querySelectorAll(".viewseg button").forEach((b) => b.classList.toggle("on", b.dataset.view === view));
  list.hidden = view !== "list";
  if (flow) flow.hidden = view !== "flow";
  if (view === "flow" && flow) renderFlow(flow, rows);
  const cnt = $("#f-counts");
  if (cnt) cnt.textContent = countText(rows);
  writeAddress();
  if (view === "list" && state.group && rows.length) {
    // Archived links get their own group, last and collapsed.
    const live = rows.filter((f) => !isArchived(f)), arch = rows.filter(isArchived);
    const groups = groupRows(live, state.group).map(([key, gr]) => [key, gr, false]);
    if (arch.length) groups.push(["Archived systems", arch, true]);
    list.replaceChildren(...groups.map(([key, gr, isArch]) => {
      const okey = isArch ? "\u0000archived" : key;
      const open = state.open.has(okey) || (!isArch && gr.some((f) => f.facets && f.facets.attention));
      const d = el("details", { class: "group" + (isArch ? " archived" : ""), ...(open ? { open: "" } : {}) },
        el("summary", {}, el("span", { class: "gkey", text: key }), el("span", { class: "gcount", text: countText(gr) })),
        el("div", { class: "list" }, ...gr.map(feedCard)));
      d.addEventListener("toggle", () => { if (d.open) state.open.add(okey); else state.open.delete(okey); });
      return d;
    }));
    document.querySelectorAll(".chip").forEach((c) => c.classList.toggle("on", c.dataset.filter === state.filter));
    return;
  }
  list.replaceChildren(...(rows.length ? rows.map(feedCard) : [el("p", { class: "empty", text:
    state.filter === "attention" ? "Nothing needs attention. Choose All to see every link." :
    state.filter === "archived" ? "No links to or from archived systems." :
    state.filter === "mine" ? "You have no feeds yet. Request one to send a topic to a system, or to bring a system's data into the fabric." : "No feeds match." })]));
  document.querySelectorAll(".chip").forEach((c) => c.classList.toggle("on", c.dataset.filter === state.filter));
}
async function showFeeds(main) {
  mount(main, "#tpl-feeds");
  const fromAddress = readAddress();
  if (!fromAddress && state.me.viewer.is_admin && state.filter === "mine" && !showFeeds._set) { state.filter = "attention"; showFeeds._set = true; }
  document.querySelectorAll(".chip").forEach((c) => c.addEventListener("click", () => { state.filter = c.dataset.filter; renderList(); }));
  const s = $("#feed-search");
  s.value = state.search;
  s.addEventListener("input", () => { state.search = s.value; renderList(); });
  document.querySelectorAll(".viewseg button").forEach((b) => b.addEventListener("click", () => { state.view = b.dataset.view; renderList(); }));
  $("#f-site").addEventListener("change", (e) => { state.site = e.target.value; renderList(); });
  $("#f-role").addEventListener("change", (e) => { state.role = e.target.value; renderList(); });
  $("#f-group").addEventListener("change", (e) => { state.group = e.target.value; renderList(); });
  await refreshFeeds();
  renderFacetBar();
  renderList();
  state.timer = setInterval(async () => { try { await refreshFeeds(); renderFacetBar(); renderList(); } catch { /* shown next time */ } }, 5000);
}

// ---------- flow view: sensors -> streams -> receiving systems ----------
const SVGNS = "http://www.w3.org/2000/svg";
function sv(tag, attrs, ...kids) {
  const n = document.createElementNS(SVGNS, tag);
  for (const [k, v] of Object.entries(attrs || {})) n.setAttribute(k, v);
  for (const k of kids) if (k != null) n.append(k);
  return n;
}
const clip = (t, n) => (t.length > n ? t.slice(0, n - 1) + "…" : t);
// One sensor name for the same radar however a label spells it ("Radar X tracks, Site", "Radar X, Site (raw)").
const sensorName = (t) => t.replace(/ tracks,/, ",").replace(/ \(raw\)$/, "").trim();
const rank = (f) => (f.flowing && f.flowing.on ? 2 : f.integrated && f.integrated.on ? 1 : 0);
function flowGraph(rows) {
  const cols = [new Map(), new Map(), new Map()];
  const edges = new Map();
  const node = (c, id, l1, l2, search) => {
    if (!cols[c].has(id)) cols[c].set(id, { id, c, l1, l2, search, best: 0, members: new Set() });
    return cols[c].get(id);
  };
  // An edge keeps its best state and how many links it stands for, and how many of them send.
  const edge = (a, b, st, link) => {
    const k = a.c + ":" + a.id + ">" + b.c + ":" + b.id;
    const e = edges.get(k) || { a, b, st: 0, links: new Set(), on: new Set() };
    e.st = Math.max(e.st, st); e.links.add(link); if (st === 2) e.on.add(link);
    edges.set(k, e);
  };
  // Grouped: sensors collapse into one box per site, receivers into one box per group value.
  const g = state.group;
  const srcBox = (s) => {
    const name = sensorName(s.label || String(s));
    const siteLabel = s.site_label || s.site || "";
    if (!g) return node(0, "src:" + name, name, siteLabel, name);
    const site = siteLabel || "Site not in the estate";
    const n = node(0, "srcsite:" + site, "Sensors, " + site, "", site);
    n.members.add(name); n.l2 = [...n.members].join(", ");
    return n;
  };
  const dstBox = (f) => {
    const fc = f.facets || {};
    if (fc.boundary && (!g || ["source", "format", "owner"].includes(g))) {
      // "To <far side>" on the first line, "through boundary node <name>" on the second.
      const i = fc.boundary.indexOf(" through boundary node ");
      const l1 = i > 0 ? fc.boundary.slice(0, i) : "Far side not declared";
      const l2 = i > 0 ? fc.boundary.slice(i + 1) : fc.boundary;
      const n = node(2, "d:" + f.dest_ip + ":" + f.port, l1, l2, f.dest_ip);
      n.full = fc.boundary;
      return n;
    }
    if (!g || ["source", "format", "owner"].includes(g)) return node(2, "d:" + f.dest_ip + ":" + f.port, f.dest_name || f.dest_ip, `${f.dest_ip}:${f.port}/${f.transport}`, f.dest_ip);
    const facet = g === "site" ? "dest_site" : g === "region" ? "region" : g;
    const v = facetValues({ ...f, facets: { ...(f.facets || {}), source_sites: [], source_regions: [] } }, facet)[0] || "Not in the estate";
    const n = node(2, "dg:" + v, v, "", v);
    n.members.add(f.dest_name || f.dest_ip); n.l2 = `${n.members.size} ${n.members.size === 1 ? "system" : "systems"}: ${[...n.members].join(", ")}`;
    return n;
  };
  for (const f of rows) {
    if (["removed", "declined", "expired"].includes(f.status)) continue;
    const st = rank(f);
    if (isIn(f)) {
      const snd = node(0, "sys:" + f.dest_ip, f.dest_name || f.dest_ip, f.dest_ip, f.dest_ip);
      const str = node(1, "in:" + f.id, f.topic_label, `${f.output_label} · received on port ${f.port}`, f.topic_label);
      const fab = node(2, "fabric", "goat fabric", "offered in the catalog", "IN");
      edge(snd, str, st, f.id); edge(str, fab, st, f.id);
      continue;
    }
    const label = f.topic_label || f.topic_id;
    const str = node(1, "s:" + label + "|" + f.output_label, label, f.output_label || f.output_id, label);
    const dst = dstBox(f);
    edge(str, dst, st, f.id);
    const srcs = (f.sources && f.sources.length) ? f.sources : [{ label: sensorName(label) }];
    for (const s of srcs) edge(srcBox(s), str, st, f.id);
  }
  for (const e of edges.values()) { e.a.best = Math.max(e.a.best, e.st); e.b.best = Math.max(e.b.best, e.st); }
  // Order: streams by label; sensors and destinations by the average position of the streams they touch.
  const order = [[], [...cols[1].values()].sort((x, y) => x.l1.localeCompare(y.l1)), []];
  order[1].forEach((n, i) => (n.y = i));
  for (const c of [0, 2]) {
    const ns = [...cols[c].values()];
    for (const n of ns) {
      const ys = [...edges.values()].filter((e) => e.a === n || e.b === n).map((e) => (e.a === n ? e.b : e.a).y).filter((y) => y != null);
      n.bary = ys.length ? ys.reduce((a, b) => a + b, 0) / ys.length : 0;
    }
    order[c] = ns.sort((x, y) => x.bary - y.bary || x.l1.localeCompare(y.l1));
  }
  return { order, edges: [...edges.values()] };
}
function renderFlow(box, rows) {
  const { order, edges } = flowGraph(rows);
  if (!order[1].length) { box.replaceChildren(el("p", { class: "empty", text: "No feeds match." })); return; }
  const W = 1080, colX = [0, 400, 800], NW = 280, NH = 50, GAP = 16, TOP = 34;
  const most = Math.max(...order.map((o) => o.length));
  const H = TOP + most * (NH + GAP);
  order.forEach((ns) => { const off = TOP + (most - ns.length) * (NH + GAP) / 2; ns.forEach((n, i) => { n.x = colX[n.c]; n.py = off + i * (NH + GAP); }); });
  const svg = sv("svg", { viewBox: `0 0 ${W} ${H}`, class: "flowsvg", role: "img",
    "aria-label": `Flow of ${rows.length} feeds: sensors and senders, the streams they become, and the systems that receive them.` });
  ["Sensors and senders", "Streams, as converted", "Receiving systems"].forEach((t, i) =>
    svg.append(sv("text", { x: colX[i], y: 14, class: "fhead" }, t)));
  const cls3 = ["off", "idle", "on"];
  for (const e of edges.sort((p, q) => p.st - q.st)) {
    const x1 = e.a.x + NW, y1 = e.a.py + NH / 2, x2 = e.b.x, y2 = e.b.py + NH / 2, mx = (x1 + x2) / 2;
    svg.append(sv("path", { d: `M${x1},${y1} C${mx},${y1} ${mx},${y2} ${x2},${y2}`, class: `fedge ${cls3[e.st]}` }));
    if (e.links.size > 1) svg.append(sv("text", { x: mx, y: (y1 + y2) / 2 - 5, class: "fcount", "text-anchor": "middle" }, `${e.on.size} of ${e.links.size} sending`));
  }
  for (const ns of order) for (const n of ns) {
    const g = sv("g", { class: `fnode c${n.c} ${cls3[n.best]}`, tabindex: "0", role: "button", "aria-label": `${n.full || n.l1 + " " + n.l2}: show in the list` },
      sv("title", {}, n.full || `${n.l1}${n.l2 ? " · " + n.l2 : ""}`),
      sv("rect", { x: n.x, y: n.py, width: NW, height: NH, rx: 8 }),
      sv("circle", { cx: n.x + 14, cy: n.py + NH / 2, r: 5, class: "fdot" }),
      sv("text", { x: n.x + 28, y: n.py + (n.l2 ? 21 : 30), class: "fl1" }, clip(n.l1, 34)),
      n.l2 ? sv("text", { x: n.x + 28, y: n.py + 38, class: "fl2" }, clip(n.l2, 40)) : null);
    const go = () => { state.search = n.search; $("#feed-search").value = n.search; state.view = "list"; renderList(); };
    g.addEventListener("click", go);
    g.addEventListener("keydown", (ev) => { if (ev.key === "Enter" || ev.key === " ") { ev.preventDefault(); go(); } });
    svg.append(g);
  }
  const legend = el("div", { class: "flegend" },
    el("span", { class: "lg on", text: "Sending" }), el("span", { class: "lg idle", text: "Integrated, nothing sent in the last minute" }),
    el("span", { class: "lg off", text: "Not integrated" }), el("span", { class: "lg hint", text: "Select a box to see its feeds in the list." }));
  box.replaceChildren(legend, el("div", { class: "flowwrap" }, svg));
}

// ---------- request flow ----------
function newReq(dir) {
  return { dir: dir || "out", step: 1, topic: null, output: null, format: null, label: "", peer: null, manual: false, dest_name: "", dest_ip: "",
           transport: "udp", port: "", days: state.me.default_days, purpose: "", error: "",
           resolved: null, resolvedFor: "",
           declare: { name: "", site: "", class: "", role: "", label: "", far_label: "", far_dir: "out", named: false, check: null, taken: false } };
}
// ---------- the declare rule: a destination must be a declared system ----------
const enc = encodeURIComponent;
const destKey = (r) => `${r.dest_name.trim()}|${r.dest_ip.trim()}`;
async function resolveDest(r) {
  const key = destKey(r);
  if (r.resolvedFor === key && r.resolved) return r.resolved;
  try {
    r.resolved = await api(`/api/systems/resolve?direction=${r.dir}&name=${enc(r.dest_name.trim())}&ip=${enc(r.dest_ip.trim())}`);
  } catch { r.resolved = { enforced: false, found: false }; }
  r.resolvedFor = key;
  if (needsDeclare(r)) prefillDeclare(r);
  return r.resolved;
}
// A member's earlier proposal for the same system already waits in Tidy up.
const proposed = (r) => !!(r.resolved && r.resolved.proposal && !state.me.viewer.is_admin);
const needsDeclare = (r) => !!(r.resolved && r.resolved.enforced && !r.resolved.found && !proposed(r));
function prefillDeclare(r) {
  const d = r.declare, p = r.resolved && r.resolved.proposal;
  if (p && !d.name) Object.assign(d, { name: p.key, site: p.site || "", class: p.class || "", role: p.role || "", label: p.label || "",
    far_label: p.far_side ? p.far_side.label : "", far_dir: p.far_side ? p.far_side.direction || "out" : "out", named: true });
  if (!d.label) d.label = r.dest_name.trim();
}
const siteLabelOf = (id) => { const s = ((state.naming && state.naming.sites) || []).find((x) => x.id === id); return s ? s.label : id; };
function systemLine(sys) {
  if (!sys) return "";
  const parts = [sys.label, sys.site_label || sys.site, sys.boundary || [sys.class, sys.role].filter((x) => x).join(" · ")];
  if (sys.placed_by) parts.push(`placed by ${sys.placed_by}`);
  return parts.filter((x) => x).join(" · ");
}
// The fields that declare a new system: site, kind, function, label and a machine name
// that follows the naming rule (checked against GET /api/naming as it is typed).
function declarePanel(r) {
  const n = state.naming || { sites: [], roles: [], classes: [], rule: "" };
  const d = r.declare;
  const admin = state.me.viewer.is_admin;
  const field = (id, label, input, hint, wide) => el("div", { class: "field" + (wide ? " wide" : "") },
    el("label", { for: id, text: label }), input, hint ? el("span", { class: "hint", text: hint }) : null);
  const select = (id, opts, cur, blank, on) => {
    const sl = el("select", { id, onchange: (e) => on(e.target.value) }, el("option", { value: "", text: blank }),
      ...opts.map((o) => el("option", { value: o.id || o, text: o.label ? `${o.label} (${o.id})` : o })));
    sl.value = cur || "";
    return sl;
  };
  const checkBox = el("div", { id: "decl-check", class: "wide" });
  const showCheck = () => {
    if (!d.check) return checkBox.replaceChildren();
    if (d.check.ok && !d.taken) {
      const stale = document.querySelector("#step-body > .err[role=alert]");
      if (stale && /naming rule|machine name/.test(stale.textContent)) stale.remove();
      return checkBox.replaceChildren(el("span", { class: "ok-note", text: "Follows the naming rule." }));
    }
    const probs = [...(d.check.problems || []), ...(d.taken ? ["That name is already a declared system; choose the next number."] : [])];
    checkBox.replaceChildren(el("ul", { class: "problems" }, ...probs.map((p) => el("li", { text: p }))));
  };
  let ct;
  const check = () => {
    clearTimeout(ct);
    ct = setTimeout(async () => {
      try { const x = await api(`/api/naming?name=${enc(d.name)}&site=${enc(d.site)}`); d.check = x.check; d.taken = x.taken; } catch { d.check = null; }
      showCheck();
    }, 200);
  };
  const nameIn = el("input", { type: "text", id: "decl-name", value: d.name, placeholder: n.example || "console-north-ops-01", autocomplete: "off",
    oninput: (e) => { d.name = e.target.value.trim(); d.named = true; check(); } });
  const suggest = async () => {
    if (d.named || !d.site || !d.class) return;
    try { const x = await api(`/api/naming?kind=${enc(d.class)}&site=${enc(d.site)}&function=${enc(d.role || "ops")}`); d.name = x.suggestion; nameIn.value = d.name; check(); } catch { /* typed by hand */ }
  };
  const farBox = el("div", { class: "wide", id: "decl-far-box" });
  const showFar = () => {
    if (d.class !== "boundary") return farBox.replaceChildren();
    const far = el("input", { type: "text", id: "decl-far", value: d.far_label, placeholder: "Partner network", oninput: (e) => (d.far_label = e.target.value) });
    const dir = select("decl-far-dir", ["out", "in", "both"], d.far_dir, "Direction", (v) => (d.far_dir = v || "out"));
    farBox.replaceChildren(field("decl-far", "Where does the data go beyond it? (far side)", far,
      "A boundary node passes data to another network. Name that network, so nobody sends data without knowing where it ends up."),
      field("decl-far-dir", "Direction", dir, "out: data leaves through it; in: data arrives through it."));
  };
  const roles = el("datalist", { id: "decl-roles" }, ...(n.roles || []).map((x) => el("option", { value: x })));
  const role = el("input", { type: "text", id: "decl-role", list: "decl-roles", value: d.role, placeholder: "ops",
    oninput: (e) => { d.role = e.target.value; suggest(); } });
  const panel = el("div", { class: "declare", id: "declare" },
    el("h3", { text: "Declare this system" }),
    el("p", { class: "hint wide", text: `${r.dest_name || r.dest_ip} (${r.dest_ip}) is not in this goatnet's estate. Say where it is and what it does, so its links are grouped and checked. ` +
      (admin ? "As an admin, it is placed in the estate when you submit." : "It is sent with your request; an admin places it in the estate (Tidy up).") }),
    field("decl-site", "Site", select("decl-site", n.sites || [], d.site, "Choose the site", (v) => { d.site = v; suggest(); check(); })),
    field("decl-class", "Kind of system", select("decl-class", n.classes || [], d.class, "Choose the kind", (v) => { d.class = v; showFar(); suggest(); })),
    field("decl-role", "Function", role, "Its job, for example ops, cue or relay."), roles,
    field("decl-label", "Display label", el("input", { type: "text", id: "decl-label", value: d.label, oninput: (e) => (d.label = e.target.value) }), "The name people know it by."),
    field("decl-name", "Machine name", nameIn, n.rule ? "Rule: " + n.rule : "", true),
    checkBox, farBox);
  showFar();
  if (d.name) check(); else suggest();
  return panel;
}
function declareProblem(r) {
  if (!needsDeclare(r)) return "";
  const d = r.declare;
  if (!d.site) return "Choose the site the system is at.";
  if (!d.class) return "Choose what kind of system it is.";
  if (!d.role.trim()) return "Choose the system's function, for example ops, cue or relay.";
  if (!d.name) return "Give the system a machine name that follows the naming rule.";
  if (d.check && !d.check.ok) return "The machine name does not follow the naming rule: " + d.check.problems[0];
  if (d.taken) return "That machine name is already a declared system; choose the next number.";
  if (d.class === "boundary" && !d.far_label.trim()) return "Say where data through this boundary node goes (the far side).";
  return "";
}
function declareBody(r) {
  if (!needsDeclare(r)) return undefined;
  const d = r.declare;
  return { name: d.name, site: d.site, class: d.class, role: d.role.trim(), label: d.label.trim(),
    far_side: d.class === "boundary" ? { label: d.far_label.trim(), direction: d.far_dir || "out" } : undefined };
}
// What the review step says about the destination.
function destReview(r) {
  if (r.resolved && r.resolved.found) return systemLine(r.resolved.system);
  if (proposed(r)) return `Proposed by ${r.resolved.proposal.by} as ${r.resolved.proposal.key}; it waits for an admin in Tidy up.`;
  if (needsDeclare(r)) {
    const d = r.declare;
    const where = d.class === "boundary" ? `To ${d.far_label.trim()} through boundary node ${d.label.trim() || d.name}` : `${d.class} · ${d.role.trim()}`;
    return `New system ${d.name} at ${siteLabelOf(d.site)} · ${where}` + (state.me.viewer.is_admin ? " (placed in the estate when you submit)" : " (an admin places it in Tidy up)");
  }
  return "";
}
const STEP_NAMES = { out: ["Topic", "Destination", "Delivery", "Review"], in: ["What it sends", "Sending system", "Receive port", "Review"] };
async function showRequest(main) {
  mount(main, "#tpl-request");
  try { await Promise.all([loadCatalog(), loadNaming()]); } catch { /* keep the last copy */ }
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
      return declareProblem(r);
    }
    if (r.step === 3) return r.port ? "" : "Choose a free receive port.";
    if (r.step === 4) return r.purpose.trim().length >= 3 ? "" : "Say what the data is and who uses it, so an approver can decide.";
    return "";
  }
  if (r.step === 1) return r.topic ? "" : "Choose a topic.";
  if (r.step === 2) {
    if (!r.dest_name.trim()) return "Choose the receiving system, or enter its name.";
    if (!/^\d{1,3}(\.\d{1,3}){3}$/.test(r.dest_ip.trim())) return "Enter the system's mesh IP address.";
    return declareProblem(r);
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
  let problem = stepValid(r);
  if (problem) { r.error = problem; return renderStep(); }
  // Step 2: find out whether the destination is a declared system; if not, show the
  // declare fields before going on.
  if (r.step === 2 && r.resolvedFor !== destKey(r)) {
    await resolveDest(r);
    if (needsDeclare(r)) { r.error = ""; return renderStep(); }
    problem = stepValid(r);
    if (problem) { r.error = problem; return renderStep(); }
  }
  r.error = "";
  if (r.step < 4) { r.step++; return renderStep(); }
  $("#step-next").disabled = true;
  try {
    const common = { dest_name: r.dest_name.trim(), dest_ip: r.dest_ip.trim(), dest_peer_id: r.peer ? r.peer.id : "",
      port: Number(r.port), days: Number(r.days), purpose: r.purpose.trim(), declare: declareBody(r) };
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
      t.kind === "published" ? el("span", { class: "d simline" + (t.sim_status ? " " + t.sim_status.mode : " none"), text: "Real or simulated: " + simText(t.sim_status) }) : null,
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
          onclick: () => {
            r.peer = p; r.dest_name = p.name; r.dest_ip = p.ip;
            r.resolved = p.system ? { enforced: true, found: true, system: p.system } : null;
            r.resolvedFor = p.system ? destKey(r) : "";
            if (p.system) renderStep(); else resolveDest(r).then(renderStep);
          } },
          el("span", { class: "t" }, el("span", { class: "dot" + (p.connected ? " on" : ""), title: p.connected ? "online" : "offline" }), p.name),
          el("span", { class: "d" }, `${p.ip} · ${p.hostname || "—"} · ${p.connected ? "online" : "offline, last seen " + ago(p.last_seen)}`),
          p.groups && p.groups.length ? el("span", { class: "d", text: p.groups.join(", ") }) : null,
          p.system ? el("span", { class: "d", text: "In the estate: " + systemLine(p.system) + (p.system.archive_note ? ". " + p.system.archive_note : "") })
            : state.naming && state.naming.enforced ? el("span", { class: "d", text: "Not in the estate: you declare it after choosing it." }) : null)) : [el("p", { class: "empty", text: "No system matches." })]));
      } catch (e) { results.replaceChildren(el("div", { class: "err", text: e.message })); }
    };
    let t;
    search.addEventListener("input", () => { clearTimeout(t); t = setTimeout(run, 250); });
    out.push(el("div", { class: "field" }, el("label", { text: r.dir === "in" ? "Sending system" : "Receiving system" }), search,
      el("span", { class: "hint", text: "Systems on this goatnet's mesh. The address fills in from the one you choose." })));
    if (r.peer) out.push(el("div", { class: "next-box", id: "dest-selected" }, "Selected: ", el("b", { text: r.peer.name }), ` (${r.peer.ip})`,
      r.resolved && r.resolved.found ? el("div", { text: systemLine(r.resolved.system) + (r.resolved.system.archive_note ? ". " + r.resolved.system.archive_note + " Bring it back in Tidy up before relying on it." : "") }) : null));
    if (needsDeclare(r)) out.push(declarePanel(r));
    if (proposed(r)) out.push(el("div", { class: "next-box", text: `Already proposed by ${r.resolved.proposal.by} as ${r.resolved.proposal.key}; it waits for an admin in Tidy up.` }));
    out.push(results);
    out.push(el("p", {}, el("a", { href: "#request", onclick: (e) => { e.preventDefault(); r.manual = true; renderStep(); }, text: "Enter a name and address instead" })));
    run();
  } else {
    const name = el("input", { type: "text", value: r.dest_name, id: "dest-name", oninput: (e) => { r.dest_name = e.target.value; r.resolvedFor = ""; } });
    const ip = el("input", { type: "text", value: r.dest_ip, id: "dest-ip", placeholder: "100.64.x.y", oninput: (e) => { r.dest_ip = e.target.value; r.peer = null; r.resolvedFor = ""; } });
    out.push(el("div", { class: "field" }, el("label", { for: "dest-name", text: r.dir === "in" ? "Sending system's name" : "Receiving system's name" }), name,
      el("span", { class: "hint", text: "As people know it, for example the NetBird peer name." })));
    out.push(el("div", { class: "field" }, el("label", { for: "dest-ip", text: "Its mesh (NetBird) IP address" }), ip));
    if (r.resolved && r.resolved.found && r.resolvedFor === destKey(r)) out.push(el("div", { class: "next-box", text: "In the estate: " + systemLine(r.resolved.system) }));
    if (needsDeclare(r) && r.resolvedFor === destKey(r)) out.push(declarePanel(r));
    if (proposed(r)) out.push(el("div", { class: "next-box", text: `Already proposed by ${r.resolved.proposal.by} as ${r.resolved.proposal.key}; it waits for an admin in Tidy up.` }));
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
    destReview(r) ? el("dt", { text: "System" }) : null, destReview(r) ? el("dd", { id: "review-system", text: destReview(r) }) : null,
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
    destReview(r) ? el("dt", { text: "System" }) : null, destReview(r) ? el("dd", { id: "review-system", text: destReview(r) }) : null,
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
    if (action === "sim-status") return askSim(f);
    $("#sim-box").hidden = true;
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
  // Real or simulated: choose one of three, an optional note; it applies at once.
  const askSim = (f) => {
    $("#note-box").hidden = true;
    const box = $("#sim-box");
    const cur = f.sim_status && f.sim_status.mode;
    for (const r of box.querySelectorAll("input[name=sim-mode]")) r.checked = r.value === cur;
    $("#sim-note").value = (f.sim_status && f.sim_status.note) || "";
    box.hidden = false;
    (box.querySelector("input[name=sim-mode]:checked") || box.querySelector("input[name=sim-mode]")).focus();
  };
  $("#sim-box").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const picked = $("#sim-box").querySelector("input[name=sim-mode]:checked");
    if (!picked) { toast("Choose real, simulated or unknown."); return; }
    try {
      await api(`/api/feeds/${id}/sim-status`, { body: { mode: picked.value, note: $("#sim-note").value } });
      toast(`Saved: declared ${simWord[picked.value]}.`);
      $("#sim-box").hidden = true;
      await load();
    } catch (e) { toast(e.message); }
  });
  $("#sim-cancel").addEventListener("click", () => ($("#sim-box").hidden = true));
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
  const fc = f.facets || {};
  $("#feed-head").replaceChildren(...[
    el("h1", { text: title(f) }),
    fc.boundary ? el("p", { class: "why boundary", text: fc.boundary }) : null,
    fc.archived ? el("p", { class: "why", text: fc.archive_note }) : null,
    el("div", { class: "sigs" }, ...signals(f)),
    el("div", { class: "meta" }, cls(f.data_class), el("span", { class: "mono", text: f.id })),
    isIn(f) ? el("p", { class: "simhead simline" + (f.sim_status ? " " + f.sim_status.mode : " none"), id: "sim-head", text: "Real or simulated: " + simText(f.sim_status) }) : null,
    !isIn(f) && f.source_sim_status ? el("p", { class: "simhead simline " + f.source_sim_status.mode, id: "sim-head", text: "Its source, real or simulated: " + simText(f.source_sim_status) }) : null,
  ].filter(Boolean));
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
  const sysText = fc.dest_known ? systemLine({ label: fc.dest_system, site_label: fc.dest_site, boundary: fc.boundary, class: fc.dest_class,
    role: fc.dest_role, placed_by: fc.dest_placed_by }) : "Not in the estate. An admin places it in Tidy up.";
  $("#feed-details").replaceChildren(
    ...where,
    ...kv(isIn(f) ? "Sending system" : "Receiving system", sysText),
    ...kv("Owner", f.owner),
    ...kv("Purpose", f.purpose),
    ...kv("State", f.status),
    ...(isIn(f) ? kv("Real or simulated", simText(f.sim_status)) : []),
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

// ---------- tidy up: hand the clean-up of systems back to the tactical admin ----------
const TIDY_KIND = { undeclared: "Not in the estate", proposed: "Proposed", "name-rule": "Name", "boundary-far-side": "Boundary far side",
  "archived-link-on": "Archived, links on" };
const RETURNS = { "same-system-same-site": "The same system, at the same site", "same-system-new-site": "The same system, at a new site",
  "new-system": "A new system replaces it", unknown: "Not known" };
const REJOIN = { "peer-kept": "Its mesh peer is kept (it comes back as is)", "rejoin-needed": "It must rejoin the mesh", unknown: "Not known" };
let formSeq = 0; // unique ids for the fields of each open form
function tfield(id, label, input, hint, wide) {
  return el("div", { class: "field" + (wide ? " wide" : "") }, el("label", { for: id, text: label }), input, hint ? el("span", { class: "hint", text: hint }) : null);
}
function tselect(id, opts, cur, blank) {
  const sl = el("select", { id }, blank !== undefined ? el("option", { value: "", text: blank }) : null,
    ...opts.map(([v, t]) => el("option", { value: v, text: t })));
  sl.value = cur || "";
  return sl;
}
function linkItem(l) {
  if (l.kind === "declared") return el("li", { text: `${l.title} — declared link, controlled by ${l.controlled_by || "its deployment file"} (manifest ${l.manifest || "—"})${l.on ? "" : ", switched off"}` });
  return el("li", {}, el("a", { href: `#feed/${l.id}`, text: l.title }), ` (${l.status})`);
}
function tidyResult(...lines) {
  $("#tidy-result").replaceChildren(el("div", { class: "next-box" }, ...lines.filter((x) => x).map((x) => el("div", { text: x }))));
}
// Place: put a system in its site and function (and for a boundary node, its far side).
function placeForm(d, it, done) {
  const sys = (it && it.system) || null;
  const prop = (it && it.proposal) || null;
  const isNew = !sys;
  const uid = String(++formSeq);
  const v = {
    name: isNew ? (prop ? prop.key : "") : (it.suggested_name || ""),
    site: (sys && sys.site) || (prop && prop.site) || "", class: (sys && sys.class) || (prop && prop.class) || "",
    role: (sys && sys.role) || (prop && prop.role) || "", label: (sys && sys.label) || (prop && prop.label) || (it && it.peer) || "",
    far: (sys && sys.far_side && sys.far_side.label) || (prop && prop.far_side && prop.far_side.label) || "",
    dir: (sys && sys.far_side && sys.far_side.direction) || "out",
  };
  const site = tselect(`pl-site-${uid}`, d.sites.map((x) => [x.id, `${x.label} (${x.id})`]), v.site, "Choose the site");
  const kind = tselect(`pl-class-${uid}`, d.classes.map((x) => [x, x]), v.class, "Choose the kind");
  const roles = el("datalist", { id: `pl-roles-${uid}` }, ...d.roles.map((x) => el("option", { value: x })));
  const role = el("input", { type: "text", id: `pl-role-${uid}`, list: `pl-roles-${uid}`, value: v.role, placeholder: "ops" });
  const label = el("input", { type: "text", id: `pl-label-${uid}`, value: v.label });
  const name = el("input", { type: "text", id: `pl-name-${uid}`, class: "pl-name", value: v.name, placeholder: isNew ? "console-north-ops-01" : sys.name, autocomplete: "off" });
  const far = el("input", { type: "text", id: `pl-far-${uid}`, class: "pl-far", value: v.far, placeholder: "Partner network" });
  const dir = tselect(`pl-dir-${uid}`, [["out", "out"], ["in", "in"], ["both", "both"]], v.dir);
  const farBox = el("div", { class: "wide row" });
  const showFar = () => farBox.replaceChildren(...(kind.value === "boundary" ? [
    tfield(`pl-far-${uid}`, "Far side: where data through it goes", far, "Name the network beyond it."), tfield(`pl-dir-${uid}`, "Direction", dir)] : []));
  const checkBox = el("div", { class: "wide" });
  let ct;
  const check = () => {
    clearTimeout(ct);
    ct = setTimeout(async () => {
      if (!name.value.trim()) return checkBox.replaceChildren();
      try {
        const x = await api(`/api/naming?name=${enc(name.value.trim())}&site=${enc(site.value)}`);
        checkBox.replaceChildren(x.check.ok ? el("span", { class: "ok-note", text: "Follows the naming rule." })
          : el("ul", { class: "problems" }, ...x.check.problems.map((p) => el("li", { text: p }))));
      } catch { checkBox.replaceChildren(); }
    }, 200);
  };
  const suggest = async () => {
    if (!site.value || !kind.value) return;
    try { const x = await api(`/api/naming?kind=${enc(kind.value)}&site=${enc(site.value)}&function=${enc(role.value || "ops")}`); name.value = x.suggestion; check(); } catch { /* by hand */ }
  };
  kind.addEventListener("change", showFar);
  name.addEventListener("input", check);
  site.addEventListener("change", check);
  const submit = el("button", { type: "button", class: "btn primary small pl-submit", text: "Place" });
  submit.addEventListener("click", async () => {
    const key = isNew ? name.value.trim() : sys.name;
    const body = { site: site.value, class: kind.value, role: role.value.trim(), label: label.value.trim(),
      peer: isNew ? ((prop && prop.peer) || it.peer || "") : undefined,
      far_side: kind.value === "boundary" ? { label: far.value.trim(), direction: dir.value || "out" } : undefined };
    if (!isNew && name.value.trim() && name.value.trim() !== sys.name) body.name = name.value.trim();
    if (!key) { toast("Give the system a machine name."); return; }
    try {
      const r = await api(`/api/systems/${enc(key)}/place`, { body });
      tidyResult(`Placed ${r.system.name} at ${r.system.site_label || r.system.site}.`,
        r.system.boundary ? r.system.boundary + "." : "",
        body.name ? "Rename the machine at the mesh (NetBird) stage to match." : "");
      done();
    } catch (e) { toast(e.message); }
  });
  const form = el("div", { class: "tidy-form", "data-form": "place" },
    tfield(`pl-site-${uid}`, "Site", site), tfield(`pl-class-${uid}`, "Kind of system", kind),
    tfield(`pl-role-${uid}`, "Function", role, "Its job: ops, cue, relay…"), roles,
    tfield(`pl-label-${uid}`, "Display label", label),
    tfield(`pl-name-${uid}`, isNew ? "Machine name" : "New machine name (leave as is to keep it)", name, "Rule: " + d.rule, true),
    el("div", { class: "wide row" }, el("button", { type: "button", class: "btn small", text: "Suggest a name", onclick: suggest }), submit),
    checkBox, farBox);
  showFar();
  if (name.value) check();
  return form;
}
// Archive: say how the system is expected to come back, and which links to switch back on.
function archiveForm(sys, links, done) {
  const uid = String(++formSeq);
  const op = el("input", { type: "text", id: `ar-op-${uid}`, class: "ar-op", placeholder: "Exercise or operation name" });
  const ret = tselect(`ar-ret-${uid}`, Object.entries(RETURNS), "unknown");
  ret.classList.add("ar-returns");
  const rej = tselect(`ar-rej-${uid}`, Object.entries(REJOIN), "unknown");
  rej.classList.add("ar-rejoin");
  const notes = el("textarea", { id: `ar-notes-${uid}`, rows: 2, placeholder: "Where it is stored, who holds it, what to check when it comes back" });
  const boxes = links.map((l) => ({ l, cb: el("input", { type: "checkbox", value: l.id, ...(l.on ? { checked: "" } : {}) }) }));
  const pause = el("input", { type: "checkbox", id: `ar-pause-${uid}`, class: "ar-pause", checked: "" });
  const submit = el("button", { type: "button", class: "btn primary small", text: "Archive" });
  submit.addEventListener("click", async () => {
    try {
      const r = await api(`/api/systems/${enc(sys.name)}/archive`, { body: { returns: ret.value, rejoin: rej.value, operation: op.value.trim(), notes: notes.value.trim(),
        restore: boxes.filter((b) => b.cb.checked).map((b) => b.l.id), pause: pause.checked } });
      tidyResult(`${sys.name}: ${r.system.archive_note}`,
        r.paused.length ? `Paused ${r.paused.length} requested feed${r.paused.length === 1 ? "" : "s"}: ${r.paused.join(", ")}.` : "",
        ...r.declared_still_on.map((l) => `Still on: ${l.title}. Switch it off in ${l.controlled_by || "its deployment file"} (manifest ${l.manifest}); it is read-only here.`));
      done();
    } catch (e) { toast(e.message); }
  });
  return el("div", { class: "tidy-form", "data-form": "archive" },
    tfield(`ar-op-${uid}`, "Operation", op, "Free text: the exercise or operation it served."),
    tfield(`ar-ret-${uid}`, "How does it come back?", ret), tfield(`ar-rej-${uid}`, "Does it need to rejoin the mesh?", rej),
    tfield(`ar-notes-${uid}`, "Notes", notes, "", true),
    el("div", { class: "wide checks" }, el("b", { text: links.length ? "Links to switch back on when it returns" : "No links to or from it." }),
      ...boxes.map((b) => el("label", {}, b.cb, " ", b.l.title, b.l.kind === "declared" ? " (declared link)" : ` (${b.l.status})`))),
    el("label", { class: "wide" }, pause, " Pause its requested feeds now (declared links are switched off in their own file)"),
    el("div", { class: "wide row" }, submit));
}
function tidyCard(d, it, reload) {
  const formBox = el("div");
  const open = (form) => formBox.replaceChildren(form);
  const acts = el("div", { class: "acts" });
  for (const a of it.actions) {
    if (a === "place") acts.append(el("button", { type: "button", class: "btn small", "data-act": "place", text: it.kind === "proposed" ? "Place as proposed" : "Place",
      onclick: () => open(placeForm(d, it, reload)) }));
    if (a === "archive") acts.append(el("button", { type: "button", class: "btn small", "data-act": "archive", text: "Archive",
      onclick: () => open(archiveForm(it.system, it.links, reload)) }));
    if (a === "restore") acts.append(el("button", { type: "button", class: "btn small", "data-act": "restore", text: "Bring back",
      onclick: () => bringBack(it.system, true, reload) }));
    if (a === "dismiss") acts.append(el("button", { type: "button", class: "btn small", "data-act": "dismiss", text: "Dismiss",
      onclick: async () => { try { await api(`/api/systems/${enc(it.proposal.key)}/dismiss`, { body: {} }); tidyResult(`Dismissed the proposal for ${it.proposal.key}.`); reload(); } catch (e) { toast(e.message); } } }));
  }
  return el("div", { class: "card tidy", "data-kind": it.kind },
    el("div", { class: "main" },
      el("div", { class: "line1" }, el("span", { class: "kind", text: TIDY_KIND[it.kind] || it.kind }), it.title),
      el("div", { class: "line2", text: it.reason }),
      it.suggested_name ? el("div", { class: "line2", text: `Suggested name: ${it.suggested_name}` }) : null,
      it.system && it.system.boundary && it.kind !== "boundary-far-side" ? el("div", { class: "line2", text: it.system.boundary }) : null,
      it.links.length ? el("ul", { class: "links" }, ...it.links.map(linkItem)) : null),
    acts, formBox);
}
async function bringBack(sys, resume, reload) {
  try {
    const r = await api(`/api/systems/${enc(sys.name)}/restore`, { body: { resume } });
    tidyResult(`Brought back ${sys.name}.`,
      r.resumed.length ? `Resumed: ${r.resumed.join(", ")}.` : "No requested feed was resumed.",
      ...r.declared.map((l) => `Switch on ${l.title} in ${l.controlled_by || "its deployment file"} (manifest ${l.manifest}); declared links are read-only here.`),
      ...r.skipped.map((x) => `Not resumed: ${x.id} (${x.why}).`));
    reload();
  } catch (e) { toast(e.message); }
}
function archivedCard(a, reload) {
  const resume = el("input", { type: "checkbox", checked: "" });
  const restoreTitles = ((a.archive && a.archive.restore) || []).map((id) => (a.links.find((l) => l.id === id) || { title: id }).title);
  return el("div", { class: "card tidy archived-system", "data-system": a.name },
    el("div", { class: "main" },
      el("div", { class: "line1" }, el("span", { class: "kind", text: "Archived" }), `${a.label} (${a.name}) · ${a.site_label || a.site}`),
      el("div", { class: "line2", text: a.archive_note }),
      a.archive && a.archive.notes ? el("div", { class: "line2", text: "Notes: " + a.archive.notes }) : null,
      el("div", { class: "line2", text: restoreTitles.length ? "Switched back on when it returns: " + restoreTitles.join("; ") : "No links to switch back on." })),
    el("div", { class: "acts" }, el("label", {}, resume, " Resume its paused feeds"),
      el("button", { type: "button", class: "btn small", "data-act": "restore", text: "Bring back", onclick: () => bringBack(a, resume.checked, reload) })));
}
async function showTidy(main) {
  mount(main, "#tpl-tidy");
  if (!state.me.viewer.is_admin) { main.replaceChildren(el("p", { class: "empty", text: "Tidy up is for tactical and platform admins." })); return; }
  const reload = async () => {
    let d;
    try { d = await api("/api/tidy"); } catch (e) { $("#tidy-items").replaceChildren(el("div", { class: "err", text: e.message })); return; }
    state.tidyCount = d.count;
    const c = $("#tidy-count");
    c.textContent = d.count ? String(d.count) : "";
    c.hidden = !d.count;
    $("#tidy-rule").textContent = "Naming rule: " + d.rule;
    $("#tidy-items-head").textContent = `To organise (${d.items.length})`;
    $("#tidy-items").replaceChildren(...(d.items.length ? d.items.map((it) => tidyCard(d, it, reload))
      : [el("p", { class: "empty", text: "Nothing to organise: every system is in its site and function." })]));
    const pick = $("#tidy-archive-pick");
    pick.replaceChildren(el("option", { value: "", text: "Choose a system" }),
      ...d.systems.map((x) => el("option", { value: x.name, text: `${x.label} (${x.name}) · ${x.site_label || x.site}` })));
    pick.onchange = () => {
      const sys = d.systems.find((x) => x.name === pick.value);
      $("#tidy-archive-box").replaceChildren(sys ? archiveForm(sys, sys.links, reload) : "");
    };
    $("#tidy-archive-box").replaceChildren();
    $("#tidy-archived-head").textContent = `Archived (${d.archived.length})`;
    $("#tidy-archived").replaceChildren(...(d.archived.length ? d.archived.map((a) => archivedCard(a, reload))
      : [el("p", { class: "empty", text: "No system is archived." })]));
  };
  await reload();
}

// ---------- audit ----------
async function showAudit(main) {
  mount(main, "#tpl-audit");
  const d = await api("/api/audit");
  $("#audit-body").replaceChildren(...(d.events || []).map((e) => el("tr", {},
    el("td", { text: fmtTime(e.at) }), el("td", { text: e.actor }), el("td", { text: e.action }),
    el("td", {}, el("a", { href: e.feed_id.startsWith("system:") ? "#tidy" : `#feed/${e.feed_id}`, text: e.feed_id })), el("td", { text: e.detail }))));
}

// ---------- start ----------
window.addEventListener("hashchange", route);
(async () => {
  try { await loadMe(); route(); }
  catch (e) { if (e.message !== "sign in") $("#main").replaceChildren(el("div", { class: "err", text: "Could not load: " + e.message })); }
})();
