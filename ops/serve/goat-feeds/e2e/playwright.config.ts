// SPDX-License-Identifier: Apache-2.0
//
// Browser tests for goat-feeds. tests/start.mjs runs the real binary against a real Postgres
// (GOAT_FEEDS_E2E_DSN) with a stub pipe that prints counter lines, so every screen is driven
// end to end. Identity comes from headers the sign-in fence would set (X-Forwarded-Email,
// X-Forwarded-Groups), injected per test context.

import { defineConfig, devices } from "@playwright/test";

export const PORT = Number(process.env.GOAT_FEEDS_E2E_PORT || 18199);
const BASE_URL = `http://127.0.0.1:${PORT}`;

export default defineConfig({
  testDir: "./tests",
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: [["list"], ["html", { open: "never" }]],
  timeout: 60_000,
  expect: { timeout: 15_000 },
  use: {
    baseURL: BASE_URL,
    trace: "retain-on-failure",
    screenshot: "on",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: `node tests/start.mjs ${PORT}`,
    url: `${BASE_URL}/healthz`,
    reuseExistingServer: false,
    timeout: 60_000,
    stdout: "pipe",
    stderr: "pipe",
  },
});
