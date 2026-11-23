// SPDX-License-Identifier: Apache-2.0
import { test, expect } from "@playwright/test";
import { as, watch, noJunkText, noHorizontalOverflow } from "./helpers";

// Many sites and systems (fabric-feeds.md §11): needs-attention first, facets from the
// estate, grouping with counts, every filter in the page address, grouped flow boxes.
test("an admin lands on needs attention, groups by site, and the address keeps it", async ({ browser }) => {
  const ctx = await as(browser, "tactical");
  const page = await ctx.newPage();
  const errors = watch(page);
  await page.goto("/#feeds");
  await expect(page.locator(".chip[data-filter=attention]")).toHaveClass(/on/);

  await page.goto("/#feeds?filter=all&group=site");
  await page.reload();
  const alpha = page.locator("details.group", { hasText: "Alpha" }).first();
  await expect(alpha.locator(".gcount")).toContainText(/\d+ links?/);
  if (!(await alpha.getAttribute("open"))) await alpha.locator("summary").click();
  const decl = alpha.locator(".card.declared");
  await expect(decl).toContainText("Alpha → Alpha · console");

  await page.selectOption("#f-group", "dest_role");
  await expect(page).toHaveURL(/group=dest_role/);
  await page.selectOption("#f-group", "");
  await page.selectOption("#f-site", "Alpha");
  await expect(page).toHaveURL(/site=Alpha/);
  await expect(page.locator(".card.declared")).toHaveCount(1);

  await page.goto("/#feeds?filter=all&group=site&view=flow");
  await page.reload();
  await expect(page.locator(".fnode.c0", { hasText: "Sensors, Alpha" })).toHaveCount(1);
  await noJunkText(page);
  await noHorizontalOverflow(page);
  errors.check();
  await ctx.close();
});

test("the API filters and counts by facet", async ({ request }) => {
  const h = { "X-Forwarded-Email": "tactical@example.test", "X-Forwarded-Groups": "TACTICAL-ADMIN" };
  const all = await (await request.get("/api/feeds", { headers: h })).json();
  const alpha = await (await request.get("/api/feeds?dest_site=alpha", { headers: h })).json();
  expect(alpha.feeds.length).toBeGreaterThan(0);
  expect(alpha.feeds.length).toBeLessThanOrEqual(all.feeds.length);
  for (const f of alpha.feeds) expect(f.facets.dest_site).toBe("Alpha");
  const sum = await (await request.get("/api/feeds/summary?group=site", { headers: h })).json();
  expect(sum.groups.find((g: { key: string }) => g.key === "Alpha").total).toBeGreaterThan(0);
  expect((await request.get("/api/feeds/summary?group=nonsense", { headers: h })).status()).toBe(400);
});
