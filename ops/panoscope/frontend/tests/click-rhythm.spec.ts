// SPDX-License-Identifier: Apache-2.0
//
// E2E spec covering the panoscope unified fold interaction model
// (docs/design/panoscope.md §"Fold/unfold interaction"): SINGLE click
// always selects; DOUBLE click always toggles the fold of the node under
// the cursor. The two gestures never overlap outcomes.
//
//   1. Search overlay (`/` then click match OR `/` then Enter) updates
//      BOTH canvas selection AND the side panel + stream pane.
//   2. A plain single click ONLY selects — clicking any peer (even one
//      already in a multi-selection) narrows selection to it and NEVER
//      collapses the vendor. Clicking the same peer twice is still just
//      two selects, no fold.
//   3. Double-click a peer folds JUST that peer into one group (not its
//      vendor). Double-click a vendor's hull collapses the whole vendor;
//      double-click the resulting organisation node expands it back. Each
//      gesture is a view-parameter edit the server answers.
//   4. Click a topic entry in the side-panel `publishes` list pivots the
//      stream pane to that single topic (`#info-stream-topic-label`
//      shows the topic key, not "N topics").
//
// Drives `vite dev` against the mocked backend in fixtures/backend.ts. The
// fixture is a small two-vendor graph: vendor `acme` with three peers
// (acme-alpha / acme-beta / acme-gamma), each publishing one topic; vendor
// `beta` with one peer.

import { test, expect, type Page } from "@playwright/test";
import {
  hullDblClick,
  installBackend,
  nodeEvent,
  nodeIds,
  selectedIds,
  waitForNodes,
  waitForNoNodes,
  type FxPeer,
} from "./fixtures/backend";

const PEERS: FxPeer[] = [
  { id: "acme-alpha", vendor: "acme", publishes: ["acme/svc/temperature"] },
  { id: "acme-beta", vendor: "acme", publishes: ["acme/svc/pressure"] },
  { id: "acme-gamma", vendor: "acme", publishes: ["acme/svc/voltage"] },
  { id: "beta-one", vendor: "beta", publishes: ["beta/svc/sensor"] },
];

async function isVendorCollapsed(page: Page, name: string): Promise<boolean> {
  return (await nodeIds(page)).includes(`org:${name}`);
}

test.describe("panoscope click rhythm", () => {
  test.beforeEach(async ({ page }) => {
    await installBackend(page, { peers: PEERS });
    await page.goto("/");
    await waitForNodes(page, PEERS.map((p) => p.id));
  });

  test("search Enter pivots side panel to matched node (REGRESSION GUARD)", async ({
    page,
  }) => {
    // `/` opens the search overlay. Type a unique substring of one peer
    // so the active match is unambiguous, then Enter. After Enter, the
    // side-panel #info-title should name the matched peer and the
    // stream pane should be open (not stuck on whatever was selected
    // before — which here is "nothing").
    await page.keyboard.press("/");
    await page.locator("#search-input").fill("acme-alpha");
    await page.locator("#search-input").press("Enter");
    await expect(page.locator("#info-panel")).not.toHaveClass(/hidden/);
    await expect(page.locator("#info-title")).toContainText("acme-alpha");
  });

  test("single click only selects — never collapses, even when clicked twice (REGRESSION GUARD)", async ({
    page,
  }) => {
    // The unified model: a plain single click ALWAYS just selects. It
    // must never collapse a vendor — that is exclusively the double-click
    // / side-panel gesture now. Clicking a member of a multi-selection
    // narrows to it; clicking the same peer a second time is still just a
    // re-select, with the vendor untouched.
    await page.evaluate(() => {
      const r = (window as unknown as {
        __panoscopeRenderer?: { setSelection: (ids: string[]) => void };
      }).__panoscopeRenderer;
      r?.setSelection(["acme-alpha", "acme-beta", "acme-gamma"]);
    });
    await nodeEvent(page, "acme-alpha");
    expect(await selectedIds(page)).toEqual(["acme-alpha"]);
    expect(await isVendorCollapsed(page, "acme")).toBe(false);
    // Second single click on the same peer: still no collapse.
    await nodeEvent(page, "acme-alpha");
    expect(await selectedIds(page)).toEqual(["acme-alpha"]);
    expect(await isVendorCollapsed(page, "acme")).toBe(false);
  });

  test("double-click a peer folds just that peer (not its vendor)", async ({
    page,
  }) => {
    // Double-click on a peer folds ITS OWN topics into one group; the vendor
    // stays expanded. The group is selected when it is drawn, so the canvas
    // re-centres on it. Double-clicking the group unfolds the peer.
    await nodeEvent(page, "acme-alpha", "dblclick");
    const groupId = "grp:acme-alpha:pub";
    await waitForNodes(page, [groupId]);
    expect(await isVendorCollapsed(page, "acme")).toBe(false);
    await expect.poll(() => selectedIds(page)).toEqual([groupId]);
    await waitForNoNodes(page, ["topic:acme/svc/temperature"]);

    await nodeEvent(page, groupId, "dblclick");
    await waitForNodes(page, ["topic:acme/svc/temperature"]);
    await waitForNoNodes(page, [groupId]);
    await expect.poll(() => selectedIds(page)).toEqual(["acme-alpha"]);
  });

  test("double-click hull collapses vendor; double-click org node expands", async ({
    page,
  }) => {
    // The vendor-level toggle is symmetric across the hull ⇄ org-node pair:
    // double-click the cluster hull to collapse the whole vendor,
    // double-click the resulting org node to expand it back.
    await hullDblClick(page, "acme");
    await waitForNodes(page, ["org:acme"]);
    await waitForNoNodes(page, ["acme-alpha", "acme-beta", "acme-gamma"]);
    await expect.poll(() => selectedIds(page)).toEqual(["org:acme"]);
    await nodeEvent(page, "org:acme", "dblclick");
    await waitForNodes(page, ["acme-alpha", "acme-beta", "acme-gamma"]);
    expect(await isVendorCollapsed(page, "acme")).toBe(false);
  });

  test("click topic in publishes list pivots stream to single topic", async ({
    page,
  }) => {
    // After clicking a peer, the side-panel publishes list shows each
    // topic as a clickable row. Clicking a topic row should pivot the
    // stream pane to that ONE topic — #info-stream-topic-label should
    // show the topic key, not the "N topics" multi-merge label.
    await nodeEvent(page, "acme-alpha");
    await expect(page.locator("#info-title")).toContainText("acme-alpha");
    await page.locator(".topic-list-item .topic-name").first().click();
    await expect(page.locator("#info-stream-topic-label")).toHaveText(
      "acme/svc/temperature",
    );
  });
});
