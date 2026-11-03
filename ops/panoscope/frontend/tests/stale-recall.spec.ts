// SPDX-License-Identifier: Apache-2.0
//
// E2E spec for panoscope-stale-recall Layer A — the staleness UX on the
// decoded/raw stream pane (docs/design/panoscope.md §"Stale-recall in the
// decoded stream").
//
// Contract under test:
//   1. When /api/samples returns a sample flagged `stale: true` (the B1
//      on-demand Zenoh-storage recall, returned when panoscope's in-RAM
//      ring is empty for a quiet topic), the stream pane carries `.stale`,
//      the recalled row carries `.stream-sample--stale`, and a
//      "STALE · recalled from storage · last seen Ns ago" banner is
//      prepended — so the operator sees the last-known decode without it
//      ever reading as live.
//   2. When /api/samples returns a genuinely empty list (no live samples
//      AND nothing retained in storage), the pane keeps today's
//      "[no samples yet — waiting…]" empty state and is NOT marked stale.
//
// Backend mocked by fixtures/backend.ts.
// Lives in its own file because it drives the stream pane to a specific
// per-topic mock response, which the other fixtures don't exercise.

import { test, expect, type Page } from "@playwright/test";
import { installBackend, waitForNodes } from "./fixtures/backend";

const PEER = "drs-radar";
const TOPIC = "org-1/radar1radar/RADAR-EXAMPLE/tracks";

// A sample "recalled from storage": flagged stale, with an original publish
// timestamp well in the past so the "last seen Ns ago" age is unambiguous.
function staleEntry() {
  return {
    ts_ms: Date.now() - 5 * 60_000, // 5 min ago
    topic: TOPIC,
    source_peer: PEER,
    format: "json",
    size_bytes: 18,
    truncated: false,
    preview: '{\n  "track_id": 7\n}',
    stale: true,
  };
}

/** Mock the backend. `samplesEntries` is what /api/samples returns for the
 *  topic — a stale entry to exercise the recall UX, or [] for the
 *  genuine-empty case. Must be called before `goto`. */
async function installMocks(page: Page, samplesEntries: unknown[]): Promise<void> {
  await installBackend(page, {
    peers: [{ id: PEER, vendor: "drs", publishes: [TOPIC] }],
    samples: () => ({ topic: TOPIC, count: samplesEntries.length, entries: samplesEntries }),
    decoded: () => ({ topic: TOPIC, status: "portal_disabled", samples: [] }),
  });
}

async function waitForSnapshot(page: Page): Promise<void> {
  await waitForNodes(page, [PEER]);
}

/** Click the canvas node bound to `peerId`, then the topic row in the
 *  side-panel publishes list, pivoting the (raw, default-mode) stream pane
 *  to that single topic. */
async function pivotStreamToTopic(page: Page): Promise<void> {
  await page.evaluate((id) => {
    const nodes = document.querySelectorAll("g.node");
    for (const el of Array.from(nodes)) {
      const d = (el as unknown as { __data__?: { id?: string } }).__data__;
      if (d?.id === id) {
        el.dispatchEvent(
          new MouseEvent("click", { bubbles: true, cancelable: true }),
        );
        return;
      }
    }
    throw new Error(`no g.node bound to id=${id}`);
  }, PEER);
  await expect(page.locator("#info-title")).toContainText(PEER);
  await page.locator(".topic-list-item .topic-name").first().click();
  await expect(page.locator("#info-stream-topic-label")).toHaveText(TOPIC);
}

test.describe("panoscope stale-recall (Layer A)", () => {
  test("storage-recalled stale sample marks the pane stale with a last-seen banner", async ({
    page,
  }) => {
    await installMocks(page, [staleEntry()]);
    await page.goto("/");
    await waitForSnapshot(page);
    await pivotStreamToTopic(page);

    const body = page.locator("#info-stream-body");
    // The recalled row renders (not the empty-state text)...
    await expect(body.locator(".stream-sample--stale")).toHaveCount(1);
    // ...the pane is marked stale...
    await expect(body).toHaveClass(/\bstale\b/);
    // ...and the banner names it a storage recall with an age.
    const banner = body.locator(".stream-stale-banner");
    await expect(banner).toBeVisible();
    await expect(banner).toContainText("recalled from storage");
    await expect(banner).toContainText("last seen");
  });

  test("genuinely empty topic keeps the no-samples empty state, not stale", async ({
    page,
  }) => {
    await installMocks(page, []);
    await page.goto("/");
    await waitForSnapshot(page);
    await pivotStreamToTopic(page);

    const body = page.locator("#info-stream-body");
    await expect(body).toContainText("no samples yet");
    await expect(body.locator(".stream-stale-banner")).toHaveCount(0);
    await expect(body).not.toHaveClass(/\bstale\b/);
  });
});
