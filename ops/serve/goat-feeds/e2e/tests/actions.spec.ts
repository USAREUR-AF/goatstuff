// SPDX-License-Identifier: Apache-2.0
import { test, expect } from "@playwright/test";
import { as, watch, post, freshPort } from "./helpers";

test("the owner pauses, resumes and removes a feed from its page; the audit log records it", async ({ browser, request }) => {
  const f = await post(request, "tactical", "/api/feeds", { topic_id: "fused", output_id: "cat062", dest_name: "actions-target",
    dest_ip: "198.51.100.80", port: freshPort(), purpose: "actions test" });
  const ctx = await as(browser, "tactical");
  const page = await ctx.newPage();
  const w = watch(page);
  await page.goto(`/#feed/${f.id}`);
  await expect(page.locator("#feed-head .h")).toHaveText(/starting|sending/);

  await page.locator("#feed-actions").getByRole("button", { name: "Pause" }).click();
  await expect(page.locator("#note-box")).toBeVisible();
  await page.fill("#note-input", "receiver maintenance");
  await page.click("#note-ok");
  await expect(page.locator("#feed-head .h")).toHaveText("paused");

  await page.locator("#feed-actions").getByRole("button", { name: "Resume" }).click();
  await expect(page.locator("#feed-head .h")).toHaveText(/starting|sending/);

  await page.locator("#feed-actions").getByRole("button", { name: "Renew" }).click();
  await expect(page.locator("#note-input")).toHaveValue(/\d+/);
  await page.fill("#note-input", "14 extended for the exercise");
  await page.click("#note-ok");
  await expect(page.locator("#feed-details")).toContainText("14 d left");

  await page.locator("#feed-actions").getByRole("button", { name: "Remove" }).click();
  await page.click("#note-ok");
  await expect(page.locator("#feed-details")).toContainText("removed");
  await expect(page.locator("#feed-actions button")).toHaveCount(0);

  await page.goto("/#audit");
  const rows = page.locator("#audit-body tr", { hasText: f.id });
  await expect(rows).toHaveCount(6); // request, on, pause, resume, renew, remove
  await expect(rows.first()).toContainText("remove");
  await page.screenshot({ path: test.info().outputPath("audit.png"), fullPage: true });
  w.check();
  await ctx.close();
});

test("the note box can be cancelled without changing the feed", async ({ browser, request }) => {
  const f = await post(request, "tactical", "/api/feeds", { topic_id: "fused", output_id: "cat062", dest_name: "cancel-target",
    dest_ip: "198.51.100.81", port: freshPort(), purpose: "cancel test" });
  const ctx = await as(browser, "tactical");
  const page = await ctx.newPage();
  await page.goto(`/#feed/${f.id}`);
  await page.locator("#feed-actions").getByRole("button", { name: "Pause" }).click();
  await page.click("#note-cancel");
  await expect(page.locator("#note-box")).toBeHidden();
  await expect(page.locator("#feed-head .h")).not.toHaveText("paused");
  await ctx.close();
});

test("the feeds list filters and searches", async ({ browser, request }) => {
  await post(request, "member", "/api/feeds", { topic_id: "sim", output_id: "cat062", dest_name: "filter-mine",
    dest_ip: "198.51.100.82", port: freshPort(), purpose: "filter test" });
  const ctx = await as(browser, "member");
  const page = await ctx.newPage();
  await page.goto("/#feeds");
  await page.getByRole("button", { name: "Mine" }).click();
  await expect(page.locator(".card", { hasText: "filter-mine" })).toBeVisible();
  await expect(page.locator(".card", { hasText: "actions-target" })).toHaveCount(0);
  await page.getByRole("button", { name: "All" }).click();
  await page.fill("#feed-search", "filter-mine");
  await expect(page.locator(".card")).toHaveCount(1);
  await page.fill("#feed-search", "no-such-feed-anywhere");
  await expect(page.locator(".empty")).toHaveText("No feeds match.");
  await ctx.close();
});
