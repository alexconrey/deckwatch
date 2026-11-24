import type { Page } from "@playwright/test";
import { eachTheme, expect, test } from "./visual-test";
import { openMenu } from "../support/ui";

/**
 * Applications: list, create wizard, detail tabs and their dialogs.
 */

const wizard = (page: Page) => page.locator(".v-stepper");

eachTheme(() => {
  test("applications list", async ({ app }) => {
    await app.goto("/applications");
    await app.snap("applications/list", { fullPage: true });
  });

  test("applications list empty", async ({ app, mockState }) => {
    mockState.applications = [];
    await app.goto("/applications");
    await expect(app.page.getByText("No applications in this namespace")).toBeVisible();
    await app.snap("applications/list-empty", { fullPage: true });
  });

  test("create wizard step 1", async ({ app, page }) => {
    await app.goto("/applications/create");
    await page.getByLabel("App name").fill("checkout");
    await page.getByLabel("Description (optional)").fill("Checkout flow and payment hand-off.");
    await app.snap("applications/wizard-step1", { fullPage: true });
  });

  test("create wizard step 1 with an invalid name", async ({ app, page }) => {
    await app.goto("/applications/create");
    await page.getByLabel("App name").fill("Checkout_Service");
    await expect(page.getByText("Resource names cannot contain underscores", { exact: false })).toBeVisible();
    await expect(page.getByRole("button", { name: "Next" })).toBeDisabled();
    await app.snap("applications/wizard-step1-invalid", { fullPage: true });
  });

  test("create wizard namespace menu", async ({ app, page }) => {
    await app.goto("/applications/create");
    await openMenu(wizard(page), "Namespace");
    await expect(page.getByRole("option", { name: "staging" })).toBeVisible();
    await app.snap("applications/wizard-namespace-menu");
  });

  test("create wizard steps 2 to 4", async ({ app, page }) => {
    await app.goto("/applications/create");
    await page.getByLabel("App name").fill("checkout");
    await page.getByRole("button", { name: "Next" }).click();

    await page.getByText("I have a Git repository").click();
    await page.getByLabel("Repository URL").fill("https://github.com/acme/checkout.git");
    await page.getByLabel("Access token secret name").fill("github-bot-token");
    await app.snap("applications/wizard-step2-git", { fullPage: true });

    await page.getByRole("button", { name: "Next" }).click();
    // A public URL is the default and the hostname is still empty.
    await expect(page.getByText("Hostname is required for a public URL.")).toBeVisible();
    await app.snap("applications/wizard-step3-hostname-required", { fullPage: true });

    await page.getByLabel("Hostname", { exact: true }).fill("checkout.example.com");
    await page.getByRole("button", { name: "Next" }).click();
    await expect(page.getByText("Ready to create?")).toBeVisible();
    await app.snap("applications/wizard-step4-review", { fullPage: true });
  });

  test("application detail overview", async ({ app }) => {
    await app.goto("/applications/default/storefront");
    // Two metrics polls, so the sparklines are lines rather than a ramp.
    await app.runTimers(10_000);
    await app.snap("applications/detail-overview", { fullPage: true });
  });

  test("application detail plugins tab", async ({ app, page }) => {
    await app.goto("/applications/default/storefront");
    await page.getByRole("tab", { name: "Plugins" }).click();
    await expect(page.getByText("is not currently loaded")).toBeVisible();
    await app.snap("applications/detail-plugins", { fullPage: true });
  });

  test("add plugin dialog with the plugin select open", async ({ app, page }) => {
    await app.goto("/applications/default/storefront");
    await page.getByRole("tab", { name: "Plugins" }).click();
    await page.getByRole("button", { name: "Add Plugin" }).click();
    await openMenu(app.dialog("Add Plugin"), "Plugin");
    await expect(page.getByRole("option", { name: "aws-rds" })).toBeVisible();
    await app.snap("applications/add-plugin-select");
  });

  test("application detail infrastructure tab", async ({ app, page }) => {
    await app.goto("/applications/default/storefront");
    await page.getByRole("tab", { name: "Infrastructure" }).click();
    await expect(page.getByText("provisioned", { exact: true })).toBeVisible();
    await app.snap("applications/detail-infrastructure", { fullPage: true });
  });

  test("provision dialog with a select open", async ({ app, page }) => {
    await app.goto("/applications/default/storefront");
    await page.getByRole("tab", { name: "Infrastructure" }).click();
    await page.getByRole("button", { name: "Provision" }).click();
    const dialog = app.dialog("Add Redis cache");
    await expect(dialog).toBeVisible();
    await openMenu(dialog, "Node type");
    await expect(page.getByRole("option", { name: "cache.m6g.large" })).toBeVisible();
    await app.snap("applications/provision-select");
  });

  test("edit application dialog", async ({ app, page }) => {
    await app.goto("/applications/default/storefront");
    await page.getByRole("button", { name: "Edit" }).click();
    await expect(app.dialog("Edit Application").getByLabel("Repository URL")).toHaveValue(
      "https://github.com/acme/storefront.git",
    );
    await app.snap("applications/edit-dialog");
  });

  test("delete application dialog with cascade", async ({ app, page }) => {
    await app.goto("/applications/default/storefront");
    await page.getByRole("button", { name: "Delete" }).click();
    const dialog = app.dialog("Delete Application");
    await dialog.getByLabel("Also delete all member resources (deployments and cronjobs)").check();
    await dialog.getByRole("textbox").fill("storefront");
    await expect(dialog.getByText("This will delete all attached deployments and cronjobs.")).toBeVisible();
    await app.snap("applications/delete-dialog-cascade");
  });

  test("add resource dialog", async ({ app, page }) => {
    await app.goto("/applications/default/storefront");
    await page.getByRole("button", { name: "Add Resource" }).click();
    const dialog = app.dialog("Add Resource to Application");
    await expect(dialog.getByRole("cell", { name: "legacy-admin", exact: true })).toBeVisible();
    await app.snap("applications/add-resource-dialog");
  });

  test("fix with AI agent chooser", async ({ app, page }) => {
    await app.goto("/applications/default/storefront");
    await page.getByRole("button", { name: "Fix with AI" }).click();
    await expect(app.dialog("Choose an AI agent")).toBeVisible();
    await app.snap("applications/fix-with-ai-chooser");
  });
});

for (const width of [1200, 900]) {
  test.describe(`applications list at ${width}px`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("applications list", async ({ app }) => {
      await app.goto("/applications");
      await app.snap("applications/list", { fullPage: true });
    });
  });
}
