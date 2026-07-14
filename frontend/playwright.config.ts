import { defineConfig, devices } from "@playwright/test";

/**
 * Playwright config for deckwatch e2e tests.
 *
 * The tests mock all `/api/**` traffic by default (see `e2e/fixtures.ts`)
 * so they can run against a plain `vite` dev server with no backend or
 * cluster attached. Point `PLAYWRIGHT_BASE_URL` at a real deployment to
 * bypass the mocks and drive a live cluster.
 *
 * Run:
 *   pnpm test:e2e             # headless, all browsers
 *   pnpm test:e2e:ui          # interactive UI mode
 *   PLAYWRIGHT_BASE_URL=https://deckwatch.example.com pnpm test:e2e
 */
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 2 : undefined,
  reporter: process.env.CI
    ? [["github"], ["html", { open: "never" }]]
    : [["list"], ["html", { open: "never" }]],
  use: {
    baseURL: process.env.PLAYWRIGHT_BASE_URL ?? "http://localhost:3000",
    trace: "on-first-retry",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
  },
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
    { name: "firefox", use: { ...devices["Desktop Firefox"] } },
    { name: "webkit", use: { ...devices["Desktop Safari"] } },
  ],
  webServer: process.env.PLAYWRIGHT_BASE_URL
    ? undefined
    : {
        command: "pnpm dev",
        url: "http://localhost:3000",
        reuseExistingServer: !process.env.CI,
        timeout: 60_000,
      },
});
