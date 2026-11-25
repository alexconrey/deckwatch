import type { Locator } from "@playwright/test";
import { expect, test } from "./fixtures";
import { deprovisionReport, pluginProblems, writePluginEvents } from "./support/data";
import { ok } from "./support/mock-api";
import { card } from "./support/ui";

/**
 * Plugin health and plugin events: the app bar's Problems button, the
 * Problems page, the health badges and alerts on application and plugin
 * pages, and what plugins report on create, update, provision and removal.
 * Rendering is covered by visual/plugin-status.spec.ts.
 */

const RESOURCE = "/namespaces/:ns/applications/:name/resources/:plugin/:resource";

/** The data-severity of each plugin event shown in `scope`, in order. */
const severities = (scope: Locator) =>
  scope.locator(".plugin-event").evaluateAll((els) => els.map((e) => e.getAttribute("data-severity")));

test.describe("Plugin problems", () => {
  test("the app bar shows Problems only while a target is failing or degraded", async ({
    app,
    page,
    mockApi,
    mockState,
  }) => {
    await app.goto("/applications");
    const poll = await mockApi.waitForRequest("GET", "/plugin-status");
    expect(poll.query.get("health")).toBe("failing,degraded");
    await expect(page.getByTestId("plugin-problems-nav")).toHaveCount(0);

    // The app bar polls every 30 seconds.
    mockState.pluginStatus = pluginProblems();
    await app.runTimers(30_000);
    const nav = page.getByTestId("plugin-problems-nav");
    await expect(nav).toBeVisible();
    await expect(nav.locator(".v-badge__badge")).toHaveText("3");

    await nav.click();
    await expect(page).toHaveURL(/\/plugin-problems$/);
    await expect(page.getByRole("heading", { name: "Plugin problems" })).toBeVisible();
  });

  test("the Problems page lists failing targets first, with their events", async ({ app, page, mockState }) => {
    mockState.pluginStatus = pluginProblems();
    await app.goto("/plugin-problems");

    const problems = page.getByTestId("plugin-problem");
    await expect(problems).toHaveCount(3);
    await expect(page.getByText("2 failing", { exact: true })).toBeVisible();
    await expect(page.getByText("1 degraded", { exact: true })).toBeVisible();

    await expect(problems.nth(0)).toContainText("default/storefront · postgres");
    await expect(problems.nth(0)).toContainText(
      "Failing for 14m · 3 failed fetch() calls in a row · last success 2h ago",
    );
    await expect(problems.nth(0)).toContainText("Hint: Allow rds:DescribeDBInstances for the deckwatch IAM role.");
    await expect(problems.nth(0)).toContainText("Trace: 4bf92f3577b34da6a3ce929d0e0e4736");
    await expect(problems.nth(1)).toContainText("vault-secrets (load)");
    await expect(problems.nth(1)).toContainText("no success recorded");
    await expect(problems.nth(2)).toContainText("default/storefront · apply");
    await expect(problems.nth(2)).toContainText("Last apply() call 1m ago");
    // Within a target, the most severe event comes first.
    await expect(problems.nth(2).locator(".plugin-event")).toHaveCount(2);
    expect(await severities(problems.nth(2))).toEqual(["warning", "info"]);

    await problems.nth(0).getByRole("button", { name: "Open application" }).click();
    await expect(page).toHaveURL(/\/applications\/default\/storefront$/);
  });

  test("Open plugin goes to the plugin's settings, which show the load failure", async ({ app, page, mockApi, mockState }) => {
    mockState.pluginStatus = pluginProblems();
    await app.goto("/plugin-problems");
    await page
      .getByTestId("plugin-problem")
      .filter({ hasText: "vault-secrets" })
      .getByRole("button", { name: "Open plugin" })
      .click();

    await expect(page).toHaveURL(/\/settings\/plugins\/vault-secrets$/);
    await mockApi.waitForRequest("GET", "/plugins/vault-secrets/status");
    const alert = page.getByTestId("plugin-status-problems");
    await expect(alert).toContainText("vault-secrets (load)");
    await expect(alert).toContainText("wasm trap: unreachable instruction executed");
    await alert.getByRole("link", { name: "All plugin problems" }).click();
    await expect(page).toHaveURL(/\/plugin-problems$/);
  });

  test("a target that recovers drops off on the next poll", async ({ app, page, mockState }) => {
    mockState.pluginStatus = pluginProblems();
    await app.goto("/plugin-problems");
    await expect(page.getByTestId("plugin-problem")).toHaveCount(3);

    mockState.pluginStatus = mockState.pluginStatus.map((s) =>
      s.plugin === "otel" ? { ...s, health: "healthy", last_events: [] } : s,
    );
    await app.runTimers(10_000);
    await expect(page.getByTestId("plugin-problem")).toHaveCount(2);
    await expect(page.getByText("1 degraded", { exact: true })).toHaveCount(0);
  });

  test("with nothing failing the page says so", async ({ app, page }) => {
    await app.goto("/plugin-problems");
    await expect(page.getByTestId("no-plugin-problems")).toHaveText("No plugin is failing or degraded.");
    await expect(page.getByTestId("plugin-problem")).toHaveCount(0);
  });
});

test.describe("Plugin health on application and plugin pages", () => {
  test("a healthy application gets a green Plugins badge and no alert", async ({ app, page, mockApi }) => {
    await app.goto("/applications/default/storefront");
    await mockApi.waitForRequest("GET", "/namespaces/default/applications/storefront/plugin-status");
    await expect(page.getByTestId("plugin-health-chip").first()).toHaveText("Plugins: healthy");
    await expect(page.getByTestId("app-plugin-problems")).toHaveCount(0);
  });

  test("an application lists its failing and degraded targets on every tab", async ({ app, page, mockState }) => {
    mockState.pluginStatus = pluginProblems();
    await app.goto("/applications/default/storefront");

    await expect(page.getByTestId("plugin-health-chip").first()).toHaveText("Plugins: failing");
    const alert = page.getByTestId("app-plugin-problems");
    await expect(alert).toContainText("2 plugin targets are failing");
    await expect(alert).toContainText("3 failed calls in a row");
    await expect(alert).toContainText("DescribeDBInstances returned AccessDenied for storefront-db.");
    // vault-secrets has no application, so it is not this application's problem.
    await expect(alert).not.toContainText("vault-secrets");

    await page.getByRole("tab", { name: "Plugins" }).click();
    await expect(alert).toBeVisible();
    const pluginCard = (name: string) =>
      page.locator(".v-card").filter({ has: page.locator(".text-subtitle-2").getByText(name, { exact: true }) });
    await expect(pluginCard("otel").getByTestId("plugin-health-chip")).toHaveText("degraded");
    await expect(pluginCard("aws-rds").getByTestId("plugin-health-chip")).toHaveText("failing");

    await page.getByRole("tab", { name: "Infrastructure" }).click();
    await expect(page.locator('[data-testid="plugin-health-chip"][data-health="failing"]').last()).toBeVisible();
  });

  test("a plugin's page shows its health and failing targets", async ({ app, page, mockState }) => {
    mockState.pluginStatus = pluginProblems();
    await app.goto("/settings/plugins/aws-rds");
    await expect(page.getByTestId("plugin-health-chip").first()).toHaveText("failing");
    const alert = page.getByTestId("plugin-status-problems");
    await expect(alert).toContainText("default/storefront · postgres");
    await expect(alert).not.toContainText("otel");
  });
});

test.describe("Plugin events", () => {
  test("removing a resource record shows what the plugin did and didn't clean up", async ({ app, page, mockApi }) => {
    mockApi.on("DELETE", RESOURCE, ok(deprovisionReport()));
    await app.goto("/applications/default/storefront");
    await page.getByRole("tab", { name: "Infrastructure" }).click();
    await page.getByRole("button", { name: "Remove record" }).click();
    await app.dialog("Remove resource record").getByRole("button", { name: "Remove record" }).click();

    await mockApi.waitForRequest("DELETE", "/namespaces/default/applications/storefront/resources/aws-rds/postgres");
    const notice = page.getByTestId("infra-plugin-notice");
    await expect(notice).toContainText("PostgreSQL database record removed");
    await expect(page.getByTestId("infra-plugin-message")).toHaveText(
      "Deleted the RDS instance storefront-db. Its final snapshot storefront-db-final is kept.",
    );
    await expect(notice).toContainText("The final snapshot storefront-db-final is billed until you delete it.");
    await expect(notice.locator("li")).toHaveText([
      "Job storefront-db-migrate: deleted",
      "Job storefront-db-backup: not found",
      "ServiceAccount storefront-db: refused (still used by deployment/web; deleting it would stop new pods from being created)",
    ]);
    await notice.getByRole("button", { name: "Close" }).click();
    await expect(notice).toHaveCount(0);
  });

  test("a removal the plugin has nothing to say about shows only the snackbar", async ({ app, page, mockApi }) => {
    await app.goto("/applications/default/storefront");
    await page.getByRole("tab", { name: "Infrastructure" }).click();
    await page.getByRole("button", { name: "Remove record" }).click();
    await app.dialog("Remove resource record").getByRole("button", { name: "Remove record" }).click();
    await mockApi.waitForRequest("DELETE", "/namespaces/default/applications/storefront/resources/aws-rds/postgres");
    await expect(page.getByText("PostgreSQL database record removed")).toBeVisible();
    await expect(page.getByTestId("infra-plugin-notice")).toHaveCount(0);
  });

  test("a failed provision keeps the dialog open with the plugin's events", async ({ app, page, mockApi }) => {
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

    await expect(dialog.getByRole("alert").filter({ hasText: "Plugin call failed" })).toContainText(
      "Plugin call failed: aws-rds.quota_exceeded: ElastiCache node quota reached",
    );
    await expect(dialog.getByTestId("plugin-event-list")).toContainText(
      "ElastiCache node quota reached (20 of 20 cache.m6g.large nodes).",
    );
    await expect(dialog.getByTestId("plugin-event-list")).toContainText(
      "Hint: Request a quota increase or pick a smaller node type.",
    );
  });

  test("a deployment created with plugin warnings stays on the page until the user continues", async ({
    app,
    page,
    mockApi,
    mockState,
  }) => {
    mockState.writePluginEvents = writePluginEvents();
    await app.goto("/deployments/create");
    await page.getByRole("textbox", { name: "Deployment Name", exact: true }).fill("checkout");
    await page.getByRole("textbox", { name: "Container Image", exact: true }).fill("ghcr.io/acme/checkout:1.4.0");
    await page.getByRole("button", { name: "Create Deployment" }).click();

    await mockApi.waitForRequest("POST", "/namespaces/default/deployments");
    const alert = page.getByTestId("create-plugin-events");
    await expect(alert).toContainText("Deployment checkout was created, but its plugins reported 1 error, 1 warning, 1 notice:");
    // Most severe first.
    await expect(alert.locator(".plugin-event")).toHaveCount(3);
    expect(await severities(alert)).toEqual(["error", "warning", "info"]);
    await expect(alert).toContainText("Secret storefront-db-credentials");
    await expect(page).toHaveURL(/\/deployments\/create$/);

    await alert.getByRole("button", { name: "Continue to deployment" }).click();
    await expect(page).toHaveURL(/\/deployments\/default\/checkout$/);
  });

  test("saving a deployment shows what its plugins reported", async ({ app, page, mockApi, mockState }) => {
    mockState.writePluginEvents = writePluginEvents().filter((e) => e.severity !== "error");
    await app.goto("/deployments/default/web");
    const labels = card(page, "Labels & Annotations");
    await labels.getByRole("button", { name: "Edit" }).click();
    await labels.getByRole("button", { name: "Save" }).click();

    await mockApi.waitForRequest("PUT", "/namespaces/default/deployments/web");
    const alert = page.getByTestId("update-plugin-events");
    await expect(alert).toContainText("Saved. The plugins reported 1 warning, 1 notice:");
    // The alert takes the colour of the worst event.
    await expect(alert).toHaveClass(/\btext-warning\b/);
    await alert.getByRole("button", { name: "Close" }).click();
    await expect(alert).toHaveCount(0);
  });

  test("a save with only notices shows nothing", async ({ app, page, mockApi, mockState }) => {
    mockState.writePluginEvents = writePluginEvents().filter((e) => e.severity === "info");
    await app.goto("/deployments/default/web");
    const labels = card(page, "Labels & Annotations");
    await labels.getByRole("button", { name: "Edit" }).click();
    await labels.getByRole("button", { name: "Save" }).click();
    await mockApi.waitForRequest("PUT", "/namespaces/default/deployments/web");
    await app.settle();
    await expect(page.getByTestId("update-plugin-events")).toHaveCount(0);
  });
});
