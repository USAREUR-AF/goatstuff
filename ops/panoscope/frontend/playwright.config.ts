// SPDX-License-Identifier: Apache-2.0
//
// Playwright config for the panoscope-frontend click-rhythm spec.
//
// Backend (Rust panoscope) is NOT required — the spec mocks `/api/*`
// via page.route() and replaces `window.WebSocket` via an init script
// so the SPA renders against a deterministic canned snapshot. See
// tests/click-rhythm.spec.ts for the fixture shape.
//
// CI uses `vite preview` against the prebuilt dist/ (fast cold start —
// just serves static files; build step runs earlier in the workflow).
// Local dev uses `vite dev` for HMR + faster iteration.

import { defineConfig, devices } from "@playwright/test";

const CI_PORT = 4173;
const DEV_PORT = 5173;
const PORT = process.env.CI ? CI_PORT : DEV_PORT;
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
  webServer: process.env.CI
    ? {
        command: `npx vite preview --host 127.0.0.1 --port ${CI_PORT} --strictPort`,
        url: BASE_URL,
        reuseExistingServer: false,
        timeout: 60_000,
      }
    : {
        command: "npm run dev",
        url: BASE_URL,
        reuseExistingServer: true,
        timeout: 120_000,
      },
});
