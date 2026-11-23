// SPDX-License-Identifier: Apache-2.0
import { test, expect } from "@playwright/test";
import { as, watch, noJunkText, freshPort } from "./helpers";

test("a tactical admin sends a topic to a system and sees it deliver", async ({ browser }) => {
  const ctx = await as(browser, "tactical");
  const page = await ctx.newPage();
  const w = watch(page);
  await page.goto("/#request?dir=out");
  await expect(page.locator("#steps li").first()).toHaveText("Topic");
  // Next without a topic explains what is missing.
  await page.click("#step-next");
  await expect(page.getByRole("alert")).toHaveText("Choose a topic.");
  await page.getByRole("button", { name: /Fused air picture Real data/ }).click();
  await page.click("#step-next");
  await page.fill("#dest-name", "console-west");
  await page.fill("#dest-ip", "198.51.100.21");
  await page.click("#step-next");
  await page.getByRole("button", { name: /ASTERIX CAT062 \(SAC 38/ }).click();
  await page.fill("#port", String(freshPort()));
  await page.click("#step-next");
  await expect(page.locator(".next-box")).toContainText("switches on as soon as you submit");
  await page.fill("#purpose", "air picture to the console");
  await noJunkText(page);
  await page.screenshot({ path: test.info().outputPath("send-review.png"), fullPage: true });
  await page.click("#step-next");
  await expect(page).toHaveURL(/#feed\/feed-/);
  await expect(page.locator("#feed-head .sig.flow .w")).toHaveText("Yes");
  // The list shows the two facts as separate labeled columns, not one word.
  await page.goto("/#feeds");
  const row = page.locator(".card.feed-row").first();
  await expect(row.locator(".sig.int .k")).toHaveText("Integrated");
  await expect(row.locator(".sig.flow .k")).toHaveText("Sending");
  await expect(row.locator(".sig.int .w")).toHaveText(/Yes|No/);
  await row.click();
  await expect(page.locator("#feed-head .sig.flow .w")).toHaveText("Yes");
  await expect(page.locator("#feed-stats")).toContainText(/In from topic\s*\d+/);
  await expect(page.locator("pre.cmd")).toContainText("--to-format asterix-cat062");
  await expect(page.locator("#timeline")).toContainText("on by tactical@example.test");
  await noJunkText(page);
  await page.screenshot({ path: test.info().outputPath("send-feed.png"), fullPage: true });
  w.check();
  await ctx.close();
});

test("a synthetic topic is self-service for a member", async ({ browser }) => {
  const ctx = await as(browser, "member");
  const page = await ctx.newPage();
  const w = watch(page);
  await page.goto("/#request?dir=out");
  await page.getByRole("button", { name: /Fused air picture \(simulated\)/ }).click();
  await page.click("#step-next");
  await page.fill("#dest-name", "test-receiver");
  await page.fill("#dest-ip", "198.51.100.30");
  await page.click("#step-next");
  await page.fill("#port", String(freshPort()));
  await page.click("#step-next");
  await expect(page.locator(".next-box")).toContainText("synthetic");
  await page.fill("#purpose", "testing a receiver");
  await page.click("#step-next");
  await expect(page.locator("#feed-head .sig.int .n")).toHaveText(/Starting|Process running/);
  w.check();
  await ctx.close();
});

test("the send flow refuses an address off the mesh with the server's reason", async ({ browser }) => {
  const ctx = await as(browser, "tactical");
  const page = await ctx.newPage();
  await page.goto("/#request?dir=out");
  await page.getByRole("button", { name: /Radar tracks \(raw\)/ }).click();
  await page.click("#step-next");
  await page.fill("#dest-name", "outside");
  await page.fill("#dest-ip", "203.0.113.9");
  await page.click("#step-next");
  await page.fill("#port", String(freshPort()));
  await page.click("#step-next");
  await page.fill("#purpose", "should be refused");
  await page.click("#step-next");
  await expect(page.getByRole("alert")).toContainText("must be a mesh address");
  await ctx.close();
});
