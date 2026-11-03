// SPDX-License-Identifier: Apache-2.0
//
// E2E spec for the structural last-good-frame guard around setView
// (docs/design/panoscope.md §"Dangling-edge guard" + docs/findings/F-210).
//
// The render path used to be one big throwable block: ANY throw inside the
// apply blanked the canvas (0/0 peers) on a single malformed frame. setView
// wraps the apply: on throw it restores + repaints the previously committed
// frame and re-throws, and the page shows the error banner. The surface
// DEGRADES — it keeps the last true picture — it never blanks.
//
// This test proves it independently of the edge-guard: it forces a throw the
// edge-guard does NOT catch (`topics: null` → the format loop at the top of
// applyView throws TypeError before any node mutation), so it exercises the
// wrapper itself, not the boundary validation.

import { test, expect } from "@playwright/test";
import { installBackend, nodeIds, refreshView, waitForNodes } from "./fixtures/backend";

test("a throwing view keeps the last good frame on screen and surfaces the banner — never blanks", async ({
  page,
}) => {
  const backend = await installBackend(page, {
    peers: [
      { id: "acme-alpha", vendor: "acme", publishes: ["acme/svc/temperature"] },
      { id: "acme-beta", vendor: "acme", publishes: ["acme/svc/pressure"] },
    ],
  });
  await page.goto("/");

  // The good frame renders both peers.
  await waitForNodes(page, ["acme-alpha", "acme-beta"]);
  const before = await page.locator("g.node").count();
  expect(before).toBeGreaterThan(0);

  // Serve a malformed view: topics=null makes the format loop at the top of
  // applyView throw — a thrower the edge-guard cannot catch.
  backend.overrideView((_params, built) => ({
    ...built,
    peers: [{ ...built.peers[0], peer_id: "ghost" }],
    topics: null,
  }));
  await refreshView(page);

  // The error banner fires (the surface stays honest about the failure)…
  await expect(page.locator("#render-error-banner")).toBeVisible();
  // …and the LAST GOOD FRAME is still on screen — the prior peers remain,
  // the malformed frame's "ghost" node did NOT replace them, the canvas
  // did NOT blank to 0 nodes.
  const after = await nodeIds(page);
  expect(after).toContain("acme-alpha");
  expect(after).toContain("acme-beta");
  expect(after).not.toContain("ghost");
  expect(await page.locator("g.node").count()).toBe(before);
});
