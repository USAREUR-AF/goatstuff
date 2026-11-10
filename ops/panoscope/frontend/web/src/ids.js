// SPDX-License-Identifier: Apache-2.0
//
// Short-form identity for labels and panel titles. One rule, shared by
// the canvas label (render.ts) and the info panel / stream headers
// (main.ts): a real Zenoh ZID (16-32 hex chars) is abbreviated to its
// last 8 characters; anything else is an already-human id — a slot
// alias, a topic-prefix identity such as `ops/fabric` or
// `caracal/power-demo` — and is returned whole. Cutting those to 8
// characters produced "s/fabric", "sistency" and "wer-demo" in the field.

const ZID_RE = /^[0-9a-f]{16,32}$/i;

export function looksLikeZid(id) {
  return ZID_RE.test(id);
}

export function shortId(id) {
  if (!looksLikeZid(id)) return id;
  return id.length > 8 ? id.slice(-8) : id;
}
