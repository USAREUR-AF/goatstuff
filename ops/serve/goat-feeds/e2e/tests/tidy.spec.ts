// SPDX-License-Identifier: Apache-2.0
import { test, expect, type Page } from "@playwright/test";
import { as, watch, noJunkText, noHorizontalOverflow, post, freshPort, ROLES } from "./helpers";

// Naming and grouping at the Fieldkit stage, and the Tidy up page that hands clean-up
// back to the tactical admin: declare a new system with the request, place a member's
// proposal, give a boundary node its far side, archive a system and bring it back.

async function toDestStep(page: Page, topic: RegExp, name: string, ip: string) {
  await page.goto("/#request?dir=out");
  await page.getByRole("button", { name: topic }).click();
  await page.click("#step-next");
  await page.fill("#dest-name", name);
  await page.fill("#dest-ip", ip);
  await page.click("#step-next");
}

test("a member declares a new system with the request, and an admin places it from Tidy up", async ({ browser }) => {
  const ctx = await as(browser, "member");
  const page = await ctx.newPage();
  const w = watch(page);
  await expect(async () => { await page.goto("/#feeds"); await expect(page.locator("#who")).toContainText("member"); }).toPass();
  await expect(page.locator("#nav-tidy")).toBeHidden();
  await toDestStep(page, /Fused air picture \(simulated\)/, "new receiver", "198.51.100.120");
  // Not in the estate: the form asks for site, kind, function and a machine name.
  const panel = page.locator("#declare");
  await expect(panel).toContainText("Declare this system");
  await expect(panel).toContainText("an admin places it");
  await page.click("#step-next");
  await expect(page.getByRole("alert")).toContainText("Choose the site");
  await page.selectOption("#decl-site", "alpha");
  await page.selectOption("#decl-class", "console");
  await page.fill("#decl-role", "cue");
  // A name that breaks the rule shows what is wrong, and the form will not go on.
  await page.fill("#decl-name", "Receiver 1");
  await expect(page.locator("#decl-check")).toContainText("lower case");
  await page.click("#step-next");
  await expect(page.getByRole("alert")).toContainText("naming rule");
  await page.fill("#decl-name", "console-alpha-cue-01");
  await expect(page.locator("#decl-check")).toContainText("Follows the naming rule");
  await page.screenshot({ path: test.info().outputPath("declare-this-system.png"), fullPage: true });
  await page.click("#step-next");
  await page.fill("#port", String(freshPort()));
  await page.click("#step-next");
  await expect(page.locator("#review-system")).toContainText("New system console-alpha-cue-01 at Alpha");
  await page.fill("#purpose", "a new cue console");
  await page.click("#step-next");
  await expect(page.locator("#feed-head .sig.int .n")).toHaveText(/Starting|Process running/);
  await expect(page.locator("#feed-details")).toContainText("Not in the estate. An admin places it in Tidy up.");
  w.check();
  await ctx.close();

  // The tactical admin sees the proposal, with a count on the nav, and places it.
  const actx = await as(browser, "tactical");
  const ap = await actx.newPage();
  const aw = watch(ap);
  await ap.goto("/#tidy");
  await expect(ap.locator("#tidy-count")).toBeVisible();
  await ap.screenshot({ path: test.info().outputPath("tidy-up.png"), fullPage: true });
  const card = ap.locator(".card.tidy[data-kind=proposed]", { hasText: "console-alpha-cue-01" });
  await expect(card).toContainText("member@example.test asked to declare console-alpha-cue-01 at Alpha");
  await card.locator("[data-act=place]").click();
  await expect(card.locator(".pl-name")).toHaveValue("console-alpha-cue-01");
  await card.locator(".pl-submit").click();
  await expect(ap.locator("#tidy-result")).toContainText("Placed console-alpha-cue-01 at Alpha");
  await expect(ap.locator(".card.tidy[data-kind=proposed]", { hasText: "console-alpha-cue-01" })).toHaveCount(0);
  // The feed now knows its receiving system, and who placed it.
  await ap.goto("/#feeds?filter=all&q=new%20receiver");
  await ap.reload();
  await expect(ap.locator(".card.feed-row", { hasText: "new receiver" }).first()).toContainText("placed by tactical@example.test");
  // The audit log carries the proposal and the placement.
  await ap.goto("/#audit");
  await expect(ap.locator("#audit-body")).toContainText("system:console-alpha-cue-01");
  aw.check();
  await actx.close();
});

test("a boundary node is never shown without where the data goes", async ({ browser }) => {
  const ctx = await as(browser, "tactical");
  const page = await ctx.newPage();
  const w = watch(page);
  // Declaring a boundary node with the request needs its far side.
  await toDestStep(page, /Radar tracks \(raw\)/, "partner gateway", "198.51.100.150");
  await page.selectOption("#decl-site", "bravo");
  await page.selectOption("#decl-class", "boundary");
  await page.fill("#decl-role", "relay");
  await page.fill("#decl-label", "Partner gateway");
  await page.fill("#decl-name", "boundary-bravo-relay-01");
  await page.click("#step-next");
  await expect(page.getByRole("alert")).toContainText("far side");
  await page.fill("#decl-far", "Partner network");
  await page.click("#step-next");
  await page.fill("#port", String(freshPort()));
  await page.click("#step-next");
  await expect(page.locator("#review-system")).toContainText("To Partner network through boundary node Partner gateway");
  await page.fill("#purpose", "radar out to the partner network");
  await page.click("#step-next");
  await expect(page.locator("#feed-head .boundary")).toHaveText("To Partner network through boundary node Partner gateway");
  await expect(page.locator("#feed-details")).toContainText("To Partner network through boundary node Partner gateway");
  // The list row and the flow node say the same.
  await page.goto("/#feeds?filter=all&q=partner%20gateway");
  await page.reload();
  await expect(page.locator(".card.feed-row", { hasText: "partner gateway" }).first()).toContainText("Bravo · To Partner network through boundary node Partner gateway");
  await page.goto("/#feeds?filter=all&q=partner%20gateway&view=flow");
  await page.reload();
  const node = page.locator(".fnode.c2", { hasText: "To Partner network" });
  await expect(node).toHaveCount(1);
  await expect(node).toContainText("through boundary node Partner gateway");
  const text = await page.locator("body").innerText();
  expect(text).not.toMatch(/(^|[^h] )boundary node\s*$/m);

  // A boundary node in the estate without a far side raises attention and is listed in Tidy up.
  await post(page.request, "tactical", "/api/feeds", { direction: "out", topic_id: "radar", output_id: "cat062", dest_name: "relay 140",
    dest_ip: "198.51.100.140", port: freshPort(), purpose: "to the undeclared far side" });
  await page.goto("/#feeds?filter=attention&q=relay%20140");
  await page.reload();
  await expect(page.locator(".card.feed-row", { hasText: "relay 140" }).first()).toContainText("boundary node: where the data goes is not declared");
  await page.goto("/#tidy");
  const card = page.locator(".card.tidy[data-kind=boundary-far-side]", { hasText: "boundary-alpha-relay-01" });
  await expect(card).toBeVisible();
  await card.locator("[data-act=place]").click();
  await card.locator(".pl-far").fill("Allied network");
  await card.locator(".pl-submit").click();
  await expect(page.locator("#tidy-result")).toContainText("To Allied network through boundary node boundary-alpha-relay-01");
  await expect(page.locator(".card.tidy[data-kind=boundary-far-side]", { hasText: "boundary-alpha-relay-01" })).toHaveCount(0);
  await noJunkText(page);
  w.check();
  await ctx.close();
});

test("an admin archives a system with care and brings it back", async ({ browser }) => {
  const ctx = await as(browser, "tactical", { viewport: { width: 390, height: 900 } });
  const page = await ctx.newPage();
  const w = watch(page);
  const feed = await post(page.request, "tactical", "/api/feeds", { direction: "out", topic_id: "fused", output_id: "cat062", dest_name: "watch console",
    dest_ip: "198.51.100.130", port: freshPort(), purpose: "picture to the watch console" });
  await page.goto("/#tidy");
  await page.selectOption("#tidy-archive-pick", "console-alpha-watch-01");
  const form = page.locator("#tidy-archive-box [data-form=archive]");
  await expect(form).toContainText("watch console");
  await form.locator(".ar-op").fill("exercise one");
  await form.locator(".ar-returns").selectOption("new-system");
  await form.locator(".ar-rejoin").selectOption("rejoin-needed");
  await form.getByRole("button", { name: "Archive" }).click();
  await expect(page.locator("#tidy-result")).toContainText("console-alpha-watch-01: Archived after exercise one");
  await expect(page.locator("#tidy-result")).toContainText(`Paused 1 requested feed: ${feed.id}`);
  const arch = page.locator(".archived-system[data-system=console-alpha-watch-01]");
  await expect(arch).toContainText("expected to be replaced by a new system; it must rejoin the mesh");
  await expect(arch).toContainText("Switched back on when it returns: Fused air picture → watch console");
  await noJunkText(page);
  await noHorizontalOverflow(page);
  await page.screenshot({ path: test.info().outputPath("tidy-up-archived-390.png"), fullPage: true });

  // Its link reads "archived", never "needs attention", and sits in its own collapsed group.
  await page.goto("/#feeds?filter=attention");
  await page.reload();
  await expect(page.locator(".card.feed-row", { hasText: "watch console" })).toHaveCount(0);
  await page.locator(".chip[data-filter=archived]").click();
  await expect(page.locator(".card.feed-row.archived", { hasText: "watch console" })).toContainText("Archived after exercise one");
  await page.goto("/#feeds?filter=all&group=site");
  await page.reload();
  const g = page.locator("details.group.archived");
  await expect(g.locator(".gkey")).toHaveText("Archived systems");
  await expect(g).not.toHaveAttribute("open", "");
  const sum = await (await page.request.get("/api/feeds/summary?group=state", { headers: ROLES.tactical })).json();
  expect(sum.groups.find((x: { key: string }) => x.key === "archived").archived).toBeGreaterThan(0);

  // Bring it back: unarchived, its paused feed resumed.
  await page.goto("/#tidy");
  await page.locator(".archived-system[data-system=console-alpha-watch-01] [data-act=restore]").click();
  await expect(page.locator("#tidy-result")).toContainText(`Resumed: ${feed.id}`);
  await expect(page.locator(".archived-system[data-system=console-alpha-watch-01]")).toHaveCount(0);
  const back = await (await page.request.get(`/api/feeds/${feed.id}`, { headers: ROLES.tactical })).json();
  expect(back.feed.status).toBe("active");
  expect(back.feed.facets.archived).toBe(false);
  w.check();
  await ctx.close();
});

test("the naming and tidy APIs: rule, check, suggestion, effective estate, admins only", async ({ request }) => {
  const m = ROLES.member, t = ROLES.tactical;
  const n = await (await request.get("/api/naming?name=console-alpha-ops-01&site=alpha&kind=radar&site=alpha&function=watch", { headers: m })).json();
  expect(n.rule).toContain("<kind>-<site>-<function>-<nn>");
  expect(n.check.ok).toBe(true);
  expect(n.suggestion).toBe("radar-alpha-watch-01");
  expect(n.sites.map((s: { id: string }) => s.id)).toEqual(["alpha", "bravo"]);
  const bad = await (await request.get("/api/naming?name=Radar_A", { headers: m })).json();
  expect(bad.check.ok).toBe(false);
  expect((await request.get("/api/tidy", { headers: m })).status()).toBe(403);
  expect((await request.get("/api/estate/effective", { headers: m })).status()).toBe(403);
  const tidy = await (await request.get("/api/tidy", { headers: t })).json();
  expect(tidy.items.some((i: { kind: string; key: string }) => i.kind === "name-rule" && i.key === "radar-a")).toBe(true);
  const eff = await (await request.get("/api/estate/effective", { headers: t })).json();
  const names = eff.sites.flatMap((s: { systems: { name: string }[] }) => s.systems.map((x) => x.name));
  expect(names).toContain("console-alpha-cue-01");
  // The API refuses an undeclared destination without a declaration, and says how to fix it.
  const r = await request.post("/api/feeds", { headers: { ...m, "X-Goat-Feeds": "1" }, data: { direction: "out", topic_id: "sim", output_id: "cat062",
    dest_name: "nobody", dest_ip: "198.51.100.199", port: freshPort(), purpose: "no declaration" } });
  expect(r.status()).toBe(400);
  expect((await r.json()).error).toContain("is not a declared system");
  // A member cannot place, archive or bring back.
  const p = await request.post("/api/systems/console-alpha-ops-21/archive", { headers: { ...m, "X-Goat-Feeds": "1" }, data: {} });
  expect(p.status()).toBe(403);
});
