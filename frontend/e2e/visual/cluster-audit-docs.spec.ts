import type { Page } from "@playwright/test";
import { eachTheme, expect, test } from "./visual-test";
import { openMenu } from "../support/ui";

/**
 * Cluster overview, audit log, docs, agent feedback, 404 and the auth
 * callback error page.
 */

const nodeRow = (page: Page, name: string) => page.locator(".v-data-table").getByRole("row").filter({ hasText: name });

eachTheme(() => {
  test("cluster overview", async ({ app, page }) => {
    await app.goto("/cluster");
    // Two node-metrics polls, so the sparklines are lines rather than a ramp.
    await app.runTimers(10_000);
    await expect(nodeRow(page, "ip-10-0-1-21")).toBeVisible();
    await app.snap("cluster/overview", { fullPage: true });
  });

  test("cluster node row expanded", async ({ app, page }) => {
    await app.goto("/cluster");
    await app.runTimers(10_000);
    // The first button in a row is the show-expand toggle.
    await nodeRow(page, "ip-10-0-1-21").getByRole("button").first().click();
    await expect(page.getByText("kubelet is posting ready status (us-east-1a)")).toBeVisible();
    await app.snap("cluster/node-expanded", { fullPage: true });
  });

  test("cluster events showing all types", async ({ app, page }) => {
    await app.goto("/cluster");
    await app.runTimers(10_000);
    await page.getByRole("button", { name: "All", exact: true }).click();
    await expect(page.getByText("Scaled up replica set web-6c9f7d8b5 to 3")).toBeVisible();
    await app.snap("cluster/events-all", { fullPage: true });
  });

  test("audit log", async ({ app }) => {
    await app.goto("/audit");
    await app.snap("audit/table", { fullPage: true });
  });

  test("audit log resource type filter", async ({ app, page }) => {
    await app.goto("/audit");
    await openMenu(page.locator("main"), "Resource Type");
    await expect(page.getByRole("option", { name: "deployment" })).toBeVisible();
    await app.snap("audit/resource-type-menu");
  });

  test("audit log items per page", async ({ app, page }) => {
    await app.goto("/audit");
    await openMenu(page.locator("main"), "Items per page:");
    await expect(page.getByRole("option", { name: "100" })).toBeVisible();
    await app.snap("audit/items-per-page-menu");
  });

  test("docs page", async ({ app, page }) => {
    await app.goto("/docs/getting-started");
    await expect(page.getByRole("heading", { name: "Install" })).toBeVisible();
    await app.snap("docs/getting-started", { fullPage: true });
  });

  test("agent feedback", async ({ app }) => {
    await app.goto("/agent-feedback");
    await app.snap("agent-feedback/list", { fullPage: true });
  });

  test("agent feedback with a detail expanded", async ({ app, page }) => {
    await app.goto("/agent-feedback");
    await page.getByRole("button", { name: "Show detail" }).first().click();
    await expect(page.getByText("get_hpa_events")).toBeVisible();
    await app.snap("agent-feedback/detail-expanded", { fullPage: true });
  });

  test("not found page", async ({ app, page }) => {
    await app.goto("/this/route/does/not/exist");
    await expect(page.getByRole("heading", { name: "Page Not Found" })).toBeVisible();
    await app.snap("not-found", { fullPage: true });
  });

  test("auth callback error", async ({ app, page }) => {
    await app.goto("/auth/callback");
    await expect(page.getByRole("heading", { name: "Sign-in failed" })).toBeVisible();
    await app.snap("auth-callback/error", { fullPage: true });
  });
});

for (const width of [1200, 900]) {
  test.describe(`cluster overview at ${width}px`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("cluster overview", async ({ app }) => {
      await app.goto("/cluster");
      await app.runTimers(10_000);
      await app.snap("cluster/overview", { fullPage: true });
    });
  });
}
