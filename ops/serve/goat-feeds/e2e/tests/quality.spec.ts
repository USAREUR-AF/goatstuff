// SPDX-License-Identifier: Apache-2.0
// Checks that catch the unpredictable ways a page breaks: errors in the console, text that
// renders a missing value, layouts that overflow on a phone or a wide screen, a dark mode that
// does not apply, and controls a keyboard cannot reach.
import { test, expect } from "@playwright/test";
import { as, watch, noJunkText, noHorizontalOverflow, post, freshPort } from "./helpers";

let outId = "", inId = "";
test.beforeAll(async ({ request }) => {
  outId = (await post(request, "tactical", "/api/feeds", { topic_id: "fused", output_id: "cat062", dest_name: "quality-out",
    dest_ip: "198.51.100.90", port: freshPort(), purpose: "quality checks, out" })).id;
  inId = (await post(request, "tactical", "/api/feeds", { direction: "in", format_id: "cot", label: "Quality receive",
    dest_name: "quality-in", dest_ip: "198.51.100.91", port: 7103, purpose: "quality checks, in" })).id;
  // A declaration with a long unbroken note, so the feed page, list and catalog are checked with it.
  await post(request, "tactical", `/api/feeds/${inId}/sim-status`, { mode: "simulated",
    note: "bench-run-" + "x".repeat(200) + " the source team says the autopilot is in simulation" });
});

const pages = () => [
  ["feeds", "/#feeds"], ["send", "/#request?dir=out"], ["receive", "/#request?dir=in"], ["approvals", "/#approvals"],
  ["audit", "/#audit"], ["out-feed", `/#feed/${outId}`], ["in-feed", `/#feed/${inId}`],
];

for (const width of [390, 1440]) {
  for (const scheme of ["light", "dark"] as const) {
    test(`every page at ${width}px, ${scheme}: no errors, no junk text, no overflow`, async ({ browser }) => {
      const ctx = await as(browser, "platform", { viewport: { width, height: 900 }, colorScheme: scheme });
      const page = await ctx.newPage();
      const w = watch(page);
      for (const [name, url] of pages()) {
        await page.goto(url);
        await page.waitForLoadState("networkidle");
        await expect(page.locator("#main .view")).toBeVisible();
        await noJunkText(page);
        await noHorizontalOverflow(page);
        await page.screenshot({ path: test.info().outputPath(`${name}-${width}-${scheme}.png`), fullPage: true });
      }
      w.check();
      await ctx.close();
    });
  }
}

test("dark mode changes the page colors", async ({ browser }) => {
  const bg = async (scheme: "light" | "dark") => {
    const ctx = await as(browser, "member", { colorScheme: scheme });
    const page = await ctx.newPage();
    await page.goto("/#feeds");
    await expect(page.locator("#main .view")).toBeVisible();
    const v = await page.evaluate(() => [getComputedStyle(document.body).backgroundColor, getComputedStyle(document.body).color]);
    await ctx.close();
    return v;
  };
  const light = await bg("light"), dark = await bg("dark");
  expect(dark[0]).not.toBe(light[0]);
  expect(dark[1]).not.toBe(light[1]);
});

test("the request flow works from the keyboard alone", async ({ browser }) => {
  const ctx = await as(browser, "tactical");
  const page = await ctx.newPage();
  await page.goto("/#request?dir=out");
  await expect(page.locator("#steps li").first()).toHaveText("Topic");
  let reached = false;
  for (let i = 0; i < 40 && !reached; i++) {
    await page.keyboard.press("Tab");
    reached = await page.evaluate(() => document.activeElement?.id === "step-next");
  }
  expect(reached, "Tab never reached the Next button").toBeTruthy();
  // Choosing a topic with the keyboard: focus the first topic and press Enter.
  await page.locator("button.pick").first().focus();
  await page.keyboard.press("Enter");
  await expect(page.locator("button.pick.on")).toHaveCount(1);
  await ctx.close();
});

test("signing out is offered and the role is shown", async ({ browser }) => {
  for (const [role, label] of [["member", "Member"], ["tactical", "Tactical admin"], ["platform", "Platform admin"]] as const) {
    const ctx = await as(browser, role);
    const page = await ctx.newPage();
    await page.goto("/#feeds");
    await expect(page.locator("#who .role")).toHaveText(label);
    await expect(page.locator("#who a")).toHaveAttribute("href", "/oauth2/sign_out");
    await expect(page.locator("#nav-approvals")).toBeVisible({ visible: role !== "member" });
    await ctx.close();
  }
});
