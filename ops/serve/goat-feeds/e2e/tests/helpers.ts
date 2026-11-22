// SPDX-License-Identifier: Apache-2.0
import { expect, type Browser, type BrowserContext, type Page, type APIRequestContext } from "@playwright/test";

export const ROLES = {
  member: { "X-Forwarded-Email": "member@example.test", "X-Forwarded-Groups": "DEVELOPER" },
  tactical: { "X-Forwarded-Email": "tactical@example.test", "X-Forwarded-Groups": "TACTICAL-ADMIN" },
  platform: { "X-Forwarded-Email": "platform@example.test", "X-Forwarded-Groups": "PLATFORM-ADMIN" },
} as const;
export type Role = keyof typeof ROLES;

/** A browser context signed in as a role, the way the sign-in fence would set headers. */
export async function as(browser: Browser, role: Role, opts: Parameters<Browser["newContext"]>[0] = {}): Promise<BrowserContext> {
  return browser.newContext({ ...opts, extraHTTPHeaders: { ...ROLES[role] } });
}

/** Watches a page for console errors and page errors; check() fails the test if any arrived. */
export function watch(page: Page) {
  const problems: string[] = [];
  page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));
  page.on("console", (m) => { if (m.type() === "error") problems.push(`console: ${m.text()}`); });
  return { check: () => expect(problems, problems.join("\n")).toEqual([]) };
}

/** No literal null/undefined/NaN/[object Object] text anywhere a person can read. */
export async function noJunkText(page: Page) {
  const text = await page.locator("body").innerText();
  for (const junk of [/\bnull\b/, /\bundefined\b/, /\bNaN\b/, /\[object Object\]/]) {
    expect(text, `page text contains ${junk}`).not.toMatch(junk);
  }
}

/** The page is no wider than the viewport. */
export async function noHorizontalOverflow(page: Page) {
  const { sw, cw } = await page.evaluate(() => ({ sw: document.documentElement.scrollWidth, cw: document.documentElement.clientWidth }));
  expect(sw, `scrollWidth ${sw} > clientWidth ${cw}`).toBeLessThanOrEqual(cw + 1);
}

export async function post(request: APIRequestContext, role: Role, path: string, body: unknown) {
  const r = await request.post(path, { headers: { ...ROLES[role], "X-Goat-Feeds": "1" }, data: body });
  expect(r.ok(), `${path}: ${r.status()} ${await r.text()}`).toBeTruthy();
  return r.json();
}

let seq = 0;
/** A destination port no other test uses. */
export const freshPort = () => 20000 + (Date.now() % 20000) + (seq++);
