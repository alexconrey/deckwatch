import type { Locator, Page } from "@playwright/test";
import type { App, SnapOptions } from "../support/app";
import { eachTheme, expect, test } from "./visual-test";

/**
 * Before the shot, park the mouse and let click ripples finish. The last
 * click leaves the pointer over a control, and whether Chromium has applied
 * its hover state yet varies from run to run; Vuetify removes a ripple up to
 * ~550ms after the click, on a timer.
 */
async function snap(app: App, name: string, options?: SnapOptions): Promise<void> {
  await app.page.mouse.move(0, 0);
  await expect(app.page.locator(".v-ripple__container")).toHaveCount(0);
  await app.snap(name, options);
}

/**
 * Cluster overview, audit log, docs, agent feedback, 404 and the auth
 * callback error page.
 */

/**
 * The v-field around the select labelled `label`. A v-select's <input> sits
 * under the selection text, so clicks must go to the field.
 */
const field = (scope: Locator | Page, label: string) => {
  const page = "page" in scope ? scope.page() : scope;
  return scope.locator(".v-field").filter({ has: page.getByRole("combobox", { name: label, exact: true }) });
};

const nodeRow = (page: Page, name: string) => page.locator(".v-data-table").getByRole("row").filter({ hasText: name });

eachTheme(() => {
  test("cluster overview", async ({ app, page }) => {
    await app.goto("/cluster");
    // Two node-metrics polls, so the sparklines are lines rather than a ramp.
    await app.runTimers(10_000);
    await expect(nodeRow(page, "ip-10-0-1-21")).toBeVisible();
    await snap(app, "cluster/overview", { fullPage: true });
  });

  test("cluster node row expanded", async ({ app, page }) => {
    await app.goto("/cluster");
    await app.runTimers(10_000);
    // The first button in a row is the show-expand toggle.
    await nodeRow(page, "ip-10-0-1-21").getByRole("button").first().click();
    await expect(page.getByText("kubelet is posting ready status (us-east-1a)")).toBeVisible();
    await snap(app, "cluster/node-expanded", { fullPage: true });
  });

  test("cluster events showing all types", async ({ app, page }) => {
    await app.goto("/cluster");
    await app.runTimers(10_000);
    await page.getByRole("button", { name: "All", exact: true }).click();
    await expect(page.getByText("Scaled up replica set web-6c9f7d8b5 to 3")).toBeVisible();
    await snap(app, "cluster/events-all", { fullPage: true });
  });

  test("audit log", async ({ app }) => {
    await app.goto("/audit");
    await snap(app, "audit/table", { fullPage: true });
  });

  test("audit log resource type filter", async ({ app, page }) => {
    await app.goto("/audit");
    await field(page.locator("main"), "Resource Type").click();
    await expect(page.getByRole("option", { name: "deployment" })).toBeVisible();
    await snap(app, "audit/resource-type-menu");
  });

  test("audit log items per page", async ({ app, page }) => {
    await app.goto("/audit");
    await field(page.locator("main"), "Items per page:").click();
    await expect(page.getByRole("option", { name: "100" })).toBeVisible();
    await snap(app, "audit/items-per-page-menu");
  });

  test("docs page", async ({ app, page }) => {
    await app.goto("/docs/getting-started");
    await expect(page.getByRole("heading", { name: "Install" })).toBeVisible();
    await snap(app, "docs/getting-started", { fullPage: true });
  });

  test("agent feedback", async ({ app }) => {
    await app.goto("/agent-feedback");
    await snap(app, "agent-feedback/list", { fullPage: true });
  });

  test("agent feedback with a detail expanded", async ({ app, page }) => {
    await app.goto("/agent-feedback");
    await page.getByRole("button", { name: "Show detail" }).first().click();
    await expect(page.getByText("get_hpa_events")).toBeVisible();
    await snap(app, "agent-feedback/detail-expanded", { fullPage: true });
  });

  test("not found page", async ({ app, page }) => {
    await app.goto("/this/route/does/not/exist");
    await expect(page.getByRole("heading", { name: "Page Not Found" })).toBeVisible();
    await snap(app, "not-found", { fullPage: true });
  });

  test("auth callback error", async ({ app, page }) => {
    await app.goto("/auth/callback");
    await expect(page.getByRole("heading", { name: "Sign-in failed" })).toBeVisible();
    await snap(app, "auth-callback/error", { fullPage: true });
  });
});

for (const width of [1200, 900]) {
  test.describe(`cluster overview at ${width}px`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("cluster overview", async ({ app }) => {
      await app.goto("/cluster");
      await app.runTimers(10_000);
      await snap(app, "cluster/overview", { fullPage: true });
    });
  });
}
