// SPDX-License-Identifier: Apache-2.0
import { test, expect } from "@playwright/test";
import { as, watch, noJunkText, noHorizontalOverflow } from "./helpers";

// The flow view draws sensors -> streams -> receiving systems, with the line style carrying
// Sending / Integrated, and a box leads back to its feeds in the list.
test("the flow view draws the declared link from its sensors to its system", async ({ browser }) => {
  for (const viewport of [{ width: 1440, height: 900 }, { width: 390, height: 844 }]) {
    const ctx = await as(browser, "tactical", { viewport });
    const page = await ctx.newPage();
    const errors = watch(page);
    await page.goto("/#feeds");
    await page.locator(".chip[data-filter=all]").click();
    await page.locator(".viewseg button[data-view=flow]").click();
    const flow = page.locator("#feed-flow svg.flowsvg");
    await expect(flow).toBeVisible();
    await expect(page.locator("#feed-list")).toBeHidden();
    await expect(flow.locator(".fnode.c0", { hasText: "Radar A" })).toHaveCount(1);
    await expect(flow.locator(".fnode.c0", { hasText: "Radar B" })).toHaveCount(1);
    await expect(flow.locator(".fnode.c1", { hasText: "Fused air picture (declared)" })).toHaveCount(1);
    await expect(flow.locator(".fnode.c2", { hasText: "198.51.100.70:7005" })).toHaveCount(1);
    await expect(flow.locator(".fedge.on").first()).toBeAttached();
    await expect(page.locator(".flegend")).toContainText("Not integrated");
    await noJunkText(page);
    await noHorizontalOverflow(page);
    // A box leads back to the list, filtered to it.
    await flow.locator(".fnode.c2", { hasText: "198.51.100.70:7005" }).click();
    await expect(page.locator("#feed-list")).toBeVisible();
    await expect(page.locator("#feed-search")).toHaveValue("198.51.100.70");
    await expect(page.locator(".card.declared")).toHaveCount(1);
    errors.check();
    await ctx.close();
  }
});
