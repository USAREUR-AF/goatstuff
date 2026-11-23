// SPDX-License-Identifier: Apache-2.0
import { test, expect } from "@playwright/test";
import { as, watch, noJunkText, post, freshPort } from "./helpers";

test("a tactical admin receives a system's data, and others can request it out", async ({ browser, request }) => {
  // Another feed already holds receive port 7100.
  await post(request, "platform", "/api/feeds", { direction: "in", format_id: "cot", label: "Existing feed", dest_name: "other-console",
    dest_ip: "198.51.100.60", port: 7100, purpose: "holds port 7100" });

  const ctx = await as(browser, "tactical");
  const page = await ctx.newPage();
  const w = watch(page);
  await page.goto("/#feeds");
  await page.getByRole("link", { name: "Receive a system's data" }).click();
  await expect(page.locator("#steps li").first()).toHaveText("What it sends");
  await page.click("#step-next");
  await expect(page.getByRole("alert")).toHaveText("Choose the standard the system sends.");
  await page.fill("#in-label", "Blue C-UAS drone position");
  await page.getByRole("button", { name: /MAVLink 2/ }).click();
  await page.click("#step-next");
  await page.fill("#dest-name", "console-west");
  await page.fill("#dest-ip", "198.51.100.42");
  await page.click("#step-next");
  const taken = page.getByRole("button", { name: "7100 (taken)" });
  await expect(taken).toBeDisabled();
  await page.getByRole("button", { name: "7101", exact: true }).click();
  await expect(page.locator(".next-box")).toContainText("198.51.100.9:7101");
  await page.screenshot({ path: test.info().outputPath("receive-port.png"), fullPage: true });
  await page.click("#step-next");
  await expect(page.locator(".next-box")).toContainText("send to 198.51.100.9:7101 over UDP");
  await page.fill("#purpose", "interceptor position into the picture");
  await noJunkText(page);
  await page.screenshot({ path: test.info().outputPath("receive-review.png"), fullPage: true });
  await page.click("#step-next");
  await expect(page).toHaveURL(/#feed\/feed-/);
  const feedId = page.url().split("/").pop()!;
  await expect(page.locator("#feed-head .sig.flow .w")).toHaveText("Yes");
  await expect(page.locator("#flow-title")).toHaveText("Receiving");
  await expect(page.locator("#check-btn")).toHaveText("Check sender");
  await expect(page.locator("#feed-details")).toContainText(`volatile/slot/stream/feeds-in/${feedId}`);
  await expect(page.locator("#feed-details")).toContainText(`in-${feedId}`);
  await expect(page.locator("pre.cmd")).toContainText("--in udp://198.51.100.9:7101");
  await page.click("#check-btn");
  await expect(page.locator("#check-result")).toContainText("counters above show whether anything arrives");
  await noJunkText(page);
  await page.screenshot({ path: test.info().outputPath("receive-feed.png"), fullPage: true });

  // The received feed is offered in the send flow and can be requested out.
  await page.goto("/#request?dir=out");
  await expect(page.locator(".group-label", { hasText: "Received from systems" })).toBeVisible();
  await page.getByRole("button", { name: /Blue C-UAS drone position/ }).click();
  await page.click("#step-next");
  await page.fill("#dest-name", "tak-server");
  await page.fill("#dest-ip", "198.51.100.55");
  await page.click("#step-next");
  await page.getByRole("button", { name: /CoT XML/ }).click();
  await page.fill("#port", String(freshPort()));
  await page.click("#step-next");
  await page.fill("#purpose", "drone position to TAK");
  await page.click("#step-next");
  await expect(page.locator("#feed-head h1")).toContainText("Blue C-UAS drone position  →  tak-server");
  await expect(page.locator("pre.cmd")).toContainText(`zenoh:volatile/slot/stream/feeds-in/${feedId}`);
  w.check();
  await ctx.close();
});

test("a member's receive request waits for an admin", async ({ browser }) => {
  const ctx = await as(browser, "member");
  const page = await ctx.newPage();
  await page.goto("/#request?dir=in");
  await page.fill("#in-label", "Effector status");
  await page.getByRole("button", { name: /Cursor on Target/ }).click();
  await page.click("#step-next");
  await page.fill("#dest-name", "effector-2");
  await page.fill("#dest-ip", "198.51.100.43");
  await page.click("#step-next");
  await page.click("#step-next");
  await expect(page.locator(".next-box")).toContainText("waits for");
  await page.fill("#purpose", "status into the picture");
  await page.click("#step-next");
  await expect(page.locator("#feed-head .sig.int .n")).toHaveText("Waiting for approval.");
  await ctx.close();
});
