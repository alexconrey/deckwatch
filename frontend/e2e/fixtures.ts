import { test as base, expect, type Route } from "@playwright/test";
import { App, THEME_NAMES, type Theme } from "./support/app";
import { FIXED_NOW, createMockState, type MockState } from "./support/data";
import { MockApi } from "./support/mock-api";
import { installDefaultRoutes } from "./support/routes";

/**
 * Extended Playwright test used by every spec (import `test` and `expect`
 * from here, not from @playwright/test).
 *
 * Each test gets:
 * - `mockApi`: every `/api/**` request answered from `mockState`
 *   (see support/routes.ts). Override one with `mockApi.on(method, path, ...)`.
 *   An unmocked API request, a request to another origin, or an uncaught page
 *   error fails the test.
 * - a browser clock frozen at FIXED_NOW (timers still run), a fixed theme,
 *   and CSS transitions/animations switched off.
 * - `app`: navigation, settle and screenshot helpers (support/app.ts).
 *
 * `PLAYWRIGHT_MOCK_MODE=off` disables the mocks for runs against a real
 * backend; the visual specs skip themselves in that mode.
 */

export interface Fixtures {
  /** Kept for older specs. */
  namespace: string;
  /** Kept for older specs. */
  deploymentName: string;
  /** Vuetify theme the app starts in. */
  theme: Theme;
  mockState: MockState;
  mockApi: MockApi;
  app: App;
}

export const useMocks = process.env.PLAYWRIGHT_MOCK_MODE !== "off";

const NO_MOTION_CSS = `
*, *::before, *::after {
  transition: none !important;
  animation: none !important;
  caret-color: transparent !important;
}
html { scroll-behavior: auto !important; }
`;

export const test = base.extend<Fixtures>({
  namespace: ["default", { option: true }],
  deploymentName: ["web", { option: true }],
  theme: ["dark", { option: true }],

  colorScheme: async ({ theme }, use) => {
    await use(theme);
  },

  page: async ({ page, theme }, use) => {
    await page.clock.setFixedTime(FIXED_NOW);
    // The app reads its theme from localStorage, falling back to
    // prefers-color-scheme. Seed it only once so a theme toggle survives a
    // reload.
    await page.addInitScript((themeName: string) => {
      if (localStorage.getItem("deckwatch-theme") === null) {
        localStorage.setItem("deckwatch-theme", themeName);
      }
    }, THEME_NAMES[theme]);
    await page.addInitScript((css: string) => {
      const install = () => {
        const style = document.createElement("style");
        style.dataset.e2e = "no-motion";
        style.textContent = css;
        (document.head ?? document.documentElement).appendChild(style);
      };
      if (document.documentElement) install();
      else document.addEventListener("readystatechange", install, { once: true });
    }, NO_MOTION_CSS);
    await use(page);
  },

  mockState: async ({}, use) => {
    await use(createMockState());
  },

  mockApi: async ({ page, mockState, baseURL }, use) => {
    const api = new MockApi(page, new URL(baseURL ?? "http://127.0.0.1").origin);
    if (useMocks) {
      installDefaultRoutes(api, mockState);
      await api.install();
    }
    await use(api);
  },

  app: [
    async ({ page, mockApi, theme }, use, testInfo) => {
      const app = new App(page, mockApi, theme);
      await use(app);
      // Only report these when the test itself passed; otherwise they would
      // hide the original failure.
      if (testInfo.status !== testInfo.expectedStatus) return;
      if (!mockApi.allowUnhandled && mockApi.unhandled.length > 0) {
        throw new Error(
          `Unmocked API request(s); add a handler in e2e/support/routes.ts or the test:\n${mockApi.unhandled
            .map((c) => `  ${c.method} /api${c.path}${c.query.size ? `?${c.query}` : ""}`)
            .join("\n")}`,
        );
      }
      if (mockApi.external.length > 0) {
        throw new Error(`Request(s) to another origin:\n  ${mockApi.external.join("\n  ")}`);
      }
      if (!app.allowPageErrors && app.pageErrors.length > 0) {
        throw new Error(`Uncaught page error(s):\n  ${app.pageErrors.map((e) => e.stack ?? e.message).join("\n  ")}`);
      }
    },
    { auto: true },
  ],
});

export { expect };
export type { Route };
