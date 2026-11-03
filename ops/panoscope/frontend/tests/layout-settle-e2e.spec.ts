// SPDX-License-Identifier: Apache-2.0
//
// E2E spec for the layout settle behavior (docs/design/panoscope.md
// §"Layout settle"; F-210 follow-on). The pure unit spec (layout-settle.spec.ts)
// locks the signature/diff logic; this one exercises the REAL renderer end to
// end, because the bug had two reheat sources in applyView and the second
// one — refreshSelectionVisuals' unconditional alpha(0.55).restart() — is only
// reachable through the live render path, not the pure helper.
//
// Regression target: a force graph that never settles because it re-heats to a
// high alpha on every view poll, with identical node/edge sets across polls. A
// data-only repaint (same topology) must NOT re-heat the simulation.

import { test, expect } from "@playwright/test";
import { installBackend, refreshView, waitForNodes } from "./fixtures/backend";

type RendererProbe = {
  __panoscopeRenderer?: {
    simulation: { alpha: () => number };
  };
};

test("an identical (data-only) view does NOT re-heat the settled layout", async ({
  page,
}) => {
  await installBackend(page, {
    peers: [
      { id: "acme-alpha", vendor: "acme", publishes: ["acme/svc/temperature"] },
      { id: "acme-beta", vendor: "acme", publishes: ["acme/svc/pressure"] },
    ],
  });
  await page.goto("/");

  // First frame renders + reheats (topology changed: empty → 2 nodes).
  await waitForNodes(page, ["acme-alpha", "acme-beta"]);

  // Let the initial reheat cool to a near-settled alpha.
  await page.waitForFunction(
    () => {
      const r = (window as unknown as RendererProbe).__panoscopeRenderer;
      return !!r && r.simulation.alpha() < 0.05;
    },
    { timeout: 10_000 },
  );

  const alphaBefore = await page.evaluate(
    () => (window as unknown as RendererProbe).__panoscopeRenderer!.simulation.alpha(),
  );

  // Fetch the SAME topology again (the ~1.5s data-only poll; the mock bumps
  // the generation, so the page applies it). Pre-fix this re-heated to
  // ~0.55/0.6; post-fix the sim must stay cool.
  await refreshView(page);

  // Give the apply a moment to run.
  await page.waitForTimeout(300);

  const alphaAfter = await page.evaluate(
    () => (window as unknown as RendererProbe).__panoscopeRenderer!.simulation.alpha(),
  );

  // No reheat: alpha must not jump up. Allow a tiny epsilon for tick jitter;
  // a reheat would set alpha to 0.55/0.6, far above this bound.
  expect(alphaAfter).toBeLessThan(0.1);
  expect(alphaAfter).toBeLessThanOrEqual(alphaBefore + 0.02);
});
