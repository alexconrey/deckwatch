import type { Page } from "@playwright/test";
import { expect, test } from "./fixtures";
import { menuItem, row } from "./support/ui";

/**
 * Resources page (/deployments): tabs, the deployments table and its row
 * actions, and the CronJob / Secret / ConfigMap / ServiceAccount dialogs.
 */

const deploymentRows = (page: Page) => page.locator(".v-data-table tbody tr");

async function openRowAction(page: Page, deployment: string, action: "Restart" | "Scale" | "Delete") {
  await row(page, deployment).locator("button:has(.mdi-dots-vertical)").click();
  await menuItem(page, action).click();
}

test.describe("Resources tabs", () => {
  test("tabs switch the table and are reachable from the keyboard", async ({ app, page }) => {
    await app.goto("/deployments");
    const deploymentsTab = page.getByRole("tab", { name: "Deployments" });
    await expect(deploymentsTab).toHaveAttribute("aria-selected", "true");
    await expect(row(page, "web")).toBeVisible();

    await page.getByRole("tab", { name: "CronJobs" }).click();
    await expect(page.getByRole("tab", { name: "CronJobs" })).toHaveAttribute("aria-selected", "true");
    await expect(row(page, "nightly-backup")).toBeVisible();
    await expect(page.getByRole("button", { name: "New CronJob" })).toBeVisible();

    await page.getByRole("tab", { name: "Secrets" }).click();
    await expect(row(page, "api-credentials")).toBeVisible();
    await page.getByRole("tab", { name: "ConfigMaps" }).click();
    await expect(row(page, "app-config")).toBeVisible();
    await page.getByRole("tab", { name: "ServiceAccounts" }).click();
    await expect(row(page, "api-s3-reader")).toBeVisible();

    // Arrow keys move focus along the tab list; Enter selects.
    await page.getByRole("tab", { name: "ServiceAccounts" }).focus();
    await page.keyboard.press("ArrowLeft");
    await expect(page.getByRole("tab", { name: "ConfigMaps" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("tab", { name: "ConfigMaps" })).toHaveAttribute("aria-selected", "true");
  });

  test("?tab= opens a tab directly and /secrets redirects to it", async ({ app, page }) => {
    await app.goto("/deployments?tab=configmaps");
    await expect(page.getByRole("tab", { name: "ConfigMaps" })).toHaveAttribute("aria-selected", "true");

    await app.goto("/secrets");
    await expect(page).toHaveURL(/\/deployments\?tab=secrets$/);
    await expect(page.getByRole("tab", { name: "Secrets" })).toHaveAttribute("aria-selected", "true");
    await expect(row(page, "registry-pull")).toBeVisible();
  });

  test("every list is fetched for the selected namespace", async ({ app, mockApi }) => {
    await app.goto("/deployments");
    for (const kind of ["deployments", "cronjobs", "secrets", "configmaps", "serviceaccounts"]) {
      await mockApi.waitForRequest("GET", `/namespaces/default/${kind}`);
    }
  });
});

test.describe("Deployments tab", () => {
  test("search filters the rows", async ({ app, page }) => {
    await app.goto("/deployments");
    await expect(deploymentRows(page)).toHaveCount(6);

    const search = page.getByRole("textbox", { name: "Search deployments" });
    await search.fill("work");
    await expect(deploymentRows(page)).toHaveCount(1);
    await expect(row(page, "worker")).toBeVisible();

    // Matches the image column too.
    await search.fill("ghcr.io/acme");
    await expect(deploymentRows(page)).toHaveCount(4);

    await search.fill("no-such-deployment");
    await expect(page.getByText("No deployments in this namespace")).toBeVisible();

    await search.clear();
    await expect(deploymentRows(page)).toHaveCount(6);
  });

  test("clicking a row opens the deployment", async ({ app, page }) => {
    await app.goto("/deployments");
    await row(page, "api").click();
    await expect(page).toHaveURL(/\/deployments\/default\/api$/);
    await expect(page.getByRole("heading", { name: "api", exact: true })).toBeVisible();
  });

  test("Create Deployment opens the form", async ({ app, page }) => {
    await app.goto("/deployments");
    await page.getByRole("button", { name: "Create Deployment" }).click();
    await expect(page).toHaveURL(/\/deployments\/create$/);
  });

  test("the row menu scales a deployment", async ({ app, page, mockApi }) => {
    await app.goto("/deployments");
    await openRowAction(page, "web", "Scale");

    const dialog = app.dialog("Scale Deployment");
    await app.autofocused(dialog.getByLabel("Replicas"));
    await expect(dialog.getByText("Set replica count for web:")).toBeVisible();
    const replicas = dialog.getByRole("spinbutton", { name: "Replicas" });
    await expect(replicas).toHaveValue("3");
    await replicas.fill("5");
    await dialog.getByRole("button", { name: "Scale" }).click();

    const call = await mockApi.waitForRequest("POST", "/namespaces/default/deployments/web/scale");
    expect(call.body).toEqual({ replicas: 5 });
    await expect(dialog).toBeHidden();
  });

  test("the row menu restarts a deployment after confirmation", async ({ app, page, mockApi }) => {
    await app.goto("/deployments");
    await openRowAction(page, "api", "Restart");

    const dialog = app.dialog("Restart Deployment");
    await expect(dialog.getByText("This will trigger a rolling restart of 'api'.")).toBeVisible();
    await dialog.getByRole("button", { name: "Restart" }).click();

    const call = await mockApi.waitForRequest("POST", "/namespaces/default/deployments/api/restart");
    expect(call.body).toBeNull();
    await expect(dialog).toBeHidden();
  });

  test("the row menu deletes a deployment once its name is typed", async ({ app, page, mockApi }) => {
    await app.goto("/deployments");
    await openRowAction(page, "worker", "Delete");

    const dialog = app.dialog("Delete Deployment");
    await app.autofocused(dialog.getByRole("textbox"));
    const confirm = dialog.getByRole("button", { name: "Delete" });
    await expect(confirm).toBeDisabled();
    await dialog.getByRole("textbox").fill("worke");
    await expect(confirm).toBeDisabled();
    await dialog.getByRole("textbox").fill("worker");
    await confirm.click();

    await mockApi.waitForRequest("DELETE", "/namespaces/default/deployments/worker");
    await expect(dialog).toBeHidden();
    await expect(row(page, "worker")).toBeHidden();
  });

  test("cancelling a row action sends nothing", async ({ app, page, mockApi }) => {
    await app.goto("/deployments");
    await openRowAction(page, "web", "Restart");
    const dialog = app.dialog("Restart Deployment");
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toBeHidden();
    // Focus must not be left inside the closed dialog.
    expect(
      await page.evaluate(() => !document.activeElement?.closest(".v-overlay__content")),
    ).toBe(true);
    expect(mockApi.requests("POST", "/namespaces/:ns/deployments/:name/restart")).toHaveLength(0);
  });
});

test.describe("CronJobs tab", () => {
  test("New CronJob posts the schedule, image, suspend flag and split command", async ({ app, page, mockApi }) => {
    await app.goto("/deployments?tab=cronjobs");
    await page.getByRole("button", { name: "New CronJob" }).click();

    const dialog = app.dialog("New CronJob");
    await app.autofocused(dialog.getByLabel("Name"));
    await expect(dialog.getByLabel("Name")).toBeFocused();
    await dialog.getByLabel("Name").fill("Db_Vacuum");
    await expect(dialog.getByText("Lowercase alphanumeric or '-', start/end alphanumeric")).toBeVisible();
    await dialog.getByLabel("Name").fill("db-vacuum");
    await expect(dialog.getByText("Lowercase alphanumeric or '-', start/end alphanumeric")).toBeHidden();
    await dialog.getByLabel("Schedule").fill("30 3 * * 0");
    await dialog.getByLabel("Image").fill("postgres:16");
    await dialog.getByLabel("Command (optional)").fill("  vacuumdb   --all  --analyze ");
    await dialog.getByLabel("Suspend", { exact: true }).check();
    await dialog.getByRole("button", { name: "Create" }).click();

    const call = await mockApi.waitForRequest("POST", "/namespaces/default/cronjobs");
    expect(call.body).toEqual({
      name: "db-vacuum",
      schedule: "30 3 * * 0",
      image: "postgres:16",
      suspend: true,
      command: ["vacuumdb", "--all", "--analyze"],
    });
    await expect(dialog).toBeHidden();
    // The list is re-fetched after the create.
    await expect.poll(() => mockApi.requests("GET", "/namespaces/default/cronjobs").length).toBeGreaterThan(1);
  });

  test("Edit sends only what can change", async ({ app, page, mockApi }) => {
    await app.goto("/deployments?tab=cronjobs");
    await row(page, "cache-warmer").getByTitle("Edit").click();

    const dialog = app.dialog("Edit cache-warmer");
    await app.autofocused(dialog.getByLabel("Name"));
    await expect(dialog.getByLabel("Name")).toHaveValue("cache-warmer");
    await expect(dialog.getByLabel("Name")).toHaveAttribute("readonly", "");
    await expect(dialog.getByText("Leave blank to keep existing image")).toBeVisible();
    await dialog.getByLabel("Schedule").fill("*/30 * * * *");
    await dialog.getByRole("button", { name: "Save" }).click();

    const call = await mockApi.waitForRequest("PATCH", "/namespaces/default/cronjobs/cache-warmer");
    expect(call.body).toEqual({ schedule: "*/30 * * * *", suspend: false });
  });

  test("Run now triggers a job and reports it", async ({ app, page, mockApi }) => {
    await app.goto("/deployments?tab=cronjobs");
    await row(page, "nightly-backup").getByTitle("Run now").click();

    await mockApi.waitForRequest("POST", "/namespaces/default/cronjobs/nightly-backup/trigger");
    const alert = page.getByRole("alert").filter({ hasText: 'Job "nightly-backup-manual-29104" created.' });
    await expect(alert).toBeVisible();
    await alert.getByRole("button", { name: "Close" }).click();
    await expect(alert).toBeHidden();
  });

  test("View logs shows the last job's output", async ({ app, page, mockApi }) => {
    await app.goto("/deployments?tab=cronjobs");
    await row(page, "nightly-backup").getByTitle("View logs").click();

    await mockApi.waitForRequest("GET", "/namespaces/default/cronjobs/nightly-backup/logs");
    const dialog = app.dialog("CronJob Logs");
    await expect(dialog.getByText("nightly-backup-29103-5xk8q")).toBeVisible();
    await expect(dialog.getByText("nightly-backup-29103", { exact: true })).toBeVisible();
    await expect(dialog.getByText("uploaded 2.1 GiB to s3://acme-backups/2026-06-01")).toBeVisible();
    await dialog.getByRole("button", { name: "Close" }).click();
    await expect(dialog).toBeHidden();
  });

  test("Delete asks for confirmation", async ({ app, page, mockApi }) => {
    await app.goto("/deployments?tab=cronjobs");
    await row(page, "quarterly-report").getByTitle("Delete").click();
    const dialog = app.dialog("Delete CronJob");
    await expect(
      dialog.getByText("Are you sure you want to delete 'quarterly-report'? This action cannot be undone."),
    ).toBeVisible();
    await dialog.getByRole("button", { name: "Delete" }).click();
    await mockApi.waitForRequest("DELETE", "/namespaces/default/cronjobs/quarterly-report");
    await expect(dialog).toBeHidden();
  });
});

test.describe("Secrets tab", () => {
  test("Create Secret posts the type and every key", async ({ app, page, mockApi }) => {
    await app.goto("/deployments?tab=secrets");
    await page.getByRole("button", { name: "Create Secret" }).click();

    const dialog = app.dialog("Create Secret");
    await app.autofocused(dialog.getByLabel("Name"));
    await dialog.getByLabel("Name").fill("smtp");
    await expect(dialog.getByLabel("Type")).toHaveValue("Opaque");

    // Nothing to save yet: the dialog says so and sends nothing.
    await dialog.getByRole("button", { name: "Create" }).click();
    await expect(dialog.getByText("At least one key/value pair is required")).toBeVisible();
    expect(mockApi.requests("POST", "/namespaces/:ns/secrets")).toHaveLength(0);

    await dialog.getByLabel("Key").fill("SMTP_PASSWORD");
    const value = dialog.getByRole("textbox", { name: "Value" });
    await value.fill("hunter2");
    await expect(value).toHaveAttribute("type", "password");
    await dialog.getByRole("button", { name: "Add key/value" }).click();
    await dialog.getByLabel("Key").nth(1).fill("SMTP_USER");
    await dialog.getByRole("textbox", { name: "Value" }).nth(1).fill("mailer");
    await dialog.getByRole("button", { name: "Create" }).click();

    const call = await mockApi.waitForRequest("POST", "/namespaces/default/secrets");
    expect(call.body).toEqual({
      name: "smtp",
      secret_type: "Opaque",
      data: { SMTP_PASSWORD: "hunter2", SMTP_USER: "mailer" },
    });
    await expect(dialog).toBeHidden();
  });

  test("values stay masked until revealed", async ({ app, page, mockApi }) => {
    await app.goto("/deployments?tab=secrets");
    await row(page, "api-credentials").locator("button:has(.mdi-eye)").click();

    const dialog = page.getByRole("dialog");
    await expect(dialog.getByText("••••••••")).toHaveCount(2);
    const plain = mockApi.requests("GET", "/namespaces/default/secrets/api-credentials");
    expect(plain).toHaveLength(1);
    expect(plain[0].query.get("reveal")).toBeNull();

    await dialog.getByRole("button", { name: "Reveal values" }).click();
    await expect(dialog.getByText("ak_live_51H8xK2eZvKYlo2C")).toBeVisible();
    await expect(dialog.getByText("s3cr3t-value-for-e2e")).toBeVisible();
    const calls = mockApi.requests("GET", "/namespaces/default/secrets/api-credentials");
    expect(calls.at(-1)?.query.get("reveal")).toBe("true");

    await dialog.getByRole("button", { name: "Hide values" }).click();
    await expect(dialog.getByText("••••••••")).toHaveCount(2);
    expect(mockApi.requests("GET", "/namespaces/default/secrets/api-credentials").at(-1)?.query.get("reveal")).toBeNull();

    // Not persistent: Escape closes it.
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
  });

  test("delete asks the browser to confirm", async ({ app, page, mockApi }) => {
    await app.goto("/deployments?tab=secrets");

    page.once("dialog", (d) => d.dismiss());
    await row(page, "web-tls").locator("button:has(.mdi-delete)").click();
    expect(mockApi.requests("DELETE", "/namespaces/:ns/secrets/:name")).toHaveLength(0);

    let message = "";
    page.once("dialog", (d) => {
      message = d.message();
      void d.accept();
    });
    await row(page, "web-tls").locator("button:has(.mdi-delete)").click();
    await mockApi.waitForRequest("DELETE", "/namespaces/default/secrets/web-tls");
    expect(message).toBe('Delete secret "web-tls"? This cannot be undone.');
  });

  test("persistent create dialog ignores Escape and closes with Cancel", async ({ app, page, mockApi }) => {
    await app.goto("/deployments?tab=secrets");
    await page.getByRole("button", { name: "Create Secret" }).click();
    const dialog = app.dialog("Create Secret");
    await app.autofocused(dialog.getByLabel("Name"));
    await expect(dialog).toHaveAttribute("aria-modal", "true");
    await expect(dialog.getByLabel("Name")).toBeFocused();

    await page.keyboard.press("Escape");
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toBeHidden();
    expect(mockApi.requests("POST", "/namespaces/:ns/secrets")).toHaveLength(0);
  });
});

test.describe("ConfigMaps tab", () => {
  test("Edit loads the data and puts it back", async ({ app, page, mockApi }) => {
    await app.goto("/deployments?tab=configmaps");
    await row(page, "app-config").locator("button:has(.mdi-pencil)").click();

    await mockApi.waitForRequest("GET", "/namespaces/default/configmaps/app-config");
    const dialog = app.dialog("Edit app-config");
    await app.autofocused(dialog.getByLabel("Name"));
    await expect(dialog.getByLabel("Name")).toHaveValue("app-config");
    await expect(dialog.getByLabel("Key")).toHaveCount(2);
    await expect(dialog.getByLabel("Key").first()).toHaveValue("LOG_LEVEL");
    await dialog.getByRole("textbox", { name: "Value" }).first().fill("debug");
    await dialog.getByRole("button", { name: "Save" }).click();

    const call = await mockApi.waitForRequest("PUT", "/namespaces/default/configmaps/app-config");
    expect(call.body).toEqual({
      name: "app-config",
      data: { LOG_LEVEL: "debug", FEATURE_FLAGS: "checkout_v2=true\nnew_search=false" },
    });
    await expect(dialog).toBeHidden();
  });

  test("Create ConfigMap rejects an invalid name", async ({ app, page, mockApi }) => {
    await app.goto("/deployments?tab=configmaps");
    await page.getByRole("button", { name: "Create ConfigMap" }).click();
    const dialog = app.dialog("Create ConfigMap");
    await app.autofocused(dialog.getByLabel("Name"));
    await dialog.getByLabel("Name").fill("Bad Name");
    await dialog.getByRole("button", { name: "Create" }).click();
    await expect(dialog.getByRole("alert").filter({ hasText: "Invalid configmap name" })).toBeVisible();
    expect(mockApi.requests("POST", "/namespaces/:ns/configmaps")).toHaveLength(0);
  });

  test("delete asks the browser to confirm", async ({ app, page, mockApi }) => {
    await app.goto("/deployments?tab=configmaps");
    page.once("dialog", (d) => d.accept());
    await row(page, "feature-toggles").locator("button:has(.mdi-delete)").click();
    await mockApi.waitForRequest("DELETE", "/namespaces/default/configmaps/feature-toggles");
  });
});

test.describe("ServiceAccounts tab", () => {
  test("Create ServiceAccount sets the IRSA annotation", async ({ app, page, mockApi }) => {
    await app.goto("/deployments?tab=serviceaccounts");
    await page.getByRole("button", { name: "Create ServiceAccount" }).click();

    const dialog = app.dialog("Create ServiceAccount");
    await app.autofocused(dialog.getByLabel("Name"));
    await dialog.getByLabel("Name").fill("billing-worker");
    await dialog.getByLabel("IRSA Role ARN (optional)").fill("  arn:aws:iam::123456789012:role/billing  ");
    await dialog.getByRole("button", { name: "Create" }).click();

    const call = await mockApi.waitForRequest("POST", "/namespaces/default/serviceaccounts");
    expect(call.body).toEqual({
      name: "billing-worker",
      annotations: { "eks.amazonaws.com/role-arn": "arn:aws:iam::123456789012:role/billing" },
    });
    await expect(dialog).toBeHidden();
  });

  test("without a role ARN the annotations are empty", async ({ app, page, mockApi }) => {
    await app.goto("/deployments?tab=serviceaccounts");
    await page.getByRole("button", { name: "Create ServiceAccount" }).click();
    const dialog = app.dialog("Create ServiceAccount");
    await app.autofocused(dialog.getByLabel("Name"));
    await dialog.getByLabel("Name").fill("plain");
    await dialog.getByRole("button", { name: "Create" }).click();
    const call = await mockApi.waitForRequest("POST", "/namespaces/default/serviceaccounts");
    expect(call.body).toEqual({ name: "plain", annotations: {} });
  });

  test("delete asks the browser to confirm", async ({ app, page, mockApi }) => {
    await app.goto("/deployments?tab=serviceaccounts");
    page.once("dialog", (d) => d.accept());
    await row(page, "api-s3-reader").locator("button:has(.mdi-delete)").click();
    await mockApi.waitForRequest("DELETE", "/namespaces/default/serviceaccounts/api-s3-reader");
  });
});
