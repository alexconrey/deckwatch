import { expect, test } from "./fixtures";
import { card, choose, openMenu } from "./support/ui";

/**
 * Functional checks for the deployment detail page's cards and dialogs:
 * each flow asserts the exact request the UI sends. Rendering is covered by
 * visual/deployment-detail.spec.ts.
 */

const WEB = "/deployments/default/web";

test.describe("Deployment detail: cards", () => {
  test("labels editor saves pod labels and annotations", async ({ page, mockApi }) => {
    await page.goto(WEB);
    const labels = card(page, "Labels & Annotations");
    await labels.getByRole("button", { name: "Edit" }).click();

    await labels.getByRole("textbox", { name: "Value" }).nth(1).fill("edge");
    await labels.getByRole("button", { name: "Add annotation" }).click();
    await labels.getByRole("textbox", { name: "Key" }).last().fill(" owner ");
    await labels.getByRole("textbox", { name: "Value" }).last().fill("team-shop");
    await labels.getByRole("button", { name: "Save" }).click();

    const call = await mockApi.waitForRequest("PUT", "/namespaces/default/deployments/web");
    expect(call.body).toEqual({
      pod_labels: { app: "web", tier: "edge" },
      pod_annotations: { "prometheus.io/scrape": "true", owner: "team-shop" },
    });
    await expect(labels.getByText("tier=edge")).toBeVisible();
    await expect(labels.getByText("owner=team-shop")).toBeVisible();
  });

  test("a pod row expands to its containers", async ({ page }) => {
    await page.goto(WEB);
    const pods = card(page, "Pods (3)");
    await expect(pods.getByText("Containers (1)")).toBeHidden();
    await pods.getByRole("row").filter({ hasText: "web-6c9f7d8b5-4xk2p" }).getByRole("button").first().click();

    await expect(pods.getByText("Containers (1)")).toBeVisible();
    const container = pods.getByRole("row").filter({ hasText: "nginx:1.25" });
    await expect(container.getByText("web", { exact: true })).toBeVisible();
    await expect(container.getByText("running")).toBeVisible();
  });

  test("the logs history select refetches with tail_lines", async ({ page, mockApi }) => {
    await page.goto(WEB);
    const history = "/namespaces/default/pods/web-6c9f7d8b5-4xk2p/logs/history";
    await mockApi.waitForRequest("GET", history);
    await expect(card(page, "Logs").getByText("24 / 24 lines")).toBeVisible();

    await choose(card(page, "Logs"), "History", "Last 100");
    await expect
      .poll(() => mockApi.requests("GET", history).map((c) => c.query.get("tail_lines")))
      .toContain("100");
    // The live stream reconnects too; it is held open by the mocks.
    expect(mockApi.requests("GET", "/namespaces/default/pods/web-6c9f7d8b5-4xk2p/logs").length).toBeGreaterThan(0);
  });

  test("build history is fetched when expanded", async ({ page, mockApi }) => {
    await page.goto(WEB);
    const gitops = card(page, "GitOps");
    await expect(gitops.getByText("success", { exact: true })).toBeVisible();
    expect(mockApi.requests("GET", "/namespaces/:ns/deployments/:name/gitops/builds")).toHaveLength(0);

    await gitops.getByRole("button", { name: "Build History" }).click();
    await mockApi.waitForRequest("GET", "/namespaces/default/deployments/web/gitops/builds");
    await expect(gitops.getByRole("row")).toHaveCount(4); // header + 3 builds
    await expect(gitops.getByRole("row").filter({ hasText: "web-build-1a2b3c4" }).getByText("failed")).toBeVisible();
  });

  test("rolling back posts the chosen revision", async ({ page, mockApi }) => {
    await page.goto(WEB);
    const history = card(page, "Rollout History");
    await history.getByRole("row").filter({ hasText: "#6" }).getByRole("button", { name: "Roll back" }).click();

    const dialog = page.getByRole("dialog").filter({ hasText: "Roll back deployment" });
    await expect(dialog.getByText("Roll web back to revision #6 (nginx:1.24)?", { exact: false })).toBeVisible();
    await dialog.getByRole("button", { name: "Roll back" }).click();

    const call = await mockApi.waitForRequest("POST", "/namespaces/default/deployments/web/rollback");
    expect(call.body).toEqual({ revision: 6 });
    await expect(dialog).toBeHidden();
  });

  test("the degraded status chip opens the diagnostics drawer, and clicking outside closes it", async ({ page }) => {
    await page.goto("/deployments/default/worker");
    const drawer = page.locator(".v-navigation-drawer").filter({ hasText: "Deployment Diagnostics" });
    await expect(drawer).not.toBeInViewport();

    await page.locator(".v-card").first().getByText("Degraded").click();
    await expect(drawer).toBeInViewport();
    await expect(drawer.getByText("MinimumReplicasUnavailable")).toBeVisible();
    await expect(drawer.getByText("CrashLoopBackOff")).toBeVisible();
    await expect(drawer.getByText("Your app is crashing on startup.", { exact: false })).toBeVisible();

    // The temporary drawer's scrim covers the rest of the page.
    await page.mouse.click(200, 450);
    await expect(drawer).not.toBeInViewport();
  });
});

test.describe("Deployment detail: ingress", () => {
  test("add posts the form with the default template applied", async ({ page, mockApi }) => {
    await page.goto(WEB);
    await card(page, "Ingresses").getByRole("button", { name: "Add" }).click();
    await mockApi.waitForRequest("GET", "/ingress-templates");
    const dialog = page.getByRole("dialog").filter({ hasText: "Add Ingress" });
    await dialog.getByLabel("Hostname").fill("shop-v2.example.com");
    await dialog.getByRole("button", { name: "Create" }).click();

    const call = await mockApi.waitForRequest("POST", "/namespaces/default/ingresses");
    expect(call.body).toEqual({
      name: "web-ingress",
      host: "shop-v2.example.com",
      // The port comes from the deployment's probes (8080).
      paths: [{ path: "/", path_type: "Prefix", service_name: "web", service_port: 8080 }],
      ingress_class: "alb",
      annotations: {
        "alb.ingress.kubernetes.io/scheme": "internet-facing",
        "alb.ingress.kubernetes.io/target-type": "ip",
      },
      template: "public-alb",
    });
    await expect(dialog).toBeHidden();
  });

  test("the class combobox accepts a typed value and the path type select works", async ({ page, mockApi }) => {
    await page.goto(WEB);
    await card(page, "Ingresses").getByRole("button", { name: "Add" }).click();
    const dialog = page.getByRole("dialog").filter({ hasText: "Add Ingress" });
    await choose(dialog, "Template", "None");
    await choose(dialog, "Type", "Exact");
    await dialog.getByRole("combobox", { name: "Ingress Class" }).fill("traefik");
    await dialog.getByLabel("Path").fill("/shop");
    await dialog.getByRole("button", { name: "Create" }).click();

    const call = await mockApi.waitForRequest("POST", "/namespaces/default/ingresses");
    expect(call.body).toEqual({
      name: "web-ingress",
      paths: [{ path: "/shop", path_type: "Exact", service_name: "web", service_port: 8080 }],
      ingress_class: "traefik",
    });
  });

  test("delete asks for confirmation and sends DELETE", async ({ page, mockApi }) => {
    await page.goto(WEB);
    await card(page, "Ingresses").getByRole("row").filter({ hasText: "web-ingress" }).locator("button:has(.mdi-delete)").click();

    const dialog = page.getByRole("dialog").filter({ hasText: "Delete Ingress" });
    await expect(dialog.getByText("Delete ingress 'web-ingress'? This will remove the routing rule.")).toBeVisible();
    await dialog.getByRole("button", { name: "Delete" }).click();
    await mockApi.waitForRequest("DELETE", "/namespaces/default/ingresses/web-ingress");
    await expect(dialog).toBeHidden();
  });
});

test.describe("Deployment detail: GitOps dialog", () => {
  test("every managed repository, token and registry option shows its title and subtitle", async ({ page }) => {
    await page.goto(WEB);
    await card(page, "GitOps").getByRole("button", { name: "Edit" }).click();
    const dialog = page.getByRole("dialog").filter({ hasText: "Edit GitOps" });

    const expectOptions = async (label: string, options: [string, string][]) => {
      const menu = await openMenu(dialog, label);
      const items = menu.getByRole("option");
      await expect(items).toHaveCount(options.length);
      for (const [i, [title, subtitle]] of options.entries()) {
        await expect(items.nth(i)).toContainText(title);
        await expect(items.nth(i)).toContainText(subtitle);
      }
      await page.keyboard.press("Escape");
      await expect(menu).toBeHidden();
    };

    await expectOptions("Repository", [
      ["storefront", "https://github.com/acme/storefront.git"],
      ["platform-api", "https://github.com/acme/platform-api.git"],
      ["Custom URL…", "Type your own"],
    ]);
    await expectOptions("Git Token", [
      ["None (public repo)", ""],
      ["github-bot", "deckwatch/github-bot-token"],
      ["gitlab-ci", "deckwatch/gitlab-ci-token"],
    ]);
    await expectOptions("OCI Registry", [
      ["deckwatch", "deckwatch · https://registry.deckwatch.local/"],
      ["ecr-prod", "ecr · 123456789012.dkr.ecr.us-east-1.amazonaws.com"],
      ["ghcr", "ghcr · ghcr.io/acme"],
      ["Custom URL…", "Type your own"],
    ]);
  });

  test("Custom URL… reveals the repository URL field", async ({ page }) => {
    await page.goto(WEB);
    await card(page, "GitOps").getByRole("button", { name: "Edit" }).click();
    const dialog = page.getByRole("dialog").filter({ hasText: "Edit GitOps" });
    await expect(dialog.getByLabel("Custom repository URL")).toBeHidden();

    await choose(dialog, "Repository", /Custom URL/);
    await expect(dialog.getByLabel("Custom repository URL")).toHaveValue("https://github.com/acme/storefront.git");
  });

  test("the branch field lists the repository's branches through the selected token", async ({ page, mockApi }) => {
    await page.goto(WEB);
    await card(page, "GitOps").getByRole("button", { name: "Edit" }).click();
    const dialog = page.getByRole("dialog").filter({ hasText: "Edit GitOps" });
    await expect(dialog.getByText("github-bot", { exact: true })).toBeVisible();

    // Focusing the field lists the branches.
    const branch = dialog.getByRole("combobox", { name: "Branch" });
    await branch.click();
    const call = await mockApi.waitForRequest("GET", "/git/branches");
    expect(Object.fromEntries(call.query)).toEqual({
      repo_url: "https://github.com/acme/storefront.git",
      token_secret: "github-bot",
      namespace: "default",
    });
    // Typing filters them. The focused input may already hold the current
    // branch, so replace its text.
    await branch.fill("rel");
    const menu = page.locator(".v-overlay--active .v-list").last();
    await expect(menu.getByRole("option")).toHaveText(["release/2026.06"]);
  });

  test("enabling GitOps puts the selected managed resources", async ({ page, mockApi }) => {
    await page.goto("/deployments/default/api");
    await card(page, "GitOps").getByRole("button", { name: "Enable" }).click();
    const dialog = page.getByRole("dialog").filter({ hasText: "Enable GitOps" });
    const save = dialog.getByRole("button", { name: "Save" });
    await expect(save).toBeDisabled();

    await choose(dialog, "Repository", /platform-api/);
    await choose(dialog, "Git Token", /github-bot/);
    await choose(dialog, "OCI Registry", /^ghcr/);
    await expect(save).toBeEnabled();
    await save.click();

    const call = await mockApi.waitForRequest("PUT", "/namespaces/default/deployments/api/gitops");
    expect(call.body).toEqual({
      repo_url: "https://github.com/acme/platform-api.git",
      branch: "main",
      token_secret: "github-bot-token",
      dockerfile_path: "Dockerfile",
      docker_context: ".",
      oci_repository: "ghcr.io/acme",
      poll_interval_seconds: 60,
      webhook_enabled: false,
    });
    await expect(dialog).toBeHidden();
    await expect(card(page, "GitOps").getByText("Awaiting first build")).toBeVisible();
  });

  test("editing keeps the stored config and sends the new branch", async ({ page, mockApi }) => {
    await page.goto(WEB);
    await card(page, "GitOps").getByRole("button", { name: "Edit" }).click();
    const dialog = page.getByRole("dialog").filter({ hasText: "Edit GitOps" });
    await choose(dialog, "Branch", "develop");
    await dialog.getByRole("button", { name: "Save" }).click();

    const call = await mockApi.waitForRequest("PUT", "/namespaces/default/deployments/web/gitops");
    expect(call.body).toEqual({
      repo_url: "https://github.com/acme/storefront.git",
      branch: "develop",
      token_secret: "github-bot-token",
      dockerfile_path: "web/Dockerfile",
      docker_context: "web",
      oci_repository: "registry.deckwatch.local/web",
      poll_interval_seconds: 60,
      webhook_enabled: true,
      include_paths: ["web/**"],
      exclude_paths: ["**/*.md"],
    });
  });
});

test.describe("Deployment detail: other dialogs", () => {
  test("affinity editor puts node selector and affinity", async ({ page, mockApi }) => {
    await page.goto(WEB);
    await page.getByRole("button", { name: "Affinity" }).click();
    const dialog = page.getByRole("dialog").filter({ hasText: "Node Affinity & Scheduling" });
    await dialog.getByRole("button", { name: "Add selector" }).click();
    await dialog.getByRole("textbox", { name: "Key" }).nth(1).fill("node-pool");
    await dialog.getByRole("textbox", { name: "Value", exact: true }).nth(1).fill("general");
    await dialog.getByRole("button", { name: "Save" }).click();

    const call = await mockApi.waitForRequest("PUT", "/namespaces/default/deployments/web");
    expect(call.body).toEqual({
      node_selector: { "kubernetes.io/os": "linux", "node-pool": "general" },
      node_affinity: {
        required: [
          { match_expressions: [{ key: "topology.kubernetes.io/zone", operator: "In", values: ["us-east-1a", "us-east-1b"] }] },
        ],
        preferred: [
          { weight: 50, match_expressions: [{ key: "node.kubernetes.io/instance-type", operator: "In", values: ["m6i.large"] }] },
        ],
      },
    });
    await expect(dialog).toBeHidden();
  });

  test("add sidecar posts the container", async ({ page, mockApi }) => {
    await page.goto(WEB);
    await card(page, "Containers").getByRole("button", { name: "Add Sidecar" }).click();
    const dialog = page.getByRole("dialog").filter({ hasText: "Add Sidecar Container" });
    const add = dialog.getByRole("button", { name: "Add" });
    await expect(add).toBeDisabled();

    await dialog.getByLabel("Name", { exact: true }).fill("metrics-exporter");
    await dialog.getByLabel("Image", { exact: true }).fill("nginx/nginx-prometheus-exporter:1.3");
    await dialog.getByLabel("Port (optional)").fill("9113");
    await dialog.getByLabel("Args (space-separated, optional)").fill("--nginx.scrape-uri  http://localhost:8080/stub_status");
    await dialog.getByLabel("Env (KEY=val, comma-separated)").fill("LOG_LEVEL=info, MODE=proxy");
    await dialog.getByLabel("CPU Request").fill("50m");
    await add.click();

    const call = await mockApi.waitForRequest("POST", "/namespaces/default/deployments/web/containers");
    expect(call.body).toEqual({
      name: "metrics-exporter",
      image: "nginx/nginx-prometheus-exporter:1.3",
      port: 9113,
      args: ["--nginx.scrape-uri", "http://localhost:8080/stub_status"],
      env: [
        { name: "LOG_LEVEL", value: "info" },
        { name: "MODE", value: "proxy" },
      ],
      resource_requests: { cpu: "50m", memory: null },
    });
    await expect(dialog).toBeHidden();
  });

  test("the edit form puts the deployment, then the autoscaler", async ({ page, mockApi }) => {
    await page.goto(WEB);
    await page.getByRole("button", { name: "Edit" }).first().click();
    const dialog = page.getByRole("dialog").filter({ hasText: "Edit Deployment" });
    // Prefilled from the existing HPA (min 2, max 6, CPU 70%).
    await expect(dialog.getByLabel("Enable Horizontal Pod Autoscaler")).toBeChecked();

    await dialog.getByLabel("Container Image").fill("nginx:1.27");
    await dialog.getByLabel("Max Replicas").fill("8");
    await dialog.getByRole("button", { name: "Update Deployment" }).click();

    const put = await mockApi.waitForRequest("PUT", "/namespaces/default/deployments/web");
    expect(put.body).toEqual({
      image: "nginx:1.27",
      replicas: 3,
      env: [
        { name: "PORT", value: "8080" },
        { name: "LOG_LEVEL", value: "info" },
      ],
      resource_requests: { cpu: "250m", memory: "256Mi" },
      resource_limits: { cpu: "500m", memory: "512Mi" },
      liveness_probe: {
        probe_type: "httpGet",
        path: "/healthz",
        port: 8080,
        initial_delay_seconds: 10,
        period_seconds: 15,
        timeout_seconds: 2,
        failure_threshold: 3,
      },
      readiness_probe: {
        probe_type: "httpGet",
        path: "/ready",
        port: 8080,
        initial_delay_seconds: 5,
        period_seconds: 10,
        timeout_seconds: 2,
        failure_threshold: 3,
        success_threshold: 1,
      },
    });
    const hpa = await mockApi.waitForRequest("PUT", "/namespaces/default/deployments/web/hpa");
    expect(hpa.body).toEqual({ min_replicas: 2, max_replicas: 8, target_cpu_utilization: 70 });
    expect(mockApi.calls.indexOf(put)).toBeLessThan(mockApi.calls.indexOf(hpa));
    await expect(dialog).toBeHidden();
  });

  test("turning the autoscaler off deletes it", async ({ page, mockApi }) => {
    await page.goto(WEB);
    await page.getByRole("button", { name: "Edit" }).first().click();
    const dialog = page.getByRole("dialog").filter({ hasText: "Edit Deployment" });
    await dialog.getByLabel("Enable Horizontal Pod Autoscaler").uncheck();
    await dialog.getByRole("button", { name: "Update Deployment" }).click();

    await mockApi.waitForRequest("PUT", "/namespaces/default/deployments/web");
    await mockApi.waitForRequest("DELETE", "/namespaces/default/deployments/web/hpa");
    expect(mockApi.requests("PUT", "/namespaces/:ns/deployments/:name/hpa")).toHaveLength(0);
  });
});

test.describe("Deployment detail: keyboard and focus", () => {
  test("a dialog takes focus and is modal", async ({ page }) => {
    await page.goto(WEB);
    await page.getByRole("button", { name: "Restart" }).click();
    const dialog = page.getByRole("dialog").filter({ hasText: "Restart Deployment" });
    await expect(dialog).toBeVisible();
    await expect(dialog).toHaveAttribute("aria-modal", "true");
    await expect.poll(() => dialog.evaluate((el) => el.contains(document.activeElement))).toBe(true);
  });

  test("Escape closes the View YAML dialog", async ({ page, mockApi }) => {
    await page.goto(WEB);
    await page.getByRole("button", { name: "View YAML" }).click();
    const dialog = page.getByRole("dialog").filter({ hasText: "web — YAML" });
    await mockApi.waitForRequest("GET", "/namespaces/default/deployments/web/yaml");
    await expect(dialog.getByText("kind: Deployment")).toBeVisible();

    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    await expect(page.getByRole("button", { name: "View YAML" })).toBeVisible();
  });
});
