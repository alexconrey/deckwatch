import { deprovisionReport, pluginProblems, writePluginEvents } from "../support/data";
import { apiError, ok } from "../support/mock-api";
import { card } from "../support/ui";
import { openDeployment } from "./deployment-page";
import { eachTheme, expect, test } from "./visual-test";

/**
 * Plugin health and plugin events: the app bar's Problems button, the
 * Problems page, health badges and alerts on the application and plugin
 * pages, and the reports plugins send back on create, update, provision and
 * removal. The default dataset is all healthy; these states load
 * pluginProblems() or a plugin response with events.
 */

const RESOURCE = "/namespaces/:ns/applications/:name/resources/:plugin/:resource";

eachTheme(() => {
  test("app bar with failing plugins", async ({ app, page, mockState }) => {
    mockState.pluginStatus = pluginProblems();
    await app.goto("/applications");
    await expect(page.getByTestId("plugin-problems-nav")).toBeVisible();
    await app.snap("plugins/app-bar-failing", { locator: page.locator(".v-app-bar") });
  });

  test("app bar with only degraded plugins", async ({ app, page, mockState }) => {
    mockState.pluginStatus = pluginProblems().filter((s) => s.health !== "failing");
    await app.goto("/applications");
    await expect(page.getByTestId("plugin-problems-nav")).toBeVisible();
    await app.snap("plugins/app-bar-degraded", { locator: page.locator(".v-app-bar") });
  });

  test("problems page", async ({ app, page, mockState }) => {
    mockState.pluginStatus = pluginProblems();
    await app.goto("/plugin-problems");
    await expect(page.getByTestId("plugin-problem")).toHaveCount(3);
    await app.snap("plugins/problems-page", { fullPage: true });
  });

  test("problems page with nothing failing", async ({ app, page }) => {
    await app.goto("/plugin-problems");
    await expect(page.getByTestId("no-plugin-problems")).toBeVisible();
    await app.snap("plugins/problems-page-empty");
  });

  test("problems page load error", async ({ app, page, mockApi }) => {
    mockApi.on("GET", "/plugin-status", apiError(500, "database is locked", "internal_error"));
    await app.goto("/plugin-problems");
    await expect(page.getByText("database is locked")).toBeVisible();
    await app.snap("plugins/problems-page-error");
  });

  test("application with failing plugins", async ({ app, page, mockState }) => {
    mockState.pluginStatus = pluginProblems();
    await app.goto("/applications/default/storefront");
    await expect(page.getByTestId("app-plugin-problems")).toBeVisible();
    // Two metrics polls, so the sparklines are lines rather than a ramp.
    await app.runTimers(10_000);
    await app.snap("plugins/application-overview-problems", { fullPage: true });
  });

  test("application plugins tab with health badges", async ({ app, page, mockState }) => {
    mockState.pluginStatus = pluginProblems();
    await app.goto("/applications/default/storefront");
    await page.getByRole("tab", { name: "Plugins" }).click();
    await expect(page.getByText("is not currently loaded")).toBeVisible();
    await app.snap("plugins/application-plugins-problems", { fullPage: true });
  });

  test("application infrastructure tab with a failing resource", async ({ app, page, mockState }) => {
    mockState.pluginStatus = pluginProblems();
    await app.goto("/applications/default/storefront");
    await page.getByRole("tab", { name: "Infrastructure" }).click();
    await expect(page.getByText("provisioned", { exact: true })).toBeVisible();
    await app.snap("plugins/application-infrastructure-problems", { fullPage: true });
  });

  test("remove resource record dialog", async ({ app, page }) => {
    await app.goto("/applications/default/storefront");
    await page.getByRole("tab", { name: "Infrastructure" }).click();
    await page.getByRole("button", { name: "Remove record" }).click();
    await expect(app.dialog("Remove resource record")).toBeVisible();
    await app.snap("plugins/deprovision-dialog");
  });

  test("what the plugin reported on removal", async ({ app, page, mockApi }) => {
    mockApi.on("DELETE", RESOURCE, ok(deprovisionReport()));
    await app.goto("/applications/default/storefront");
    await page.getByRole("tab", { name: "Infrastructure" }).click();
    await page.getByRole("button", { name: "Remove record" }).click();
    await app.dialog("Remove resource record").getByRole("button", { name: "Remove record" }).click();
    await expect(page.getByTestId("infra-plugin-notice")).toBeVisible();
    // The "record removed" snackbar times out; the notice stays.
    await app.runTimers(5_000);
    await expect(page.locator(".v-snackbar--active")).toHaveCount(0);
    await app.snap("plugins/deprovision-report", { fullPage: true });
  });

  test("provision dialog with the plugin's error events", async ({ app, page, mockApi }) => {
    mockApi.on("POST", RESOURCE, {
      status: 502,
      json: {
        error: "plugin_failed",
        message: "Plugin call failed: aws-rds.quota_exceeded: ElastiCache node quota reached",
        plugin_events: [
          {
            plugin: "aws-rds",
            export: "provision",
            severity: "error",
            code: "aws-rds.quota_exceeded",
            message: "ElastiCache node quota reached (20 of 20 cache.m6g.large nodes).",
            resource_id: "redis",
            retryable: false,
            hint: "Request a quota increase or pick a smaller node type.",
          },
        ],
      },
    });
    await app.goto("/applications/default/storefront");
    await page.getByRole("tab", { name: "Infrastructure" }).click();
    await page.getByRole("button", { name: "Provision" }).click();
    const dialog = app.dialog("Add Redis cache");
    await dialog.getByRole("button", { name: "Provision" }).click();
    await expect(dialog.getByTestId("plugin-event-list")).toBeVisible();
    await app.snap("plugins/provision-error");
  });

  test("plugin page with failing targets", async ({ app, page, mockState }) => {
    mockState.pluginStatus = pluginProblems();
    await app.goto("/settings/plugins/aws-rds");
    await expect(page.getByTestId("plugin-status-problems")).toBeVisible();
    await app.snap("plugins/plugin-settings-problems", { fullPage: true });
  });

  test("deployment created with plugin events", async ({ app, page, mockState }) => {
    mockState.writePluginEvents = writePluginEvents();
    await app.goto("/deployments/create");
    await page.getByRole("textbox", { name: "Deployment Name", exact: true }).fill("checkout");
    await page.getByRole("textbox", { name: "Container Image", exact: true }).fill("ghcr.io/acme/checkout:1.4.0");
    await page.getByRole("button", { name: "Create Deployment" }).click();
    await expect(page.getByTestId("create-plugin-events")).toBeVisible();
    await app.snap("plugins/create-deployment-events", { fullPage: true });
  });

  test("deployment saved with plugin events", async ({ app, page, mockState }) => {
    mockState.writePluginEvents = writePluginEvents();
    await openDeployment(app);
    const labels = card(page, "Labels & Annotations");
    await labels.getByRole("button", { name: "Edit" }).click();
    await labels.getByRole("button", { name: "Save" }).click();
    await expect(page.getByTestId("update-plugin-events")).toBeVisible();
    await app.snap("plugins/deployment-update-events");
  });
});
