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
