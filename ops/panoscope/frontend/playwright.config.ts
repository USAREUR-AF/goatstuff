// SPDX-License-Identifier: Apache-2.0
//
// Playwright config for the panoscope-frontend click-rhythm spec.
//
// Backend (Rust panoscope) is NOT required — the spec mocks `/api/*`
// via page.route() and replaces `window.WebSocket` via an init script
// so the pages render against a deterministic canned snapshot. See
// tests/click-rhythm.spec.ts for the fixture shape.
//
// The pages in web/ are served as files by tests/serve.mjs, which maps
// paths the same way the Rust binary does (src/embed.rs). There is no
// build step.

import { defineConfig, devices } from "@playwright/test";

const PORT = 4173;
const BASE_URL = `http://127.0.0.1:${PORT}`;

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: process.env.CI ? 2 : undefined,
  reporter: "list",
  use: {
    baseURL: BASE_URL,
    trace: "retain-on-failure",
  },
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
  ],
  webServer: {
    command: `node tests/serve.mjs ${PORT}`,
    url: BASE_URL,
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  },
});
