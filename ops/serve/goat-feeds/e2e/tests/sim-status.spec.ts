// SPDX-License-Identifier: Apache-2.0
// Real or simulated: the team operating a source declares, per received feed, whether the source
// is sending real or simulated data. The declaration is shown beside the data, served at
// GET /api/sim-status, and never changes what is sent.
import { test, expect } from "@playwright/test";
import { as, watch, noJunkText, noHorizontalOverflow, post } from "./helpers";

const NOTE = `ops desk: autopilot in simulation <img src=x onerror="window.__pwned=1">`;

test("the requester declares a received feed simulated; everyone sees it; the API serves it", async ({ browser, request }) => {
  const f = await post(request, "member", "/api/feeds", { direction: "in", format_id: "mavlink2", label: "drone-a position",
    dest_name: "drone-a", dest_ip: "198.51.100.44", port: 7104, purpose: "drone position into the picture" });

  const ctx = await as(browser, "member");
  const page = await ctx.newPage();
  const w = watch(page);
  await page.goto(`/#feed/${f.id}`);
  await expect(page.locator("#sim-head")).toHaveText("Real or simulated: Not declared");
  await expect(page.locator("#feed-details")).toContainText("Not declared");

  // The action states its result before it runs.
  await page.locator("#feed-actions").getByRole("button", { name: "Real or simulated" }).click();
  await expect(page.locator("#sim-box")).toBeVisible();
  await expect(page.locator("#sim-result")).toHaveText(/does not change the data that is received or sent/);
  await page.click("#sim-ok");
  await expect(page.locator("#toast")).toHaveText("Choose real, simulated or unknown.");
  // Touch targets: each choice and the buttons are at least 44 px tall.
  for (const loc of [page.locator(".sim-opt").first(), page.locator("#sim-ok"), page.locator("#sim-cancel")]) {
    const box = await loc.boundingBox();
    expect(box && box.height, "touch target height").toBeGreaterThanOrEqual(44);
  }
  await page.getByLabel(/Simulated/).check();
  await page.fill("#sim-note", NOTE);
  await page.screenshot({ path: test.info().outputPath("sim-box.png"), fullPage: true });
  await page.click("#sim-ok");
  await expect(page.locator("#sim-box")).toBeHidden();
  await expect(page.locator("#sim-head")).toContainText("Real or simulated: Declared simulated by member@example.test, ");
  // Text is placed as text: the note shows literally and nothing in it runs.
  await expect(page.locator("#sim-head")).toContainText(NOTE);
  await expect(page.locator("#sim-head img")).toHaveCount(0);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
  await expect(page.locator("#feed-details")).toContainText("Declared simulated by member@example.test");
  // It changes nothing else: the feed still waits for approval.
  await expect(page.locator("#feed-head .sig.int .n")).toHaveText("Waiting for approval.");
  await expect(page.locator("#timeline")).toContainText("sim-status by member@example.test");
  await noJunkText(page);
  await page.screenshot({ path: test.info().outputPath("sim-declared.png"), fullPage: true });

  // The feed card in the list says the same.
  await page.goto("/#feeds");
  await page.getByRole("button", { name: "Mine" }).click();
  await expect(page.locator(".card", { hasText: "drone-a position" }).locator(".simline")).toContainText("Declared simulated by member@example.test");
  w.check();
  await ctx.close();

  // Another member sees it but cannot change it.
  const other = await browser.newContext({ extraHTTPHeaders: { "X-Forwarded-Email": "other@example.test", "X-Forwarded-Groups": "DEVELOPER" } });
  const op = await other.newPage();
  await op.goto(`/#feed/${f.id}`);
  await expect(op.locator("#sim-head")).toContainText("Declared simulated");
  await expect(op.locator("#feed-actions").getByRole("button", { name: "Real or simulated" })).toHaveCount(0);
  await other.close();

  // A tactical admin switches it on and changes the declaration; the audit log has both.
  await post(request, "tactical", `/api/feeds/${f.id}/approve`, { note: "ok" });
  const actx = await as(browser, "tactical");
  const ap = await actx.newPage();
  await ap.goto(`/#feed/${f.id}`);
  await ap.locator("#feed-actions").getByRole("button", { name: "Real or simulated" }).click();
  await expect(ap.getByLabel(/Simulated/)).toBeChecked(); // the current declaration is preselected
  await expect(ap.locator("#sim-note")).toHaveValue(NOTE);
  await ap.getByLabel(/Unknown/).check();
  await ap.fill("#sim-note", "");
  await ap.click("#sim-ok");
  await expect(ap.locator("#sim-head")).toHaveText(/^Real or simulated: Declared unknown by tactical@example\.test, [0-9-]+ [0-9:]+$/);
  await ap.goto("/#audit");
  await expect(ap.locator("#audit-body tr", { hasText: f.id }).filter({ hasText: "sim-status" })).toHaveCount(2);

  // Whoever requests it out sees the declaration on the catalog topic.
  await post(request, "member", `/api/feeds/${f.id}/sim-status`, { mode: "simulated", note: "bench run" });
  await ap.goto("/#request?dir=out");
  const pick = ap.getByRole("button", { name: /drone-a position/ });
  await expect(pick.locator(".simline")).toHaveText(/Real or simulated: Declared simulated by member@example\.test, .*: bench run/);
  await actx.close();

  // The open, machine-readable document: no sign-in, the boundary node's shape.
  const r = await request.get("/api/sim-status");
  expect(r.status()).toBe(200);
  const doc = await r.json();
  expect(doc.schema).toBe("goat.sim-status/v1");
  const e = doc.feeds[f.id];
  expect(Object.keys(e).sort()).toEqual(["label", "mode", "note", "set_at", "set_by"]);
  expect(e).toMatchObject({ mode: "simulated", note: "bench run", set_by: "member@example.test", label: "drone-a position" });
  expect(typeof e.set_at).toBe("number");
  expect(Math.abs(e.set_at - Date.now() / 1000)).toBeLessThan(600);
});

for (const width of [390, 1440]) {
  for (const scheme of ["light", "dark"] as const) {
    test(`the declaration form at ${width}px, ${scheme}: no overflow, no junk text`, async ({ browser, request }) => {
      const feeds = (await (await request.get("/api/feeds", { headers: { "X-Forwarded-Email": "member@example.test" } })).json()).feeds;
      const f = feeds.find((x: { label: string }) => x.label === "drone-a position");
      const ctx = await as(browser, "member", { viewport: { width, height: 900 }, colorScheme: scheme });
      const page = await ctx.newPage();
      const w = watch(page);
      await page.goto(`/#feed/${f.id}`);
      await page.locator("#feed-actions").getByRole("button", { name: "Real or simulated" }).click();
      await expect(page.locator("#sim-box")).toBeVisible();
      await noJunkText(page);
      await noHorizontalOverflow(page);
      await page.screenshot({ path: test.info().outputPath(`sim-${width}-${scheme}.png`), fullPage: true });
      w.check();
      await ctx.close();
    });
  }
}
