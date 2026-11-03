// SPDX-License-Identifier: Apache-2.0
//
// E2E spec for the test-data provenance filter (docs/design/panoscope.md
// §"Test-data filter"; ADR 1103 enforcement). A publisher whose every topic
// is loadtest data is hidden from the operational COP by DEFAULT; the TEST
// toolbar toggle shows the test publishers alone. The classification is the
// server's (`is_test_topic`, tested in Rust); this spec covers the page: the
// parameter it sends, the label it shows, and that the choice survives a
// reload.

import { test, expect, type Page } from "@playwright/test";
import { installBackend, nodeIds, type Backend, type FxPeer } from "./fixtures/backend";

const N_LOADTEST = 20;
const N_REAL = 5;

const PEERS: FxPeer[] = [
  ...Array.from({ length: N_LOADTEST }, (_, i) => ({ id: `lt-${i}`, publishes: [`bench/loadtest-${i}/t`], test: true })),
  ...Array.from({ length: N_REAL }, (_, i) => ({ id: `svc-${i}`, publishes: [`svc-${i}/status`] })),
];

async function count(page: Page, prefix: string): Promise<number> {
  return (await nodeIds(page)).filter((id) => id.startsWith(prefix)).length;
}

async function setup(page: Page): Promise<Backend> {
  const backend = await installBackend(page, { peers: PEERS });
  await page.goto("/");
  await expect.poll(() => count(page, "svc-")).toBe(N_REAL);
  return backend;
}

test.describe("panoscope test-data filter", () => {
  test("hides load-test publishers by default; keeps real ones", async ({ page }) => {
    const backend = await setup(page);
    expect(backend.lastParams()?.test).toBe("hide");
    await expect(page.locator("#fleet-test-toggle")).toHaveText("TEST HIDDEN");
    expect(await count(page, "lt-")).toBe(0);
  });

  test("TEST ONLY shows ONLY load-test publishers and survives a reload", async ({ page }) => {
    const backend = await setup(page);
    await page.locator("#fleet-test-toggle").click();
    await expect(page.locator("#fleet-test-toggle")).toHaveText("TEST ONLY");
    await expect.poll(() => count(page, "lt-")).toBe(N_LOADTEST);
    expect(await count(page, "svc-")).toBe(0);
    expect(backend.lastParams()?.test).toBe("only");

    await page.reload();
    await expect(page.locator("#fleet-test-toggle")).toHaveText("TEST ONLY");
    await expect.poll(() => count(page, "lt-")).toBe(N_LOADTEST);
  });
});
