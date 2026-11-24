import type { Page } from "@playwright/test";
import { eachTheme, expect, test } from "./visual-test";
import { menuItem, row } from "../support/ui";

/**
 * Resources page (/deployments): the five tabs, their tables and dialogs.
 */

async function openRowMenu(page: Page, deployment: string): Promise<void> {
  await row(page, deployment).locator("button:has(.mdi-dots-vertical)").click();
  await expect(menuItem(page, "Scale")).toBeVisible();
}

eachTheme(() => {
  test.describe("deployments tab", () => {
    test("populated", async ({ app }) => {
      await app.goto("/deployments");
      await expect(row(app.page, "legacy-admin")).toBeVisible();
      await app.snap("resources/deployments", { fullPage: true });
    });

    test("search with matches", async ({ app, page }) => {
      await app.goto("/deployments");
      await page.getByRole("textbox", { name: "Search deployments" }).fill("acme");
      await expect(row(page, "web")).toBeHidden();
      await app.snap("resources/deployments-search", { fullPage: true });
    });

    test("search without matches", async ({ app, page }) => {
      await app.goto("/deployments");
      await page.getByRole("textbox", { name: "Search deployments" }).fill("no-such-deployment");
      await expect(page.getByText("No deployments in this namespace")).toBeVisible();
      await app.snap("resources/deployments-search-empty", { fullPage: true });
    });

    test("row menu", async ({ app, page }) => {
      await app.goto("/deployments");
      await openRowMenu(page, "web");
      await app.snap("resources/deployments-row-menu");
    });

    test("scale dialog", async ({ app, page }) => {
      await app.goto("/deployments");
      await openRowMenu(page, "web");
      await menuItem(page, "Scale").click();
      await expect(app.dialog("Scale Deployment")).toBeVisible();
      await app.snap("resources/deployments-scale-dialog");
    });

    test("restart confirmation", async ({ app, page }) => {
      await app.goto("/deployments");
      await openRowMenu(page, "api");
      await menuItem(page, "Restart").click();
      await expect(app.dialog("Restart Deployment")).toBeVisible();
      await app.snap("resources/deployments-restart-dialog");
    });

    test("delete confirmation with the name typed", async ({ app, page }) => {
      await app.goto("/deployments");
      await openRowMenu(page, "worker");
      await menuItem(page, "Delete").click();
      const dialog = app.dialog("Delete Deployment");
      await dialog.getByRole("textbox").fill("worker");
      await expect(dialog.getByRole("button", { name: "Delete" })).toBeEnabled();
      await app.snap("resources/deployments-delete-dialog");
    });
  });

  test.describe("cronjobs tab", () => {
    test("populated", async ({ app, page }) => {
      await app.goto("/deployments?tab=cronjobs");
      await expect(row(page, "quarterly-report")).toBeVisible();
      await app.snap("resources/cronjobs", { fullPage: true });
    });

    test("new cronjob dialog", async ({ app, page }) => {
      await app.goto("/deployments?tab=cronjobs");
      await page.getByRole("button", { name: "New CronJob" }).click();
      await expect(app.dialog("New CronJob")).toBeVisible();
      await app.snap("resources/cronjob-create-dialog");
    });

    test("new cronjob dialog with an invalid name", async ({ app, page }) => {
      await app.goto("/deployments?tab=cronjobs");
      await page.getByRole("button", { name: "New CronJob" }).click();
      const dialog = app.dialog("New CronJob");
      // Let the dialog finish opening: its autofocus would otherwise move the
      // focus back to Name after the fields are filled (Vuetify 4 defers it).
      await expect(dialog.getByLabel("Name")).toBeFocused();
      await dialog.getByLabel("Name").fill("Nightly_Backup");
      await dialog.getByLabel("Schedule").fill("0 3 * * *");
      await expect(dialog.getByText("Lowercase alphanumeric or '-', start/end alphanumeric")).toBeVisible();
      await app.snap("resources/cronjob-create-dialog-invalid");
    });

    test("logs dialog", async ({ app, page }) => {
      await app.goto("/deployments?tab=cronjobs");
      await row(page, "nightly-backup").getByTitle("View logs").click();
      const dialog = app.dialog("CronJob Logs");
      await expect(dialog.getByText("uploaded 2.1 GiB")).toBeVisible();
      await app.snap("resources/cronjob-logs-dialog");
    });

    test("run now result", async ({ app, page }) => {
      await app.goto("/deployments?tab=cronjobs");
      await row(page, "nightly-backup").getByTitle("Run now").click();
      await expect(page.getByText('Job "nightly-backup-manual-29104" created.')).toBeVisible();
      await app.snap("resources/cronjobs-run-now", { fullPage: true });
    });
  });

  test.describe("secrets tab", () => {
    test("populated", async ({ app, page }) => {
      await app.goto("/deployments?tab=secrets");
      await expect(row(page, "web-tls")).toBeVisible();
      await app.snap("resources/secrets", { fullPage: true });
    });

    test("create secret dialog", async ({ app, page }) => {
      await app.goto("/deployments?tab=secrets");
      await page.getByRole("button", { name: "Create Secret" }).click();
      const dialog = app.dialog("Create Secret");
      await dialog.getByLabel("Name").fill("smtp-credentials");
      await dialog.getByLabel("Key").fill("SMTP_PASSWORD");
      await dialog.getByRole("textbox", { name: "Value" }).fill("hunter2");
      await dialog.getByRole("button", { name: "Add key/value" }).click();
      await expect(dialog.getByLabel("Key")).toHaveCount(2);
      await app.snap("resources/secret-create-dialog");
    });

    test("view secret with values revealed", async ({ app, page }) => {
      await app.goto("/deployments?tab=secrets");
      await row(page, "api-credentials").locator("button:has(.mdi-eye)").click();
      const dialog = page.getByRole("dialog");
      await dialog.getByRole("button", { name: "Reveal values" }).click();
      await expect(dialog.getByText("ak_live_51H8xK2eZvKYlo2C")).toBeVisible();
      await app.snap("resources/secret-view-dialog-revealed");
    });
  });

  test.describe("configmaps tab", () => {
    test("populated", async ({ app, page }) => {
      await app.goto("/deployments?tab=configmaps");
      await expect(row(page, "feature-toggles")).toBeVisible();
      await app.snap("resources/configmaps", { fullPage: true });
    });

    test("edit configmap dialog", async ({ app, page }) => {
      await app.goto("/deployments?tab=configmaps");
      await row(page, "app-config").locator("button:has(.mdi-pencil)").click();
      const dialog = app.dialog("Edit app-config");
      await expect(dialog.getByLabel("Key").first()).toHaveValue("LOG_LEVEL");
      await app.snap("resources/configmap-edit-dialog");
    });
  });

  test.describe("serviceaccounts tab", () => {
    test("populated", async ({ app, page }) => {
      await app.goto("/deployments?tab=serviceaccounts");
      await expect(row(page, "api-s3-reader")).toBeVisible();
      await app.snap("resources/serviceaccounts", { fullPage: true });
    });

    test("create serviceaccount dialog", async ({ app, page }) => {
      await app.goto("/deployments?tab=serviceaccounts");
      await page.getByRole("button", { name: "Create ServiceAccount" }).click();
      const dialog = app.dialog("Create ServiceAccount");
      await dialog.getByLabel("Name").fill("billing-worker");
      await app.snap("resources/serviceaccount-create-dialog");
    });
  });

  test.describe("empty namespace", () => {
    test.beforeEach(async ({ page }) => {
      await page.addInitScript(() => localStorage.setItem("deckwatch-namespace", "team-a"));
    });

    test("deployments", async ({ app, page }) => {
      await app.goto("/deployments");
      await expect(page.getByText("No deployments in this namespace")).toBeVisible();
      await app.snap("resources/empty-deployments", { fullPage: true });
    });

    test("cronjobs", async ({ app, page }) => {
      await app.goto("/deployments?tab=cronjobs");
      await expect(page.getByText("No cronjobs in this namespace")).toBeVisible();
      await app.snap("resources/empty-cronjobs", { fullPage: true });
    });
  });
});

// Vuetify 4 moved the md/lg breakpoints (960 -> 840, 1280 -> 1145), so the
// page is also compared at widths between the old and new thresholds.
for (const width of [1200, 900]) {
  test.describe(`at ${width}px`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("deployments tab", async ({ app, page }) => {
      await app.goto("/deployments");
      await expect(row(page, "legacy-admin")).toBeVisible();
      await app.snap("resources/deployments", { fullPage: true });
    });
  });
}
