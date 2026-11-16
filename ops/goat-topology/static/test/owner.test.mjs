// SPDX-License-Identifier: Apache-2.0
// The owner text a browser shows (review of goatstuff#104, items 1, 5, 6, 7, 8).
// Run: node --test ops/goat-topology/static/test/
import { test } from "node:test";
import assert from "node:assert/strict";
import { orgLine, ownerTone } from "../lib.js";
import { ownerFacts } from "../render/detail.js";
import { namesNote } from "../render/routers.js";

const recorded = (owner = {}, extra = {}) => ({
  zid: "bb11", node: "", address: "100.127.0.21", owner_state: "recorded", owner_reason: "", owner_cause: "",
  peer: { name: "machine-anvil-relay", hostname: "h", groups: [], owner: {
    source: "record", via: "machine-custody", matched_by: "peer-name", person_handle: "rivera-j",
    org_id: "org-anvil", org_short_tag: "anvil", org_nation: "XAA", org_state: "recorded",
    nationality: "XCC", nationality_state: "recorded", ...owner } },
  ...extra,
});

test("orgLine names every owner state in words", () => {
  assert.equal(orgLine(recorded()), "anvil · XAA");
  assert.equal(orgLine({ ...recorded(), owner_state: "lead", owner: { org: "org-local", org_nation: "XBB" } }),
    "NO RECORDED OWNER (lead only)", "a lead never shows the local roster's organization");
  assert.equal(orgLine({ ...recorded(), owner_state: "incomplete" }), "OWNER RECORD INCOMPLETE");
  assert.equal(orgLine({ ...recorded(), owner_state: "not_read" }), "owner not read");
  assert.equal(ownerTone({ owner_state: "lead" }), "var(--status-err-fg)");
  assert.equal(ownerTone({ owner_state: "incomplete" }), "var(--status-warn-fg)");
});

test("the individual is hidden when the surface hides it", () => {
  const html = ownerFacts(recorded({ person_handle: "", nationality: "" }), { directory: { individual_hidden: true } });
  assert.match(html, /hidden on this surface/);
  assert.doesNotMatch(html, /not recorded<\/span><\/dd>\s*<dt>Organization<\/dt>/, "a hidden person is not 'not recorded'");
  assert.match(html, /anvil/);
});

test("a lead keeps the individual's own nationality row", () => {
  const lead = { kind: "person", handle: "okafor-n", org: "org-local", org_nation: "XBB", individual: "okafor-n", individual_nationality: "XDD" };
  const x = { ...recorded(), owner_state: "not_read", owner_cause: "old_format", owner: lead };
  x.peer.owner = null;
  assert.match(ownerFacts(x, {}), /Their nationality \(lead\)<\/dt><dd>XDD/);
});

test("an incomplete owner shows the recorded reason", () => {
  const x = { ...recorded({ org_id: "", org_short_tag: "", org_state: "missing", org_reason: "the identity carries no organization" }),
    owner_state: "incomplete", owner_reason: "the identity carries no organization" };
  assert.match(ownerFacts(x, {}), /<dt>Organization<\/dt><dd><span class="sub">[^<]*<\/span> <span class="sub">— the identity carries no organization/);
});

test("the local roster lead appears only when the directory carries no owner", () => {
  const lead = { kind: "machine", handle: "anvil-relay", org: "org-local", org_nation: "XBB", individual: "rivera-j" };
  for (const st of ["no_record", "excepted", "lead"]) {
    const x = { ...recorded(), owner_state: st, owner: lead };
    assert.doesNotMatch(ownerFacts(x, {}), /\(lead\)<\/dt>/, st);
  }
  const old = { ...recorded(), owner_state: "not_read", owner_cause: "old_format", owner: lead };
  old.peer.owner = null;
  assert.match(ownerFacts(old, {}), /Organization \(lead\)/);
});

test("a malformed exception is shown as malformed", () => {
  const x = { ...recorded(), owner_state: "no_record" };
  x.peer.exception = { reason: "", granted_by: "g", granted_on: "2026-01-05", review_by: "", state: "malformed", problem: "no reason" };
  assert.match(ownerFacts(x, {}), /malformed \(no reason\)/);
});

test("the page says the peer directory is old, and no longer cites the roster as the owner source", () => {
  const now = 10 * 3600 * 1000;
  const note = namesNote({ directory: { peers_configured: true, roster_configured: true,
    peers_generated_ms: now - 45 * 60000, peers_max_age_ms: 20 * 60000 } }, now);
  assert.match(note, /last written 45 min ago/);
  assert.doesNotMatch(note, /read from the identity roster/);
});
