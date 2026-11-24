import type { Locator, Page } from "@playwright/test";
import type { App } from "../support/app";
import { eachTheme, expect, test } from "./visual-test";
import { card, field, openMenu } from "../support/ui";

/**
 * Deployment detail page (/deployments/:namespace/:name), the highest-risk
 * page of the Vuetify 3 -> 4 upgrade (#242): every card, the pod table, the
 * logs selects, and the dialogs it opens, including the GitOps dialog whose
 * select `#item` slots changed shape in Vuetify 4.
 */

const WEB = "/deployments/default/web";

const POD_METRICS = "/namespaces/:ns/pods/metrics";

/** Requests every deployment page makes on load. */
const ON_LOAD = [
  "/namespaces/:ns/deployments/:name",
  "/namespaces/:ns/deployments/:name/hpa",
  "/namespaces/:ns/deployments/:name/gitops",
  "/namespaces/:ns/deployments/:name/monitor",
  "/namespaces/:ns/deployments/:name/history",
  "/namespaces/:ns/events",
  "/namespaces/:ns/pods/:pod/logs/history",
  POD_METRICS,
];

/**
 * Open a deployment, wait for every request it makes on load (a quiet
 * network alone is not proof under load), then fire the 10 s metrics poll
 * once so the CPU/memory sparklines hold two identical samples and draw a
 * flat line instead of a ramp from zero. The poll's fetch can leave the page
 * after `runFor` returns, so wait for the request itself. The next real-time
 * poll is then about 10 s away, well after the screenshot.
 */
async function openDeployment(app: App, path = WEB): Promise<void> {
  await app.goto(path);
  for (const p of ON_LOAD) await app.api.waitForRequest("GET", p);
  await app.settle();
  const polls = app.api.requests("GET", POD_METRICS).length;
  await app.page.clock.runFor(10_000);
  await expect.poll(() => app.api.requests("GET", POD_METRICS).length).toBeGreaterThan(polls);
  await app.settle();
}

async function openGitOpsDialog(app: App, page: Page): Promise<Locator> {
  await card(page, "GitOps").getByRole("button", { name: "Edit" }).click();
  const dialog = page.getByRole("dialog").filter({ hasText: "Edit GitOps" });
  await expect(dialog.getByRole("combobox", { name: "Repository" })).toBeVisible();
  // Settings feed the three managed-resource selects.
  await expect(dialog.getByText("storefront", { exact: true })).toBeVisible();
  await app.settle();
  return dialog;
}

eachTheme(() => {
  test.describe("web (available)", () => {
    test("full page", async ({ app }) => {
      await openDeployment(app);
      await app.snap("deployment-detail/web", { fullPage: true });
    });

    test("pod row expanded", async ({ app, page }) => {
      await openDeployment(app);
      const pods = card(page, "Pods (3)");
      await pods.getByRole("row").filter({ hasText: "web-6c9f7d8b5-4xk2p" }).getByRole("button").first().click();
      await expect(pods.getByText("Containers (1)")).toBeVisible();
      await app.snap("deployment-detail/pods-expanded", { fullPage: true, locator: pods });
    });

    test("logs: history select open", async ({ app, page }) => {
      await openDeployment(app);
      const menu = await openMenu(card(page, "Logs"), "History");
      await expect(menu.getByRole("option", { name: "Last 100", exact: true })).toBeVisible();
      await app.snap("deployment-detail/logs-history-menu");
    });

    test("logs: pod select open", async ({ app, page }) => {
      await openDeployment(app);
      const menu = await openMenu(card(page, "Logs"), "Pod");
      await expect(menu.getByRole("option", { name: "web-6c9f7d8b5-mr7wd" })).toBeVisible();
      await app.snap("deployment-detail/logs-pod-menu");
    });

    test("labels and annotations editor", async ({ app, page }) => {
      await openDeployment(app);
      const labels = card(page, "Labels & Annotations");
      await labels.getByRole("button", { name: "Edit" }).click();
      await expect(labels.getByRole("button", { name: "Add annotation" })).toBeVisible();
      await app.snap("deployment-detail/labels-editor", { fullPage: true, locator: labels });
    });

    test("resource metrics details", async ({ app, page }) => {
      await openDeployment(app);
      await page.getByRole("button", { name: "View details" }).click();
      const metrics = card(page, "Resource Metrics");
      await expect(metrics.getByText("3 pods")).toBeVisible();
      await expect(metrics.locator("canvas")).toHaveCount(4);
      // The charts draw one x-axis label per sample: exactly two polls (mount
      // + the forced one) must have happened, or the axis would differ.
      expect(app.api.requests("GET", POD_METRICS)).toHaveLength(2);
      await app.snap("deployment-detail/metrics-details", { fullPage: true, locator: metrics });
    });

    test("gitops build history", async ({ app, page }) => {
      await openDeployment(app);
      const gitops = card(page, "GitOps");
      await gitops.getByRole("button", { name: "Build History" }).click();
      await expect(gitops.getByText("web-build-1a2b3c4")).toBeVisible();
      await app.snap("deployment-detail/gitops-build-history", { fullPage: true, locator: gitops });
    });
  });

  test.describe("ingress dialog", () => {
    test("add, with the default template applied", async ({ app, page }) => {
      await openDeployment(app);
      await card(page, "Ingresses").getByRole("button", { name: "Add" }).click();
      const dialog = page.getByRole("dialog").filter({ hasText: "Add Ingress" });
      await expect(dialog.getByLabel("Ingress Name")).toHaveValue("web-ingress");
      await expect(dialog.getByRole("textbox", { name: "Key" }).first()).toHaveValue("alb.ingress.kubernetes.io/scheme");
      await app.snap("deployment-detail/ingress-add");
    });

    test("template select open", async ({ app, page }) => {
      await openDeployment(app);
      await card(page, "Ingresses").getByRole("button", { name: "Add" }).click();
      const dialog = page.getByRole("dialog").filter({ hasText: "Add Ingress" });
      const menu = await openMenu(dialog, "Template");
      await expect(menu.getByRole("option", { name: "internal-nginx" })).toBeVisible();
      await app.snap("deployment-detail/ingress-template-menu");
    });

    test("path type select open", async ({ app, page }) => {
      await openDeployment(app);
      await card(page, "Ingresses").getByRole("button", { name: "Add" }).click();
      const dialog = page.getByRole("dialog").filter({ hasText: "Add Ingress" });
      const menu = await openMenu(dialog, "Type");
      await expect(menu.getByRole("option", { name: "Exact" })).toBeVisible();
      await app.snap("deployment-detail/ingress-path-type-menu");
    });

    test("ingress class combobox with a typed value", async ({ app, page }) => {
      await openDeployment(app);
      await card(page, "Ingresses").getByRole("button", { name: "Add" }).click();
      const dialog = page.getByRole("dialog").filter({ hasText: "Add Ingress" });
      await dialog.getByLabel("Hostname").fill("shop.example.com");
      const ingressClass = dialog.getByRole("combobox", { name: "Ingress Class" });
      await ingressClass.fill("traefik");
      await app.snap("deployment-detail/ingress-class-typed");
    });

    test("edit web-ingress", async ({ app, page }) => {
      await openDeployment(app);
      const ingresses = card(page, "Ingresses");
      await ingresses.getByRole("row").filter({ hasText: "web-ingress" }).locator("button:has(.mdi-pencil)").click();
      const dialog = page.getByRole("dialog").filter({ hasText: "Edit Ingress" });
      await expect(dialog.getByLabel("Hostname")).toHaveValue("shop.example.com");
      await app.snap("deployment-detail/ingress-edit");
    });
  });

  test.describe("gitops dialog", () => {
    test("edit", async ({ app, page }) => {
      await openDeployment(app);
      await openGitOpsDialog(app, page);
      await app.snap("deployment-detail/gitops-edit");
    });

    test("repository select open", async ({ app, page }) => {
      await openDeployment(app);
      const dialog = await openGitOpsDialog(app, page);
      const menu = await openMenu(dialog, "Repository");
      await expect(menu.getByText("https://github.com/acme/platform-api.git")).toBeVisible();
      await app.snap("deployment-detail/gitops-repository-menu");
    });

    test("git token select open", async ({ app, page }) => {
      await openDeployment(app);
      const dialog = await openGitOpsDialog(app, page);
      const menu = await openMenu(dialog, "Git Token");
      await expect(menu.getByText("deckwatch/gitlab-ci-token")).toBeVisible();
      await app.snap("deployment-detail/gitops-token-menu");
    });

    test("oci registry select open", async ({ app, page }) => {
      await openDeployment(app);
      const dialog = await openGitOpsDialog(app, page);
      const menu = await openMenu(dialog, "OCI Registry");
      await expect(menu.getByText("ghcr · ghcr.io/acme")).toBeVisible();
      await app.snap("deployment-detail/gitops-registry-menu");
    });

    test("custom repository URL", async ({ app, page }) => {
      await openDeployment(app);
      const dialog = await openGitOpsDialog(app, page);
      const menu = await openMenu(dialog, "Repository");
      await menu.getByRole("option", { name: /Custom URL/ }).click();
      await expect(dialog.getByLabel("Custom repository URL")).toBeVisible();
      await app.snap("deployment-detail/gitops-custom-repository");
    });

    test("branch autocomplete open", async ({ app, page }) => {
      await openDeployment(app);
      const dialog = await openGitOpsDialog(app, page);
      await field(dialog, "Branch").click();
      const menu = page.locator(".v-overlay--active .v-list").last();
      await expect(menu.getByRole("option", { name: "release/2026.06" })).toBeVisible();
      await app.snap("deployment-detail/gitops-branch-menu");
    });

    test("webhook section, scrolled into view", async ({ app, page }) => {
      await openDeployment(app);
      const dialog = await openGitOpsDialog(app, page);
      const generate = dialog.getByRole("button", { name: "Generate secret" });
      await generate.scrollIntoViewIfNeeded();
      await expect(dialog.getByLabel("Webhook URL")).toHaveValue(/\/api\/webhooks\/gitops$/);
      await app.snap("deployment-detail/gitops-webhook");
    });

    test("enable on a deployment without gitops", async ({ app, page }) => {
      await openDeployment(app, "/deployments/default/api");
      await card(page, "GitOps").getByRole("button", { name: "Enable" }).click();
      const dialog = page.getByRole("dialog").filter({ hasText: "Enable GitOps" });
      await expect(dialog.getByRole("button", { name: "Save" })).toBeDisabled();
      await app.snap("deployment-detail/gitops-enable");
    });

    test("browse registry on top of the dialog", async ({ app, page }) => {
      await openDeployment(app);
      const dialog = await openGitOpsDialog(app, page);
      // The web config points at a URL that is not a managed registry, so the
      // dialog shows the custom field with its Browse button.
      await dialog.locator("button:has(.mdi-image-search)").click();
      const picker = page.getByRole("dialog").filter({ hasText: "Browse Registry" });
      await picker.getByText("acme/platform-api", { exact: true }).click();
      await expect(picker.getByRole("cell", { name: "v2.4.1" })).toBeVisible();
      await app.snap("deployment-detail/gitops-browse-registry");
    });
  });

  test.describe("other dialogs", () => {
    test("compare revisions", async ({ app, page }) => {
      await openDeployment(app);
      await card(page, "Rollout History").getByRole("button", { name: "Compare" }).click();
      const dialog = page.getByRole("dialog").filter({ hasText: "Compare Revisions" });
      await expect(dialog.getByText(/lines? differ/)).toBeVisible();
      await app.snap("deployment-detail/compare-revisions");
    });

    test("affinity editor", async ({ app, page }) => {
      await openDeployment(app);
      await page.getByRole("button", { name: "Affinity" }).click();
      const dialog = page.getByRole("dialog").filter({ hasText: "Node Affinity & Scheduling" });
      await expect(dialog.getByRole("textbox", { name: "Key" }).first()).toHaveValue("kubernetes.io/os");
      await app.snap("deployment-detail/affinity-editor");
    });

    test("add sidecar", async ({ app, page }) => {
      await openDeployment(app);
      await card(page, "Containers").getByRole("button", { name: "Add Sidecar" }).click();
      const dialog = page.getByRole("dialog").filter({ hasText: "Add Sidecar Container" });
      await dialog.getByLabel("Name", { exact: true }).fill("metrics-exporter");
      await dialog.getByLabel("Image", { exact: true }).fill("nginx/nginx-prometheus-exporter:1.3");
      await app.snap("deployment-detail/add-sidecar");
    });

    test("edit deployment: form with autoscaling", async ({ app, page }) => {
      await openDeployment(app);
      await page.getByRole("button", { name: "Edit" }).first().click();
      const dialog = page.getByRole("dialog").filter({ hasText: "Edit Deployment" });
      await expect(dialog.getByLabel("Container Image")).toHaveValue("nginx:1.25");
      await expect(dialog.getByLabel("Max Replicas")).toHaveValue("6");
      await app.snap("deployment-detail/edit-form");
    });

    test("edit deployment: autoscaling section", async ({ app, page }) => {
      await openDeployment(app);
      await page.getByRole("button", { name: "Edit" }).first().click();
      const dialog = page.getByRole("dialog").filter({ hasText: "Edit Deployment" });
      await dialog.getByLabel("Target Memory %").scrollIntoViewIfNeeded();
      await app.snap("deployment-detail/edit-autoscaling");
    });

    test("edit deployment: yaml", async ({ app, page }) => {
      await openDeployment(app);
      await page.getByRole("button", { name: "Edit" }).first().click();
      const dialog = page.getByRole("dialog").filter({ hasText: "Edit Deployment" });
      await dialog.getByRole("button", { name: "YAML" }).click();
      await expect(dialog.locator("textarea")).toHaveValue(/kind: Deployment/);
      await app.snap("deployment-detail/edit-yaml");
    });

    test("view yaml", async ({ app, page }) => {
      await openDeployment(app);
      await page.getByRole("button", { name: "View YAML" }).click();
      const dialog = page.getByRole("dialog").filter({ hasText: "web — YAML" });
      await expect(dialog.getByText("kind: Deployment")).toBeVisible();
      await app.snap("deployment-detail/view-yaml");
    });

    test("monitor edit", async ({ app, page }) => {
      await openDeployment(app);
      await card(page, "Prometheus Monitor").getByRole("button", { name: "Edit" }).click();
      const dialog = page.getByRole("dialog").filter({ hasText: "Edit Monitor" });
      await expect(dialog.getByLabel("Scrape interval")).toHaveValue("30s");
      await app.snap("deployment-detail/monitor-edit");
    });

    test("restart confirm", async ({ app, page }) => {
      await openDeployment(app);
      await page.getByRole("button", { name: "Restart" }).click();
      await expect(page.getByRole("dialog").getByText("This will trigger a rolling restart of 'web'.")).toBeVisible();
      await app.snap("deployment-detail/restart-confirm");
    });

    test("delete confirm with the name typed", async ({ app, page }) => {
      await openDeployment(app);
      await page.getByRole("button", { name: "Delete" }).click();
      const dialog = page.getByRole("dialog").filter({ hasText: "Delete Deployment" });
      await dialog.getByRole("textbox").fill("web");
      await expect(dialog.getByRole("button", { name: "Delete" })).toBeEnabled();
      await app.snap("deployment-detail/delete-confirm");
    });

    test("diagnose with ai: agent chooser", async ({ app, page }) => {
      await openDeployment(app);
      await page.getByRole("button", { name: "Diagnose with AI" }).click();
      await expect(page.getByRole("dialog").filter({ hasText: "Choose an AI agent" })).toBeVisible();
      await app.snap("deployment-detail/diagnose-chooser");
    });

    test("diagnose with ai: result", async ({ app, page, mockApi }) => {
      await openDeployment(app);
      await page.getByRole("button", { name: "Diagnose with AI" }).click();
      await page.getByRole("dialog").filter({ hasText: "Choose an AI agent" }).getByText("Claude", { exact: true }).click();
      await mockApi.waitForRequest("GET", "/namespaces/default/diagnostics/:job/stream");
      const result = page.locator(".v-card").filter({ hasText: /AI Diagnostic/ }).last();
      await expect(result.getByText("Root cause: the upstream API times out after 60s.")).toBeVisible();
      await expect(result.getByText("succeeded")).toBeVisible();
      await app.snap("deployment-detail/diagnose-result", { fullPage: true, locator: card(page, "Logs") });
    });
  });

  test.describe("worker (degraded) and importer (failed)", () => {
    test("worker: full page", async ({ app }) => {
      await openDeployment(app, "/deployments/default/worker");
      await app.snap("deployment-detail/worker", { fullPage: true });
    });

    test("worker: diagnostics drawer", async ({ app, page }) => {
      await openDeployment(app, "/deployments/default/worker");
      await page.locator(".v-card").first().getByText("Degraded").click();
      const drawer = page.locator(".v-navigation-drawer").filter({ hasText: "Deployment Diagnostics" });
      await expect(drawer).toBeInViewport();
      await expect(drawer.getByText("CrashLoopBackOff")).toBeVisible();
      await app.snap("deployment-detail/worker-diagnostics-drawer");
    });

    test("importer: full page", async ({ app }) => {
      await openDeployment(app, "/deployments/default/importer");
      await app.snap("deployment-detail/importer", { fullPage: true });
    });
  });
});

test.describe("responsive", () => {
  for (const width of [1200, 900]) {
    test.describe(`${width}px wide`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("web: full page", async ({ app }) => {
        await openDeployment(app);
        await app.snap("deployment-detail/web", { fullPage: true });
      });
    });
  }
});
