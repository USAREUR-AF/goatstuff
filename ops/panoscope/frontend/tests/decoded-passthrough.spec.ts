// SPDX-License-Identifier: Apache-2.0
//
// E2E spec for JSON/text passthrough in the panoscope decoded view
// (docs/design/panoscope.md §"Decoded view"). A topic with no protobuf
// `.fds` but JSON/text wire bytes (a JSON vendor like vendor-d, or a stale
// recalled JSON sample) now renders in the DECODED tab via passthrough —
// and MUST be unmistakably marked as passthrough, never dressed up as a
// schema-validated protobuf decode.
//
// Contract under test (DECODED mode, single JSON topic):
//   1. Schema badge reads PASSTHROUGH (not "decoded", not "NO SCHEMA").
//   2. A pane-level prose note states the bytes are raw / not protobuf-decoded.
//   3. The sample row carries the cyan `.stream-sample--passthrough` gutter +
//      a `PASSTHROUGH · JSON` tag.
//   4. The JSON body still renders (so the operator sees the data).
//
// Backend mocked by fixtures/backend.ts.

import { test, expect, type Page } from "@playwright/test";
import { installBackend, waitForNodes } from "./fixtures/backend";

const PEER = "vendor-d-feed";
const TOPIC = "vendor-d/feed/test";

async function installMocks(page: Page): Promise<void> {
  await installBackend(page, {
    peers: [{ id: PEER, vendor: "vendor-d", publishes: [TOPIC] }],
    legend: { decoder_enabled: true },
    samples: () => ({ topic: TOPIC, count: 0, entries: [] }),
    // The feature under test: decoded endpoint returns a passthrough outcome
    // for this schemaless JSON topic.
    decoded: () => ({
      topic: TOPIC,
      status: "passthrough",
      samples: [
        {
          ts_ms: Date.now(),
          source_peer: "vendor-d/feed",
          size_bytes: 42,
          result: { passthrough: { format: "json", body: { hello: "vendor-d", n: 7 } } },
          stale: false,
        },
      ],
    }),
  });
}

async function waitForSnapshot(page: Page): Promise<void> {
  await waitForNodes(page, [PEER]);
}

test.describe("panoscope decoded passthrough", () => {
  test("schemaless JSON topic renders in DECODED via clearly-marked passthrough", async ({
    page,
  }) => {
    await installMocks(page);
    await page.goto("/");
    await waitForSnapshot(page);

    // Pivot the stream pane to the single JSON topic.
    await page.evaluate((id) => {
      for (const el of Array.from(document.querySelectorAll("g.node"))) {
        const d = (el as unknown as { __data__?: { id?: string } }).__data__;
        if (d?.id === id) {
          el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
          return;
        }
      }
      throw new Error(`no g.node bound to id=${id}`);
    }, PEER);
    await expect(page.locator("#info-title")).toContainText(PEER);
    await page.locator(".topic-list-item .topic-name").first().click();
    await expect(page.locator("#info-stream-topic-label")).toHaveText(TOPIC);

    // Switch to the DECODED view.
    await page.locator("#info-stream-mode-decoded").click();

    const body = page.locator("#info-stream-body");
    // (1) badge says PASSTHROUGH, not decoded / NO SCHEMA.
    await expect(page.locator("#info-stream-schema-badge")).toHaveText(/PASSTHROUGH/);
    // (2) pane-level prose note makes the not-decoded framing explicit.
    const note = body.locator(".stream-passthrough-note");
    await expect(note).toBeVisible();
    await expect(note).toContainText(/not protobuf-decoded/i);
    // (3) the row is tagged + cyan-guttered as passthrough.
    const row = body.locator(".stream-sample--passthrough");
    await expect(row).toHaveCount(1);
    await expect(row.locator(".stream-sample-passthrough-tag")).toContainText("PASSTHROUGH · JSON");
    // (4) the JSON body still renders.
    await expect(row).toContainText("vendor-d");
  });
});
