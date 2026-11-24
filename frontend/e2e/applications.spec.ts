import { expect, test } from "./fixtures";
import { openMenu } from "./support/ui";

test.describe("Applications", () => {
  test("clicking a row opens the application", async ({ app, page }) => {
    await app.goto("/applications");
    await page.getByRole("cell", { name: "billing", exact: true }).click();
    await expect(page).toHaveURL(/\/applications\/default\/billing$/);
    await expect(page.getByRole("heading", { name: "billing" })).toBeVisible();
  });

  test.describe("create wizard", () => {
    test("creates the application and its ingress", async ({ app, page, mockApi }) => {
      await app.goto("/applications/create");
      const next = page.getByRole("button", { name: "Next" });

      await page.getByLabel("App name").fill("checkout");
      await page.getByLabel("Description (optional)").fill("Checkout flow");
      await next.click();

      await page.getByText("I have a Git repository").click();
      await page.getByLabel("Repository URL").fill("https://github.com/acme/checkout.git");
      await page.getByLabel("Branch").fill("release");
      await page.getByLabel("Access token secret name").fill("github-bot-token");
      await next.click();

      await page.getByLabel("Hostname", { exact: true }).fill("checkout.example.com");
      await page.getByLabel("Path").fill("/shop");
      await page.getByLabel("Service port").fill("8080");
      await next.click();

      const review = page.locator(".v-stepper");
      await expect(review.getByText("http://checkout.example.com/shop")).toBeVisible();
      await page.getByRole("button", { name: "Create Application" }).click();

      const create = await mockApi.waitForRequest("POST", "/namespaces/default/applications");
      expect(create.body).toEqual({
        name: "checkout",
        description: "Checkout flow",
        git: { repo_url: "https://github.com/acme/checkout.git", branch: "release", token_secret: "github-bot-token" },
        create_deployment: true,
      });
      const ingress = await mockApi.waitForRequest("POST", "/namespaces/default/ingresses");
      expect(ingress.body).toEqual({
        name: "checkout",
        host: "checkout.example.com",
        paths: [{ path: "/shop", path_type: "Prefix", service_name: "checkout", service_port: 8080 }],
      });
      // The ingress is created after the application.
      expect(mockApi.calls.indexOf(create)).toBeLessThan(mockApi.calls.indexOf(ingress));
      await expect(page).toHaveURL(/\/applications\/default\/checkout$/);
    });

    test("manual and internal-only apps send no git config and no ingress", async ({ app, page, mockApi }) => {
      await app.goto("/applications/create");
      const next = page.getByRole("button", { name: "Next" });

      await page.getByLabel("App name").fill("batch-jobs");
      await openMenu(page.locator(".v-stepper"), "Namespace");
      await page.getByRole("option", { name: "staging" }).click();
      await next.click();
      await next.click(); // "I will deploy manually" is the default
      await page.getByText("Internal only (no public URL)").click();
      await next.click();
      await expect(page.locator(".v-stepper").getByText("Internal only (no ingress)")).toBeVisible();
      await page.getByRole("button", { name: "Create Application" }).click();

      const create = await mockApi.waitForRequest("POST", "/namespaces/staging/applications");
      expect(create.body).toEqual({ name: "batch-jobs", create_deployment: true });
      await expect(page).toHaveURL(/\/applications\/staging\/batch-jobs$/);
      expect(mockApi.requests("POST", "/namespaces/:ns/ingresses")).toHaveLength(0);
    });

    test("Next stays disabled until the step is valid", async ({ app, page }) => {
      await app.goto("/applications/create");
      const next = page.getByRole("button", { name: "Next" });
      await expect(next).toBeDisabled();

      await page.getByLabel("App name").fill("Bad_Name");
      await expect(page.getByText("Resource names cannot contain underscores", { exact: false })).toBeVisible();
      await expect(next).toBeDisabled();

      await page.getByLabel("App name").fill("good-name");
      await expect(next).toBeEnabled();
      await next.click();

      await page.getByText("I have a Git repository").click();
      await expect(next).toBeDisabled();
      await page.getByLabel("Repository URL").fill("https://github.com/acme/good.git");
      await expect(next).toBeEnabled();
      await next.click();

      await expect(page.getByText("Hostname is required for a public URL.")).toBeVisible();
      await expect(next).toBeDisabled();
      await page.getByLabel("Hostname", { exact: true }).fill("not a host");
      await expect(page.getByText("Enter a valid hostname (e.g. myapp.example.com).")).toBeVisible();
      await page.getByLabel("Path").fill("shop");
      await expect(page.getByText("Path must start with '/'.")).toBeVisible();
      await expect(next).toBeDisabled();
    });
  });

  test.describe("detail page", () => {
    test("Edit saves the description and git settings", async ({ app, page, mockApi }) => {
      await app.goto("/applications/default/billing");
      await page.getByRole("button", { name: "Edit" }).click();

      const dialog = app.dialog("Edit Application");
      await dialog.getByLabel("Description").fill("Invoices and payment reconciliation.");
      await dialog.getByLabel("Connect a Git repository").check();
      await dialog.getByLabel("Repository URL").fill("https://github.com/acme/billing.git");
      await dialog.getByLabel("Access token secret name").fill("github-bot-token");
      await dialog.getByRole("button", { name: "Save" }).click();

      const call = await mockApi.waitForRequest("PUT", "/namespaces/default/applications/billing");
      expect(call.body).toEqual({
        description: "Invoices and payment reconciliation.",
        git: { repo_url: "https://github.com/acme/billing.git", branch: "main", token_secret: "github-bot-token" },
      });
      await expect(dialog).toBeHidden();
      await expect(page.getByText("Invoices and payment reconciliation.")).toBeVisible();
    });

    test("Delete with cascade sends cascade=true and returns to the list", async ({ app, page, mockApi }) => {
      await app.goto("/applications/default/storefront");
      await page.getByRole("button", { name: "Delete" }).click();

      const dialog = app.dialog("Delete Application");
      const confirm = dialog.getByRole("button", { name: "Delete" });
      await dialog.getByLabel("Also delete all member resources (deployments and cronjobs)").check();
      await expect(confirm).toBeDisabled();
      await dialog.getByRole("textbox").fill("storefront");
      await confirm.click();

      const call = await mockApi.waitForRequest("DELETE", "/namespaces/default/applications/storefront");
      expect(call.query.get("cascade")).toBe("true");
      await expect(page).toHaveURL(/\/applications$/);
      await expect(page.getByRole("cell", { name: "storefront", exact: true })).toBeHidden();
    });

    test("Delete without cascade sends cascade=false", async ({ app, page, mockApi }) => {
      await app.goto("/applications/default/sandbox");
      await page.getByRole("button", { name: "Delete" }).click();
      const dialog = app.dialog("Delete Application");
      await dialog.getByRole("textbox").fill("sandbox");
      await dialog.getByRole("button", { name: "Delete" }).click();

      const call = await mockApi.waitForRequest("DELETE", "/namespaces/default/applications/sandbox");
      expect(call.query.get("cascade")).toBe("false");
    });

    test("Add Resource lists unassigned resources and posts the member", async ({ app, page, mockApi }) => {
      await app.goto("/applications/default/storefront");
      await page.getByRole("button", { name: "Add Resource" }).click();

      const dialog = app.dialog("Add Resource to Application");
      // Resources already labelled for an application are not offered.
      await expect(dialog.getByRole("cell", { name: "web", exact: true })).toBeHidden();
      await expect(dialog.getByRole("cell", { name: "nightly-backup", exact: true })).toBeHidden();
      await dialog.getByRole("row").filter({ hasText: "cache-warmer" }).getByRole("button", { name: "Add" }).click();

      const call = await mockApi.waitForRequest("POST", "/namespaces/default/applications/storefront/members");
      expect(call.body).toEqual({ kind: "CronJob", resource_name: "cache-warmer" });
      await dialog.getByRole("button", { name: "Done" }).click();
      await expect(page.getByText("CronJobs (2)")).toBeVisible();
    });

    test("the unlink button removes a member", async ({ app, page, mockApi }) => {
      await app.goto("/applications/default/storefront");
      await page.getByRole("row").filter({ hasText: "ghcr.io/acme/platform-api" }).getByRole("button").click();

      await mockApi.waitForRequest("DELETE", "/namespaces/default/applications/storefront/members/Deployment/api");
      await expect(page.getByText("Deployments (1)")).toBeVisible();
    });

    test("Associate posts the plugin and confirms with a snackbar", async ({ app, page, mockApi }) => {
      await app.goto("/applications/default/billing");
      await page.getByRole("tab", { name: "Plugins" }).click();
      await expect(page.getByText('No plugins associated with this application.', { exact: false })).toBeVisible();
      await page.getByRole("button", { name: "Add Plugin" }).click();

      const dialog = app.dialog("Add Plugin");
      await expect(dialog.getByRole("button", { name: "Associate" })).toBeDisabled();
      await openMenu(dialog, "Plugin");
      await page.getByRole("option", { name: "otel" }).click();
      await dialog.getByRole("button", { name: "Associate" }).click();

      const call = await mockApi.waitForRequest("POST", "/namespaces/default/applications/billing/plugins/otel");
      expect(call.body).toBeNull();
      await expect(page.getByText('Plugin "otel" associated')).toBeVisible();
      await expect(page.locator(".v-card").filter({ hasText: "otel" }).getByText("loaded", { exact: true })).toBeVisible();
    });

    test("Provision posts the resource fields", async ({ app, page, mockApi }) => {
      await app.goto("/applications/default/storefront");
      await page.getByRole("tab", { name: "Infrastructure" }).click();
      await page.getByRole("button", { name: "Provision" }).click();

      const dialog = app.dialog("Add Redis cache");
      await openMenu(dialog, "Node type");
      await page.getByRole("option", { name: "cache.m6g.large" }).click();
      await dialog.getByRole("button", { name: "Provision" }).click();

      const call = await mockApi.waitForRequest("POST", "/namespaces/default/applications/storefront/resources/aws-rds/redis");
      expect(call.body).toEqual({ fields: { node_type: "cache.m6g.large" } });
      await expect(page.getByText("Redis cache provisioned")).toBeVisible();
    });

    test("Fix with AI asks for an agent and shows the queued notice", async ({ app, page }) => {
      await app.goto("/applications/default/storefront");
      await page.getByRole("button", { name: "Fix with AI" }).click();
      const dialog = app.dialog("Choose an AI agent");
      await dialog.getByText("Claude", { exact: true }).click();

      await expect(dialog).toBeHidden();
      await expect(page.getByRole("alert").filter({ hasText: "Queued a fix with Claude" })).toBeVisible();
      expect(await page.evaluate(() => sessionStorage.getItem("deckwatch-ai-agent"))).toBe("claude");
    });
  });
});
