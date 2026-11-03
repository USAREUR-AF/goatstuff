// SPDX-License-Identifier: Apache-2.0
//
// E2E spec: every badge-opened reference overlay (LEGEND, KEYS) must be
// dismissable two ways the operator expects —
//   1. clicking its own fleet-bar badge again (toggle off), and
//   2. clicking the main COP canvas (the "back to the COP" gesture).
// Escape already closed them; these two paths are the gap this covers.
//
// Backend mocked (fixtures/backend.ts) so the SPA boots and the
// install*Overlay wiring runs.

import { test, expect } from "@playwright/test";
import { installBackend } from "./fixtures/backend";

const OVERLAYS: { name: string; badge: string; overlay: string }[] = [
  { name: "LEGEND", badge: "#fleet-legend-toggle", overlay: "#legend-overlay" },
  { name: "KEYS", badge: "#fleet-keys-toggle", overlay: "#keys-overlay" },
];

test.describe("panoscope overlay dismissal", () => {
  test.beforeEach(async ({ page }) => {
    await installBackend(page, { peers: [{ id: "acme-alpha", vendor: "acme", publishes: ["acme/svc/temperature"] }] });
    await page.goto("/");
    // Wait for the renderer to exist so the graph + overlays are wired.
    await page.waitForFunction(
      () => !!(window as unknown as { __panoscopeRenderer?: unknown }).__panoscopeRenderer,
      { timeout: 5_000 },
    );
  });

  for (const o of OVERLAYS) {
    test(`${o.name}: badge opens, second badge click dismisses`, async ({ page }) => {
      const overlay = page.locator(o.overlay);
      await expect(overlay).toHaveClass(/hidden/);
      await page.locator(o.badge).click();
      await expect(overlay).not.toHaveClass(/hidden/);
      await page.locator(o.badge).click();
      await expect(overlay).toHaveClass(/hidden/);
    });

    test(`${o.name}: clicking the COP canvas dismisses it`, async ({ page }) => {
      const overlay = page.locator(o.overlay);
      await page.locator(o.badge).click();
      await expect(overlay).not.toHaveClass(/hidden/);
      // Dispatch the click directly on #graph — exercises the capture-
      // phase dismiss handler without pixel hit-testing (the centered
      // LEGEND / right-aligned KEYS popups cover different parts of the
      // canvas, so a fixed pixel can't clear both).
      await page.locator("#graph").dispatchEvent("click");
      await expect(overlay).toHaveClass(/hidden/);
    });
  }
});
