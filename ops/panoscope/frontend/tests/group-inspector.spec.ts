// SPDX-License-Identifier: Apache-2.0
//
// E2E spec for the panoscope group inspector — the 4th side-panel mode.
// A group is many of one publisher's topics the server drew as one element
// (docs/design/panoscope-view-model.md). Contract:
//
//   1. Clicking a group opens the inspector in the side panel; it does not
//      change the canvas (graph viz is for relationships, a list is for
//      inventory).
//   2. The inspector lists members from /api/view/group, and its filter is
//      answered by the server (the `q` it sends), not by the browser.
//   3. Clicking a member row drills into that single topic (the stream pane
//      pivots to it — the same topic mode pins use).
//   4. A family too large to draw (over the server's `max_open_units`) has
//      no "show on canvas"; a small one does, and it opens the group — the
//      page asks for a view with the group id in `expand` and draws the pins.
//   5. "open in fleet search" is always offered.
//
// Backend mocked by fixtures/backend.ts.

import { test, expect, type Page } from "@playwright/test";
import { installBackend, nodeEvent, waitForNodes, type Backend } from "./fixtures/backend";

const PUBLISHER = "vendor-c-pub";
const BIG = "grp:vendor-c-pub:fam:00000000000000aa:vendor-c/v1/entities";
const SMALL = "grp:vendor-c-pub:fam:00000000000000bb:vendor-c/v1/tracks";
const BIG_COUNT = 2_500; // over max_open_units (2,000)
const SMALL_COUNT = 12;

const entities = Array.from({ length: BIG_COUNT }, (_, i) => `vendor-c/v1/entities/uuid-${String(i).padStart(4, "0")}`);
const tracks = Array.from({ length: SMALL_COUNT }, (_, i) => `vendor-c/v1/tracks/t${String(i).padStart(2, "0")}`);

async function setup(page: Page): Promise<Backend> {
  const backend = await installBackend(page, {
    peers: [{ id: PUBLISHER, vendor: "vendor-c", publishes: [...entities, ...tracks] }],
    groups: [
      { id: BIG, kind: "family", publisher: PUBLISHER, label: "vendor-c/v1/entities/*", selector: "vendor-c/v1/entities/*", members: entities },
      { id: SMALL, kind: "family", publisher: PUBLISHER, label: "vendor-c/v1/tracks/*", selector: "vendor-c/v1/tracks/*", members: tracks },
    ],
  });
  await page.goto("/");
  await waitForNodes(page, [BIG, SMALL]);
  // The welcome card auto-dismisses 4s after first data; the row click would
  // otherwise race it. Fire its own dismiss handler.
  await page.evaluate(() => (document.querySelector(".welcome-dismiss") as HTMLElement | null)?.click());
  await expect(page.locator(".welcome-card")).toHaveClass(/welcome-card-hidden/);
  return backend;
}

test.describe("panoscope group inspector", () => {
  test("clicking a group opens the inspector, not a canvas change", async ({ page }) => {
    const backend = await setup(page);
    const requestsBefore = backend.viewRequests.length;
    await nodeEvent(page, BIG);
    await expect(page.locator("#info-panel")).not.toHaveClass(/hidden/);
    await expect(page.locator("#info-title")).toContainText("vendor-c/v1/entities/*");
    await expect(page.locator("#info-title")).toContainText(`(${BIG_COUNT} topics)`);
    await expect(page.locator(".ns-inspector-search-input")).toBeVisible();
    // Too large to draw: no "show on canvas", but the list and fleet search.
    await expect(page.locator(".ns-inspector-show-canvas")).toHaveCount(0);
    await expect(page.locator(".ns-inspector-explode-note")).toContainText("too many to draw");
    await expect(page.locator(".ns-inspector-fleet-search")).toBeVisible();
    // A click never edits the view parameters.
    for (const p of backend.viewRequests.slice(requestsBefore)) expect(p.expand).toEqual([]);
  });

  test("lists members from the server and filters server-side", async ({ page }) => {
    await setup(page);
    await nodeEvent(page, BIG);
    await expect(page.locator(".ns-inspector-row-name").first()).toHaveText(entities[0]);
    const filtered = page.waitForRequest(
      (r) => r.url().endsWith("/api/view/group") && (r.postDataJSON() as { q: string }).q === "uuid-2499",
    );
    await page.locator(".ns-inspector-search-input").fill("uuid-2499");
    await filtered;
    await expect(page.locator(".ns-inspector-row")).toHaveCount(1);
    await expect(page.locator(".ns-inspector-row-name")).toHaveText(entities[BIG_COUNT - 1]);
    await expect(page.locator(".ns-inspector-stats")).toContainText(`1 / ${BIG_COUNT}`);
  });

  test("clicking a member row drills into that single topic's stream", async ({ page }) => {
    await setup(page);
    await nodeEvent(page, BIG);
    await expect(page.locator(".ns-inspector-row").first()).toBeVisible();
    await page.locator(".ns-inspector-row").first().click();
    await expect(page.locator("#info-title")).toContainText(entities[0]);
    await expect(page.locator("#info-stream-topic-label")).toHaveText(entities[0]);
  });

  test("show on canvas opens a small group and draws its topics", async ({ page }) => {
    const backend = await setup(page);
    await nodeEvent(page, SMALL);
    await expect(page.locator(".ns-inspector-show-canvas")).toHaveText("show on canvas");
    await page.locator(".ns-inspector-show-canvas").click();
    await waitForNodes(page, tracks.map((t) => `topic:${t}`));
    expect(backend.lastParams()?.expand).toEqual([SMALL]);
  });
});
