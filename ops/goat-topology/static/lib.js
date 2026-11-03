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
