// SPDX-License-Identifier: Apache-2.0
// What changed, newest first.
import { $, esc } from "../lib.js";

export function renderFeed(d) {
  const since = new Date(d.watching_since_ms);
  $("#since").textContent = "since this surface started watching, " + since.toLocaleString();

  if (!d.changes.length) {
    // An empty feed must read as NOT SEEN, never as DID NOT HAPPEN. The feed
    // lives in the server's memory, so a restart empties it.
    $("#feed").innerHTML =
      `<div class="quiet">Nothing has changed since ${esc(since.toLocaleTimeString())}.
       This feed is held in memory, so a restart empties it — an empty feed means
       <b>not seen</b>, not <b>did not happen</b>.</div>`;
    return;
  }
  $("#feed").innerHTML = d.changes.map(c => `
    <div class="ev">
      <time>${new Date(c.ts_unix_ms).toLocaleTimeString()}</time>
      <span class="who">${esc(c.node)}</span>
      <span class="what">${esc(c.what)}</span>
    </div>`).join("");
}
