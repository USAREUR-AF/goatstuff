// SPDX-License-Identifier: Apache-2.0
//
// E2E spec for the browser half of the view model
// (docs/design/panoscope-view-model.md). The server decides grouping; the
// page draws what it is given and turns gestures into view parameters.
//
//   1. A group draws as ONE node with an edge from its publisher; its member
//      topics are not drawn, and other publishers' topics are.
//   2. Double-clicking a group asks for a view with its id in `expand` and
//      draws the members.
//   3. A topic inside a group is findable by any substring (the server
//      searches), and committing it opens its group and selects its pin.
//   4. When the server says it simplified the view to fit its element cap,
//      the page says so.
//   5. View parameters survive a reload, and a viewer's pre-view-model window
//      and SIMPLIFY choices carry over.
//   6. SIMPLIFY sends `detail: "simplified"`.
//   7. Before the first answer the page says it is waiting, never that the
//      fabric is empty.
//   8. Component isolation draws only what the selection connects to.
//
// Backend mocked by fixtures/backend.ts.

import { test, expect, type Page } from "@playwright/test";
import {
  installBackend,
  nodeEvent,
  nodeIds,
  refreshView,
  selectedIds,
  waitForNodes,
  waitForNoNodes,
  type Backend,
  type FxPeer,
} from "./fixtures/backend";

const BIG_PUB = "video-streamer";
const CHUNKS = Array.from({ length: 200 }, (_, i) => `vid/v1/stream/s1/chunk/${String(i).padStart(4, "0")}`);
const CHUNK_GROUP = "grp:video-streamer:fam:0000000000000c0c:vid/v1/stream/s1/chunk";
const N_SMALL = 5;

const PEERS: FxPeer[] = [
  { id: BIG_PUB, vendor: "vid", publishes: [...CHUNKS, "vid/v1/stream/s1/meta"] },
  ...Array.from({ length: N_SMALL }, (_, i) => ({ id: `svc-${i}`, vendor: "svc", publishes: [`svc-${i}/status`, `svc-${i}/health`] })),
];

async function setup(page: Page): Promise<Backend> {
  const backend = await installBackend(page, {
    peers: PEERS,
    groups: [
      {
        id: CHUNK_GROUP,
        kind: "family",
        publisher: BIG_PUB,
        label: "vid/v1/stream/s1/chunk/*",
        selector: "vid/v1/stream/s1/chunk/*",
        members: CHUNKS,
      },
    ],
  });
  await page.goto("/");
  await waitForNodes(page, [BIG_PUB, CHUNK_GROUP]);
  await page.evaluate(() => (document.querySelector(".welcome-dismiss") as HTMLElement | null)?.click());
  return backend;
}

test.describe("panoscope view model", () => {
  test("a group is one node; its members are not drawn and other topics are", async ({ page }) => {
    await setup(page);
    const ids = await nodeIds(page);
    expect(ids.filter((id) => id.startsWith("topic:vid/v1/stream/s1/chunk/"))).toHaveLength(0);
    expect(ids).toContain("topic:vid/v1/stream/s1/meta");
    expect(ids.filter((id) => /^topic:svc-\d\//.test(id))).toHaveLength(N_SMALL * 2);
    // The hover badge carries the member count.
    const badge = await page.evaluate((gid) => {
      for (const el of Array.from(document.querySelectorAll("g.node"))) {
        const d = (el as unknown as { __data__?: { id?: string } }).__data__;
        if (d?.id === gid) return el.querySelector("text.peek-halo-count")?.textContent ?? null;
      }
      return null;
    }, CHUNK_GROUP);
    expect(badge).toBe(String(CHUNKS.length));
  });

  test("double-clicking a group opens it on the canvas", async ({ page }) => {
    const backend = await setup(page);
    await nodeEvent(page, CHUNK_GROUP, "dblclick");
    await waitForNodes(page, CHUNKS.map((c) => `topic:${c}`));
    await waitForNoNodes(page, [CHUNK_GROUP]);
    expect(backend.lastParams()?.expand).toEqual([CHUNK_GROUP]);
  });

  test("a grouped topic is found by a middle substring and revealed on commit", async ({ page }) => {
    const backend = await setup(page);
    const target = CHUNKS[137];
    const needle = "chunk/0137";
    expect(target.startsWith(needle)).toBe(false);
    await page.keyboard.press("/");
    await expect(page.locator("#search-overlay")).not.toHaveClass(/hidden/);
    await page.locator("#search-input").fill(needle);
    await expect(page.locator(".search-overlay-match").first()).toContainText(target);
    await page.keyboard.press("Enter");
    await waitForNodes(page, [`topic:${target}`]);
    await expect.poll(() => selectedIds(page)).toEqual([`topic:${target}`]);
    expect(backend.lastParams()?.expand).toEqual([CHUNK_GROUP]);
  });

  test("a view simplified to fit the element cap says so", async ({ page }) => {
    const backend = await setup(page);
    await expect(page.locator("#view-truncated")).toHaveCount(0);
    backend.overrideView((_p, built) => ({
      ...built,
      generation: built.generation + 1000,
      truncated: { max_elements: 5000, publisher_budget: 8, over_cap: false, opened_ignored: true },
    }));
    await refreshView(page);
    const note = page.locator("#view-truncated");
    await expect(note).toBeVisible();
    await expect(note).toContainText("at most 8 per publisher");
    await expect(note).toContainText("Opened groups are closed");
    backend.overrideView(null);
    await refreshView(page);
    await expect(note).toBeHidden();
  });

  test("view parameters survive a reload", async ({ page }) => {
    const backend = await setup(page);
    await nodeEvent(page, CHUNK_GROUP, "dblclick");
    await waitForNoNodes(page, [CHUNK_GROUP]);
    await page.locator("#fleet-window-select").selectOption("24");
    await expect.poll(() => backend.lastParams()?.window_hours).toBe(24);
    await page.reload();
    await waitForNodes(page, [`topic:${CHUNKS[0]}`]);
    expect(backend.lastParams()?.expand).toEqual([CHUNK_GROUP]);
    expect(backend.lastParams()?.window_hours).toBe(24);
    await expect(page.locator("#fleet-window-select")).toHaveValue("24");
  });

  test("a returning viewer's window and SIMPLIFY choices carry over", async ({ page }) => {
    await page.addInitScript(() => {
      if (!localStorage.getItem("panoscope.viewParams.v1")) {
        localStorage.setItem("panoscope.historyWindowHours", "48");
        localStorage.setItem("panoscope.aggressiveView", "1");
      }
    });
    const backend = await installBackend(page, { peers: PEERS });
    await page.goto("/");
    await expect.poll(() => backend.lastParams()?.detail).toBe("simplified");
    expect(backend.lastParams()?.window_hours).toBe(48);
    await expect(page.locator("#fleet-aggressive-toggle")).toHaveAttribute("aria-pressed", "true");
  });

  test("a live publish animates on the element drawing that topic", async ({ page }) => {
    // Packet dots are the "are bytes moving?" signal. A topic inside a group
    // has no pin of its own, so the dot has to attribute to the group whose
    // selector covers the key; a drawn pin takes its own dot. Each dot is
    // read in the same task that pushes the event, because the animation
    // removes it about a second later.
    await setup(page);
    const pushAndCount = (peer: string, topic: string) =>
      page.evaluate(
        ({ peer, topic }) => {
          const before = document.querySelectorAll("circle.packet-dot").length;
          (window as unknown as { __mockWs: { push: (o: unknown) => void } }).__mockWs.push({
            type: "delta",
            events: [{ kind: "publish_sampled", peer_id: peer, topic, ts_ms: Date.now(), ema_eps: 1 }],
          });
          return document.querySelectorAll("circle.packet-dot").length - before;
        },
        { peer, topic },
      );

    expect(await pushAndCount(BIG_PUB, CHUNKS[7])).toBe(1); // grouped → its group's edge
    expect(await pushAndCount("svc-0", "svc-0/status")).toBe(1); // drawn pin → its own edge
    expect(await pushAndCount(BIG_PUB, "someone/else/v1")).toBe(0); // not this publisher's
  });

  test("SIMPLIFY draws every publisher as one group", async ({ page }) => {
    const backend = await setup(page);
    await page.locator("#fleet-aggressive-toggle").click();
    await waitForNodes(page, [`grp:${BIG_PUB}:pub`, "grp:svc-0:pub"]);
    await waitForNoNodes(page, ["topic:svc-0/status"]);
    expect(backend.lastParams()?.detail).toBe("simplified");
  });
});

test.describe("panoscope first paint", () => {
  test("says it is waiting, not that the fabric is empty, before the first answer", async ({ page }) => {
    // The zero state and the unknown state look identical on a cold load, and
    // the card used to assert the fabric was empty while nothing had answered
    // yet — on a big fabric that lie stayed on screen for seconds.
    let release: (() => void) | null = null;
    const held = new Promise<void>((r) => (release = r));
    await installBackend(page, { peers: PEERS });
    await page.route("**/api/view", async (route) => {
      await held;
      await route.fallback();
    });
    await page.goto("/");
    const card = page.locator(".welcome-card");
    await expect(card).toContainText("Waiting for the first answer");
    await expect(card).not.toContainText("No data has arrived yet");
    await expect(page.locator("#fleet-peers")).toHaveText("—");
    await expect(page.locator("#fleet-topics")).toHaveText("—");

    release!();
    await expect.poll(() => page.locator("#fleet-peers").textContent()).not.toBe("—");
    await expect(card).not.toContainText("Waiting for the first answer");
  });
});

test.describe("panoscope component isolation", () => {
  const ISOLATED: FxPeer[] = [
    // Two components: a publisher its subscriber hears, and an unrelated pair.
    { id: "alpha-pub", vendor: "a", publishes: ["a/one/v1"] },
    { id: "alpha-sub", vendor: "a", publishes: [], subscribes: ["a/one/v1"] },
    { id: "beta-pub", vendor: "b", publishes: ["b/two/v1"] },
    { id: "beta-sub", vendor: "b", publishes: [], subscribes: ["b/two/v1"] },
  ];

  test("isolates the selected element's connected component and leaves on esc", async ({ page }) => {
    await installBackend(page, { peers: ISOLATED });
    await page.goto("/");
    await waitForNodes(page, ["alpha-pub", "alpha-sub", "beta-pub", "beta-sub", "topic:a/one/v1", "topic:b/two/v1"]);
    await page.evaluate(() => (document.querySelector(".welcome-dismiss") as HTMLElement | null)?.click());

    await nodeEvent(page, "alpha-pub");
    await page.keyboard.press("f");

    // The publisher's component: itself, its topic pin, and the subscriber.
    await expect.poll(() => nodeIds(page)).toEqual(
      expect.arrayContaining(["alpha-pub", "topic:a/one/v1", "alpha-sub"]),
    );
    const ids = await nodeIds(page);
    expect(ids).not.toContain("beta-pub");
    expect(ids).not.toContain("beta-sub");
    expect(ids).toHaveLength(3);
    // Everything shown is selected, so the panel and labels follow.
    expect((await selectedIds(page)).sort()).toEqual(["alpha-pub", "alpha-sub", "topic:a/one/v1"]);
    const banner = page.locator("#focus-banner");
    await expect(banner).toBeVisible();
    await expect(banner).toContainText("3 shown, 3 hidden");

    // It survives a refresh of the view — the lens is re-applied to new data.
    await refreshView(page);
    await expect.poll(() => nodeIds(page).then((x) => x.length)).toBe(3);

    await page.keyboard.press("Escape");
    await expect.poll(() => nodeIds(page).then((x) => x.length)).toBe(6);
    await expect(banner).toHaveCount(0);
  });
});
