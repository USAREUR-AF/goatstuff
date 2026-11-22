// SPDX-License-Identifier: Apache-2.0
import { test, expect } from "@playwright/test";
import { as, watch, post, freshPort } from "./helpers";

test("a member's real-data request waits, a tactical admin approves it on the Approvals page", async ({ browser, request }) => {
  const f = await post(request, "member", "/api/feeds", { topic_id: "fused", output_id: "sapient", dest_name: "approval-target",
    dest_ip: "198.51.100.70", port: freshPort(), purpose: "please approve me" });
  expect(f.status).toBe("pending");

  const memberCtx = await as(browser, "member");
  const mp = await memberCtx.newPage();
  await mp.goto("/#feeds");
  await expect(mp.locator("#nav-approvals")).toBeHidden();
  await expect(mp.locator(".card", { hasText: "approval-target" }).locator(".h")).toHaveText("waiting");
  await memberCtx.close();

  const ctx = await as(browser, "tactical");
  const page = await ctx.newPage();
  const w = watch(page);
  await page.goto("/#approvals");
  await expect(page.locator("#approvals-count")).toBeVisible();
  const card = page.locator(".card", { hasText: "approval-target" });
  await expect(card).toContainText("Purpose: please approve me");
  await page.screenshot({ path: test.info().outputPath("approvals.png"), fullPage: true });
  await card.getByRole("button", { name: "Approve" }).click();
  await expect(page.locator("#toast")).toContainText("Approve: done.");
  await expect(page.locator(".card", { hasText: "approval-target" })).toHaveCount(0);
  await page.goto(`/#feed/${f.id}`);
  await expect(page.locator("#feed-head .h")).toHaveText(/starting|sending/);
  await expect(page.locator("#timeline")).toContainText("approve by tactical@example.test");
  w.check();
  await ctx.close();
});

test("declining needs a note, and the requester sees it", async ({ browser, request }) => {
  const f = await post(request, "member", "/api/feeds", { topic_id: "radar", output_id: "cat062", dest_name: "decline-target",
    dest_ip: "198.51.100.71", port: freshPort(), purpose: "please decline me" });
  const ctx = await as(browser, "tactical");
  const page = await ctx.newPage();
  await page.goto("/#approvals");
  const card = page.locator(".card", { hasText: "decline-target" });
  await card.getByRole("button", { name: "Decline" }).click();
  await expect(page.locator("#toast")).toContainText("Say why it is declined");
  await card.getByLabel("Decision note").fill("use the simulated topic for testing");
  await card.getByRole("button", { name: "Decline" }).click();
  await expect(page.locator("#toast")).toContainText("Decline: done.");
  await ctx.close();

  const mctx = await as(browser, "member");
  const mp = await mctx.newPage();
  await mp.goto(`/#feed/${f.id}`);
  await expect(mp.locator("#feed-details")).toContainText("use the simulated topic for testing");
  await expect(mp.locator("#feed-actions")).not.toContainText("Approve");
  await mctx.close();
});

test("a member cannot approve through the API either", async ({ request }) => {
  const f = await post(request, "member", "/api/feeds", { topic_id: "fused", output_id: "cat062", dest_name: "self-approve",
    dest_ip: "198.51.100.72", port: freshPort(), purpose: "self approval attempt" });
  const r = await request.post(`/api/feeds/${f.id}/approve`, { headers: { "X-Forwarded-Email": "member@example.test", "X-Goat-Feeds": "1" }, data: {} });
  expect(r.status()).toBe(403);
});
