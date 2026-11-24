import { defineConfig, devices } from "@playwright/test";
import { DEFAULT_VIEWPORT } from "./e2e/support/app";
import { inReferenceImage } from "./e2e/support/reference-image";

/**
 * Playwright config for the deckwatch e2e suite. See docs/TESTING.md.
 *
 * Two kinds of specs share one set of API mocks (e2e/support):
 * - functional specs (e2e/*.spec.ts) drive the UI and assert behaviour and
 *   the API calls it makes;
 * - visual specs (e2e/visual/*.spec.ts) compare screenshots with the
 *   baselines in e2e/__screenshots__. They only run inside the pinned
 *   Playwright Docker image, so fonts and rendering match the baselines:
 *   `pnpm test:e2e:docker` locally, the `e2e` job in CI.
 *
 * The app under test is a production build (`vite build`) served by
 * `vite preview`, with every `/api/**` request mocked in the browser. Point
 * `PLAYWRIGHT_BASE_URL` at a running deckwatch and set
 * `PLAYWRIGHT_MOCK_MODE=off` to drive a real backend instead (functional
 * specs only).
 */

const PORT = 4174;
const baseURL = process.env.PLAYWRIGHT_BASE_URL ?? `http://127.0.0.1:${PORT}`;

const deterministic = {
  viewport: DEFAULT_VIEWPORT,
  deviceScaleFactor: 1,
  locale: "en-US",
  timezoneId: "UTC",
  reducedMotion: "reduce",
  serviceWorkers: "block",
} as const;

export default defineConfig({
  testDir: "./e2e",
  // Baselines are only read and written inside the reference image. A forced
  // run elsewhere (E2E_VISUAL_FORCE=1) compares against throwaway files.
  snapshotPathTemplate:
    inReferenceImage() || process.env.CI
      ? "{testDir}/__screenshots__/{testFileName}/{arg}{ext}"
      : ".e2e-local/screenshots/{testFileName}/{arg}{ext}",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  // One retry in CI shows up as "flaky" in the report instead of hiding it.
  retries: process.env.CI ? 1 : 0,
  // GitHub's ubuntu-latest runners have 4 vCPUs; leave one for vite preview.
  workers: process.env.CI ? 3 : undefined,
  timeout: 60_000,
  reporter: process.env.CI
    ? [["github"], ["list"], ["html", { open: "never" }]]
    : [["list"], ["html", { open: "never" }]],
  expect: {
    timeout: 10_000,
    toHaveScreenshot: {
      // Strict on purpose: the suite exists to show real UI differences.
      // `threshold` is pixelmatch's per-pixel colour distance (0-1); 0.05
      // still flags a 1px shift or a change of a few levels in a colour,
      // while anti-aliasing noise is excluded by pixelmatch itself. No
      // differing pixels are tolerated beyond that.
      threshold: 0.05,
      maxDiffPixels: 0,
      animations: "disabled",
      caret: "hide",
      scale: "css",
    },
  },
  use: {
    baseURL,
    ...deterministic,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium",
      testIgnore: /visual\//,
      use: { ...devices["Desktop Chrome"], ...deterministic },
    },
    {
      name: "visual",
      testMatch: /visual\/.*\.spec\.ts/,
      use: { ...devices["Desktop Chrome"], ...deterministic },
    },
    // Cross-browser runs of the functional specs, opt-in:
    // E2E_ALL_BROWSERS=1 pnpm test:e2e
    ...(process.env.E2E_ALL_BROWSERS
      ? [
          { name: "firefox", testIgnore: /visual\//, use: { ...devices["Desktop Firefox"], ...deterministic } },
          { name: "webkit", testIgnore: /visual\//, use: { ...devices["Desktop Safari"], ...deterministic } },
        ]
      : []),
  ],
  webServer: process.env.PLAYWRIGHT_BASE_URL
    ? undefined
    : {
        // Always test a fresh production build of the current source.
        command: `pnpm exec vite build --logLevel warn && pnpm exec vite preview --host 127.0.0.1 --port ${PORT} --strictPort`,
        url: baseURL,
        reuseExistingServer: !process.env.CI,
        timeout: 180_000,
        stdout: "ignore",
        stderr: "pipe",
      },
});
