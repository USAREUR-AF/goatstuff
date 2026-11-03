"use strict";
/* peerview — formatters and small renderers shared by peers.html and
   demand.html.
 *
 * WHY A SHARED FILE RATHER THAN THE INLINE COPIES netview.html USES. The
 * netview pair carries its own copy of these helpers per page, and that
 * duplication has already cost once: a stream-service list existed in three
 * places and one copy was still pre-relabel, so a page silently rendered
 * against names that no longer matched. Two pages sharing one definition of
 * "how do we render a number we do not have" is exactly the kind of thing that
 * must not be able to drift.
 *
 * THE MOST IMPORTANT RULE IN THIS FILE: null means NOT MEASURED and must never
 * render as a number. `0` and `null` are different facts and the difference is
 * usually the whole point — "no relayed peers" and "we cannot see the path
 * class" lead to opposite actions.
 */

const $ = s => document.querySelector(s);
const CO = ["--s1","--s2","--s3","--s4","--s5","--s6","--s7"];
const cssv = n => getComputedStyle(document.documentElement).getPropertyValue(n).trim();
const UNK = '<span class="v sm unknown">not measured</span>';

const esc = s => String(s).replace(/[&<>"]/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;"}[c]));

function bps(v){
  if(v==null) return null;
  const a=Math.abs(v);
  if(a>=1e9) return (v/1e9).toFixed(2)+" Gbit/s";
  if(a>=1e6) return (v/1e6).toFixed(2)+" Mbit/s";
  if(a>=1e3) return (v/1e3).toFixed(0)+" kbit/s";
  return v.toFixed(0)+" bit/s";
}
function bytes(v){
  if(v==null) return null;
  const a=Math.abs(v);
  if(a>=1e9) return (v/1e9).toFixed(2)+" GB";
  if(a>=1e6) return (v/1e6).toFixed(1)+" MB";
  if(a>=1e3) return (v/1e3).toFixed(0)+" kB";
  return v.toFixed(0)+" B";
}
function rps(v){
  if(v==null) return null;
  const a=Math.abs(v);
  return (a>=100?v.toFixed(0):a>=10?v.toFixed(1):v.toFixed(2))+" req/s";
}
/* Percentages never round to a certainty they do not have. 99.98% printed as
   "100.0%" reads as "all of it, exactly", and on this page that is the
   difference between "browsing is a rounding error" and "browsing does not
   exist". Same at the bottom: 0.02% is not zero. Only an exact 1 or 0 gets
   the clean number. */
function pct(v){
  if(v==null) return null;
  if(v===1) return "100%";
  if(v===0) return "0%";
  const p = v*100;
  if(p >= 99.95) return ">99.9%";
  if(p > 0 && p < 0.05) return "<0.1%";
  return p.toFixed(1)+"%";
}
const ms   = v => v==null ? null : (v<10 ? v.toFixed(1) : v.toFixed(0))+" ms";
const num  = v => v==null ? null : Math.round(v).toLocaleString();
const secs = v => v==null ? null : v<90 ? v.toFixed(0)+" s" : (v/60).toFixed(1)+" min";

const val = (v,c) => v==null ? UNK : `<div class="v ${c||""}">${v}</div>`;

/* A peer's display name. Strips the mesh DNS suffix so a table column holds a
   name instead of a domain. Never decomposed further — the mint handle inside
   a peer name is an opaque key, not a set of fields to read meaning out of. */
const shortPeer = s => String(s||"").replace(/\.netbird\.selfhosted$/,"");

/* ── the owning identity ──────────────────────────────────────────────────
   Every peer row on both pages carries `owner` from the poller, and every
   owner renders through here so the two pages cannot disagree about what a
   resolved owner looks like or about what an unresolved one is allowed to
   claim.

   TWO RULES, both learned from the grammar SoT (ops/identity/peer-name-
   grammar.yaml) rather than invented here:

   1. An owner that did not resolve is NEVER rendered as a name. The handle
      read out of a peer name is evidence, not a record; showing it in the
      owner column would present a guess in the column people click to decide
      who to go and ask. Unresolved rows render a dash and carry the reason.
   2. The link is only drawn when this deployment has an identity surface
      configured. Otherwise the owner is text — a dead link on a status page
      teaches people the links do not work. */
/* "1 peer carries" / "3 peers carry" — the counts in these sentences are
   usually small, and "1 peer(s) carry" reads as a template nobody finished. */
const plural = (n, one, many) => `${num(n)} ${n===1 ? one : many}`;

/* Two link targets, and which one a row earns depends on what resolved.

   A person the resolver reconciled against a LIVE Ory identity gets the
   id-admin-console person page — the same deep-link the relay-health card
   uses, so two surfaces land an operator in the same place. Everything else
   gets the identity roster keyed by handle, which is all a handle can honestly
   address. Neither link is drawn when its base is unconfigured. */
/* The declared registry's own age, graded rather than dated.

   This used to print `generated_at` verbatim. A date is not a judgement: a roster
   behind the repository reads "registry file generated <date>" — true,
   unalarming, and live people read as "not in roster".

   What this host CANNOT answer is whether the file is merely old or actually
   behind, because there is no checkout here to compare against. `goat access
   strays` does exactly that comparison against the served `source_digest`, so
   an ageing or stale grade names it instead of guessing. */
function rosterAge(o){
  const members = `${num(o.roster_members)} declared identities`;
  if(!o.roster_ok) return `registry not loaded — ${esc(o.roster_reason || "reason not recorded")}`;
  const when = o.roster_generated_at
    ? `generated ${esc(o.roster_generated_at)}`
    : "generated at an unrecorded time";
  const digest = o.roster_source_digest
    ? ` · source ${esc(String(o.roster_source_digest).slice(0, 8))}`
    : "";
  const days = o.roster_age_days == null ? null : Math.round(o.roster_age_days);
  const grade = o.roster_freshness || "unknown";
  if(grade === "unknown"){
    /* Unmeasured, not old: an artifact predating the provenance stamp. */
    return `registry ${when} with ${members}${digest} — age unknown, the file predates the provenance stamp`;
  }
  const age = days === 0 ? "today" : plural(days, "day old", "days old");
  if(grade === "fresh") return `registry ${when} (${age}) with ${members}${digest}`;
  return `registry ${when} — <b>${age}</b>, with ${members}${digest}. Run
    <code>goat access strays</code> against a current checkout to see whether it is
    behind; if it is, regenerate and redeploy rather than editing the file.`;
}

function identityHref(owner, links){
  if(!links || !owner || !owner.handle) return null;
  /* A team is a mesh GROUP, not an identity — the identity roster has no row
     to land on, and the console's identity list is where group membership is
     actually readable. Linking a group at a person page would be a link that
     looks right and answers nothing. */
  if(owner.kind === "team"){
    return links.id_console_base ? links.id_console_base.replace(/\/$/,"") + "/" : null;
  }
  if(owner.ory_id && links.id_console_base){
    const path = owner.id_console_path || ("/users/" + owner.ory_id);
    return links.id_console_base.replace(/\/$/,"") + path;
  }
  const base = links.identity_base;
  if(!base) return null;
  const p = (links.identity_param || "identity");
  return `${base}${base.includes("?") ? "&" : "?"}${p}=${encodeURIComponent(owner.handle)}`;
}

/* The states of the shared resolver plus the ones this surface adds. The
   labels for the unresolved ones are deliberately different sentences: "we
   looked and this peer carries no principal" and "we could not look" are
   different facts, and a reader who sees one blank for both concludes
   "unowned" for peers nobody has checked. */
const OWNER_UNRESOLVED = {
  unmatched: "not onboarded",
  ambiguous: "ambiguous",
  unread:    "not checked",
};

/* An unmatched peer is a different job depending on what its name says. A name
   carrying a handle the roster lacks is usually a roster that has not caught up
   with an onboarding; only a name with no principal at all is "not onboarded".
   Rows from a poller older than `shape` fall back to the old label. */
const UNMATCHED_BY_SHAPE = {
  person:  "not in roster",
  machine: "machine, no custodian",
  none:    "not onboarded",
};

const OWNER_CHIP = {name: "lead", custody: "custodian", team: "team", declared: "declared"};

const OWNER_TIP = {
  registry: "recorded: the roster handle reconciled against a live identity",
  name:     "a lead, not an answer: the roster names this handle, but no live "
            + "identity matched it",
  custody:  "the custodian who enrolled this machine — a machine cannot answer "
            + "for itself",
  team:     "infrastructure: the mesh group answerable for this peer's scope. "
            + "No one person owns it",
  declared: "this peer carries no onboarding name; its owner is declared in "
            + "config rather than resolved",
};

function ownerCell(owner, links){
  const o = owner || {kind:"unread", why:"this poll carried no owner reading for the peer"};
  if(!o.handle){
    const label = (o.kind === "unmatched" && UNMATCHED_BY_SHAPE[o.shape])
      || OWNER_UNRESOLVED[o.kind] || o.kind || "—";
    return `<span class="sub" title="${esc(o.why||"")}">${esc(label)}</span>`;
  }
  const href = identityHref(o, links);
  const tip = esc((OWNER_TIP[o.kind] || "") + (o.why ? " — " + o.why : ""));
  const name = esc(o.handle);
  const body = href
    ? `<a href="${href}" title="${tip}" rel="noopener">${name} ↗</a>`
    : `<span title="${tip}">${name}</span>`;
  return OWNER_CHIP[o.kind] ? `${body} <span class="chip">${OWNER_CHIP[o.kind]}</span>` : body;
}

function card3(title, desc, value, cls, sub){
  return `<div class="card c3"><h3>${title}</h3><p class="d">${desc}</p>
    ${val(value, cls)}<div class="sub">${sub||""}</div></div>`;
}

const UNITS = {bps, rps, pct, ms, bytes, count: v => num(v)};

function bars(rows, unit){
  if(!rows || !rows.length) return '<p class="sub">Nothing in this group to show.</p>';
  const fmt = UNITS[unit] || (v=>String(v));
  const max = Math.max(...rows.map(r=>r.value)) || 1;
  return '<div class="bars">' + rows.map((r,i)=>{
    const c = r.hatch
      ? `repeating-linear-gradient(45deg,${cssv("--hatch")} 0 3px,${cssv("--surface-2")} 3px 6px)`
      : cssv(CO[i%CO.length]);
    return `<div class="bar"><span class="k mono" title="${esc(r.name)}">${esc(r.name)}</span>
      <span class="track"><span class="fill" style="width:${(r.value/max*100).toFixed(1)}%;background:${c}"></span></span>
      <span class="n">${fmt(r.value)}</span></div>`;
  }).join("") + '</div>';
}

/* One horizontal bar split into shares, with a legend. Used where the
   composition matters more than the magnitudes. A zero-total set renders the
   honest sentence rather than an empty bar that reads as "all fine". */
function splitBar(rows, unit){
  rows = (rows||[]).filter(r=>r.value>0);
  const total = rows.reduce((a,b)=>a+b.value,0);
  if(!total) return '<p class="sub">Nothing to divide — every share here is zero.</p>';
  const fmt = UNITS[unit] || UNITS.count;
  return `<div class="stack">${rows.map((r,i)=>
      `<span title="${esc(r.name)}: ${fmt(r.value)}" style="width:${(r.value/total*100).toFixed(2)}%;background:${cssv(CO[i%CO.length])}"></span>`).join("")}</div>
    <div class="legend">${rows.map((r,i)=>
      `<span class="it"><span class="sw" style="background:${cssv(CO[i%CO.length])}"></span>
       ${esc(r.name)} — ${fmt(r.value)} (${(r.value/total*100).toFixed(1)}%)</span>`).join("")}</div>`;
}

const age = d => Math.max(0, Math.floor(Date.now()/1000 - (d.generated_unix||0)));

/* A document older than a few poll intervals is stale, and stale numbers
   presented as current are worse than no numbers. The banner states the age
   rather than hiding the page, because an operator debugging a dead poller
   still wants to see what it last managed to write. */
function staleBanner(d){
  const a = age(d);
  if(a < 900) return "";
  return `<div class="note bad"><b>This document is ${secs(a)} old.</b> The poller has not
    written since ${esc(d.generated_iso||"?")}. Everything below is what was true then,
    not what is true now — check <code>peerview-poll.timer</code> on
    <code>${esc(d.vantage||"the host")}</code>.</div>`;
}

/* Which of the four sources answered, and what goes dark when one does not.
   Rendered on both pages, identically, because "the panel is empty" and "the
   source is down" look the same and are not. */
function sourcesSection(d){
  const s = d.sources || {};
  const rows = Object.keys(s).map(k=>{
    const v = s[k];
    return `<tr><td class="mono">${esc(k)}</td>
      <td><span class="chip ${v.ok?"on":"off"}">${v.ok?"answered":"no answer"}</span></td>
      <td class="mono name" title="${esc(v.detail||"")}">${esc(v.detail||"")}</td>
      <td style="white-space:normal">${v.ok?'<span class="sub" style="margin:0">—</span>':esc(v.dark||"")}</td></tr>`;
  }).join("");
  return `<section class="q"><h2><span class="n">✓</span>What is actually being measured?</h2>
    <p class="lede">Four sources, each able to fail on its own. The two goatnets this
    surface ships to do not carry the same telemetry and neither is a subset of the
    other, so a blank panel needs to be distinguishable from a zero.</p>
    <div class="grid"><div class="card c12">
    <table><thead><tr><th>Source</th><th>State</th><th>Detail</th><th>What goes dark without it</th></tr></thead>
    <tbody>${rows}</tbody></table>
    <div class="sub" style="margin-top:10px">${esc(d.retention_note||"")}</div>
    </div></div></section>`;
}

/* Load the document, or say plainly that it could not be loaded. Reloads on a
   fixed interval so a wall-mounted page tracks the poller. */
function load(url, render){
  fetch(url+"?_="+Date.now())
    .then(r=>{ if(!r.ok) throw new Error("HTTP "+r.status); return r.json(); })
    .then(render)
    .catch(e=>{ $("#body").innerHTML =
      `<div class="note bad"><b class="err">Could not load ${esc(url)}.</b> ${esc(e.message)}.
       No data is shown rather than stale data presented as current. The file is written by
       <code>peerview-poll.service</code>; if it has never run, it does not exist yet.</div>`; });
  setTimeout(()=>location.reload(), 120000);
}
