// SPDX-License-Identifier: Apache-2.0
import { test, expect } from "@playwright/test";
import { as, watch, noJunkText, noHorizontalOverflow } from "./helpers";

// A link started by a deployment file shows beside the requested feeds, with the same two
// columns, read-only, naming what controls it (ADR 1260).
test("a declared link shows as its own row with Integrated and Sending", async ({ browser }) => {
  const ctx = await as(browser, "tactical");
  const page = await ctx.newPage();
  const errors = watch(page);
  await page.goto("/#feeds");
  await page.locator(".chip[data-filter=all]").click();
  const row = page.locator(".card.declared", { hasText: "Fused air picture (declared)" });
  await expect(row).toBeVisible();
  await expect(row.locator(".dir.decl")).toHaveText("DECLARED");
  await expect(row.locator(".sig.int .w")).toHaveText("Yes");
  await expect(row.locator(".sig.flow .k")).toHaveText("Sending");
  await expect(row.locator(".sig.flow .w")).toHaveText("Yes");
  await expect(row).toContainText("Controlled by destinations.test.enabled");
  await expect(row).toContainText("198.51.100.70:7005/udp");
  // Mine never lists a declared link: nobody owns it through this page.
  await page.locator(".chip[data-filter=mine]").click();
  await expect(page.locator(".card.declared")).toHaveCount(0);
  await noJunkText(page);
  await noHorizontalOverflow(page);
  errors.check();
  await ctx.close();
});
