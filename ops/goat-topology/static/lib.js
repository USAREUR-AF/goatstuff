// SPDX-License-Identifier: Apache-2.0
// Shared helpers. Nothing here knows about any particular section.

export const $ = s => document.querySelector(s);

/** Escape for interpolation into markup. Every value from the API goes through
 *  this: router names and error strings originate off-host, and a surface that
 *  renders them raw is an injection away from being someone else's page. */
export const esc = s => String(s ?? "").replace(/[&<>"]/g,
  c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));

/** A region's contribution to the route table: V(V−1), but only when it has
 *  links. An edgeless region carries no routes however many nodes it lists —
 *  the correction the live zagros router forced (design doc §4.1). Kept here
 *  rather than in a renderer because the planner and the table share it. */
export const terms = (v, e) => (e === 0 ? 0 : v * (v - 1));

/** The server sends absolute receipt times, not ages, so its payload does not
 *  churn while nothing happens. Ages are a render-time concern. */
/** A time as the routers' and publishers' logs write it: UTC, with the zone
 *  named, so two operators in different time zones quote the same time and it
 *  matches the logs. Every absolute time on the page goes through these. */
export const utcTime = ms => new Date(ms).toISOString().slice(11, 19) + " UTC";
export const utcDateTime = ms => new Date(ms).toISOString().slice(0, 19).replace("T", " ") + " UTC";

export const ageSecs = (receivedMs, nowMs = Date.now()) =>
  Math.max(0, Math.round((nowMs - receivedMs) / 1000));

export const severityChip = s =>
  s === "critical" ? "err" : s === "warning" ? "warn" : s === "advisory" ? "info" : "muted";

/** A router's part in the fabric, read from what it reports: a router that
 *  holds other routers in a south group is a gateway. Display only. */
export const roleOf = x =>
  (x.regions || []).some(r => /^south:.*:router$/.test(r.name) && r.link_state_e > 0) ||
  x.gateway_mode === "custom" ? "gateway" : "member";

// A router that never reports is known only by its ZID, and twenty hex strings
// cannot be told apart at a glance. Give each a pronounceable display name,
// derived from the ZID and from nothing else, so it is the same on every page
// load and for every viewer. DISPLAY ONLY: it is not an identity, nothing
// matches on it, and the ZID is always shown beside it.
const ADJ = ["amber", "brisk", "calm", "clear", "cobalt", "coral", "crisp", "dusty", "early", "even",
  "fern", "flint", "gold", "green", "high", "ivory", "jade", "keen", "late", "light", "lunar", "mild",
  "north", "olive", "pale", "plain", "quiet", "rapid", "slate", "solar", "still", "tidal"];
const NOUN = ["anchor", "basin", "beacon", "birch", "bridge", "canyon", "cedar", "cove", "delta", "dune",
  "field", "fjord", "grove", "harbor", "heron", "island", "juniper", "lagoon", "ledge", "maple", "meadow",
  "mesa", "orchard", "pier", "plateau", "quarry", "reef", "ridge", "summit", "valley", "willow", "yard"];
const isZid = s => /^[0-9a-f]{8,32}$/.test(String(s));

/** The label for a router: its node name when it has one; for a bare ZID, the
 *  derived display name with the ZID's first six characters. */
export function displayName(id) {
  const s = String(id ?? "");
  if (!isZid(s)) return s;
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) { h ^= s.charCodeAt(i); h = Math.imul(h, 16777619) >>> 0; }
  return `${ADJ[h % ADJ.length]}-${NOUN[(h >>> 8) % NOUN.length]} (${s.slice(0, 6)})`;
}

/** What a router is called on the page. A reporting router: its node name. A
 *  router that sends no report: the name of the mesh peer it connects from,
 *  when the peer directory has it; otherwise the derived display name. The ZID
 *  is always shown beside it in the detail. */
export function labelOf(x) {
  if (!x) return "";
  if (x.node) return x.node;
  if (x.peer && x.peer.name) return x.peer.name;
  return displayName(x.zid);
}

/** The label for a graph key (node name or ZID), looked up in the payload. */
export function labelFor(d, id) {
  const x = (d.routers || []).find(r => (r.node || r.zid) === id);
  return x ? labelOf(x) : displayName(id);
}

export const NOT_RECORDED = "not recorded";

/** The organisation line on the graph and in the table: the organisation, and
 *  the ORGANISATION's nation. A person's nationality is a different fact and
 *  is shown only in the detail, beside the individual. */
export function orgLine(x) {
  const o = x && x.owner;
  if (o) return `${o.org_display_name || o.org || "org " + NOT_RECORDED} · ${o.org_nation || "nation " + NOT_RECORDED}`;
  if (x && x.peer) return "org " + NOT_RECORDED;
  return "";
}

/** What a router carries, in one table cell's words. A reporting router: its
 *  own census. One that sends no report: what a reporter in its routing group
 *  attributes to it. Absent is "—", a failed read is "not read"; neither is 0. */
export function carriesLine(x) {
  const c = x.census;
  if (c) {
    if (c.read_error) return { text: "not read", sub: c.read_error, err: true };
    return {
      text: `${c.session_subscriptions} subs · ${c.session_publishers} pubs · ${c.topics} topics${c.capped ? "+" : ""}`,
      sub: `its applications; ${c.own_process_subscribed_keys} more held by the router itself`,
    };
  }
  if (x.held) return { text: `${x.held.subscribed_keys} subscribed keys`, sub: `as ${x.held.by} sees it; publishers not visible` };
  return { text: "—", sub: x.reporting === false ? "no reporter in its routing group has a census" : "no census from this publisher" };
}
