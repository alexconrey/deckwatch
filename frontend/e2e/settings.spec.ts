import type { Page } from "@playwright/test";
import { expect, test } from "./fixtures";
import { GATEWAY_URL, movedOutbound } from "./support/data";
import { choose, row } from "./support/ui";

/**
 * Settings, plugin settings, the marketplace and the registry page: what
 * each form sends to the API.
 */

async function section(page: Page, title: string): Promise<void> {
  await page.locator(".v-list--nav .v-list-item").filter({ has: page.getByText(title, { exact: true }) }).click();
}

test.describe("Settings", () => {
  test("save sends the edited settings and the templates, then confirms at the top", async ({ app, page, mockApi }) => {
    await app.goto("/settings");
    const namespaces = page.getByRole("combobox", { name: "Namespaces", exact: true }).first();
    await namespaces.click();
    await namespaces.fill("team-b");
    await namespaces.press("Enter");
    // Chips can be removed again.
    await page.locator(".v-chip", { hasText: "staging" }).locator(".v-chip__close").first().click();
    await page.getByLabel("CPU limit").fill("750m");

    await page.getByRole("button", { name: "Save" }).first().click();

    const put = await mockApi.waitForRequest("PUT", "/settings");
    const body = put.body as Record<string, unknown>;
    expect(body.allowed_namespaces).toEqual(["default", "team-a", "team-b"]);
    expect(body.default_resource_limits).toEqual({
      cpu_request: "100m",
      memory_request: "128Mi",
      cpu_limit: "750m",
      memory_limit: "512Mi",
    });
    // The form never sends these two back.
    expect(body).not.toHaveProperty("tracing");
    expect(body).not.toHaveProperty("credentials");

    const templates = await mockApi.waitForRequest("PUT", "/templates");
    expect((templates.body as { templates: { id: string }[] }).templates.map((t) => t.id)).toEqual([
      "web-app",
      "worker",
      "static-site",
      "acme-node",
    ]);

    const snackbar = page.locator(".v-snackbar").filter({ hasText: "Settings saved" });
    await expect(snackbar).toBeVisible();
    const box = await snackbar.locator(".v-snackbar__wrapper").boundingBox();
    const viewport = page.viewportSize();
    expect(box && viewport && box.y < 120 && Math.abs(box.x + box.width / 2 - viewport.width / 2) < 2).toBe(true);
  });

  test("client-side validation blocks the save", async ({ app, page, mockApi }) => {
    await app.goto("/settings");
    await section(page, "Git Repositories");
    await page.getByRole("button", { name: "Add repository" }).click();
    await page.getByRole("button", { name: "Save" }).first().click();

    await expect(page.getByRole("alert").filter({ hasText: "Every Git repository needs a name and URL." })).toBeVisible();
    expect(mockApi.requests("PUT", "/settings")).toHaveLength(0);
  });

  test("storage class: create is disabled until required fields are filled, then posts", async ({ app, page, mockApi }) => {
    await app.goto("/settings");
    await section(page, "Storage");
    await page.getByRole("button", { name: "Create Storage Class" }).click();
    const dialog = app.dialog("Create Storage Class");
    const create = dialog.getByRole("button", { name: "Create" });
    await expect(create).toBeDisabled();

    await dialog.getByLabel("Name").fill("gp3-fast");
    await dialog.getByLabel("Provisioner").fill("ebs.csi.aws.com");
    await choose(dialog, "Reclaim Policy", "Retain");
    await dialog.getByLabel("Allow Volume Expansion").check();
    await dialog.getByLabel("Mount Options").fill("noatime, nodiratime");
    await dialog.getByRole("button", { name: "Add" }).click();
    await dialog.getByLabel("Key").fill("iops");
    await dialog.getByLabel("Value").fill("6000");
    await create.click();

    const post = await mockApi.waitForRequest("POST", "/storageclasses");
    expect(post.body).toEqual({
      name: "gp3-fast",
      provisioner: "ebs.csi.aws.com",
      reclaim_policy: "Retain",
      volume_binding_mode: "WaitForFirstConsumer",
      allow_volume_expansion: true,
      mount_options: ["noatime", "nodiratime"],
      parameters: { iops: "6000" },
      is_default: false,
    });
    await expect(page.getByText("Storage class created")).toBeVisible();
    await expect(dialog).toBeHidden();
  });

  test("storage class delete asks first, then sends DELETE", async ({ app, page, mockApi }) => {
    await app.goto("/settings");
    await section(page, "Storage");
    await row(page, "efs-shared").locator("button:has(.mdi-delete)").click();
    const dialog = app.dialog("Delete Storage Class");
    await expect(dialog.getByText("Are you sure you want to delete storage class 'efs-shared'?")).toBeVisible();
    await dialog.getByRole("button", { name: "Delete" }).click();
    await mockApi.waitForRequest("DELETE", "/storageclasses/efs-shared");
    await expect(page.getByText("Storage class deleted")).toBeVisible();
  });

  test("ingress class with a parameters reference", async ({ app, page, mockApi }) => {
    await app.goto("/settings");
    await section(page, "Networking");
    await page.getByRole("button", { name: "Create Ingress Class" }).click();
    const dialog = app.dialog("Create Ingress Class");
    await dialog.getByLabel("Name", { exact: true }).fill("traefik");
    await dialog.getByLabel("Controller").fill("traefik.io/ingress-controller");
    await dialog.getByLabel("Set as Default").check();
    await dialog.getByLabel("Configure parameters reference").check();
    await dialog.getByLabel("Kind").fill("IngressClassParams");
    await dialog.getByLabel("Name", { exact: true }).nth(1).fill("traefik-params");
    await dialog.getByRole("button", { name: "Create" }).click();

    const post = await mockApi.waitForRequest("POST", "/ingressclasses");
    expect(post.body).toEqual({
      name: "traefik",
      controller: "traefik.io/ingress-controller",
      is_default: true,
      // Untouched optional fields (API group, namespace, scope) are left out.
      parameters: { kind: "IngressClassParams", name: "traefik-params" },
    });
    await expect(page.getByText("Ingress class created")).toBeVisible();
  });

  test("git token dialog derives the secret name and stores the token", async ({ app, page, mockApi }) => {
    await app.goto("/settings");
    await section(page, "Git Repositories");
    await page.getByRole("button", { name: "Add token" }).click();
    const dialog = app.dialog("Add Git Token");
    await dialog.getByLabel("Display name").fill("Bitbucket CI");
    await dialog.getByLabel("Display name").blur();
    await expect(dialog.getByLabel("Kubernetes Secret name")).toHaveValue("bitbucket-ci-token");
    await dialog.getByLabel("Token value").fill("bbt_0123456789");
    await dialog.getByRole("button", { name: "Create" }).click();

    const put = await mockApi.waitForRequest("PUT", "/git-token-secrets");
    expect(put.body).toEqual({ name: "Bitbucket CI", secret_name: "bitbucket-ci-token", token: "bbt_0123456789" });
    await expect(page.getByText("Git token created")).toBeVisible();
  });

  test("AI credentials are posted on their own endpoint", async ({ app, page, mockApi }) => {
    await app.goto("/settings");
    await section(page, "AI Providers");
    await page.getByLabel("API key").fill("sk-ant-test-key");
    await page.getByRole("button", { name: "Save Credentials" }).click();

    const post = await mockApi.waitForRequest("POST", "/settings/credentials");
    expect(post.body).toEqual({ anthropic_api_key: "sk-ant-test-key" });
    await expect(page.getByText("Credentials saved and encrypted")).toBeVisible();
  });

  test("test notification saves first, then posts the test event", async ({ app, page, mockApi }) => {
    await app.goto("/settings");
    await page.getByRole("button", { name: "Send test notification" }).click();
    await mockApi.waitForRequest("POST", "/notifications/test");
    const order = mockApi.calls
      .filter((c) => (c.method === "PUT" && c.path === "/settings") || c.path === "/notifications/test")
      .map((c) => `${c.method} ${c.path}`);
    expect(order).toEqual(["PUT /settings", "POST /notifications/test"]);
    await expect(page.getByText("Test notification sent")).toBeVisible();
  });

  test("the Agent Feedback section follows the Observability switch", async ({ app, page }) => {
    await app.goto("/settings");
    const nav = page.locator(".v-list--nav");
    await expect(nav.getByText("Agent Feedback", { exact: true })).toBeVisible();
    await section(page, "Observability");
    await page.getByLabel("Enable Agent Feedback").uncheck();
    await expect(nav.getByText("Agent Feedback", { exact: true })).toBeHidden();
  });
});

test.describe("Settings: endpoints", () => {
  test("endpoint overrides are checked while typing and saved with the advanced settings", async ({
    app,
    page,
    mockApi,
  }) => {
    await app.goto("/settings");
    await section(page, "Advanced");
    await mockApi.waitForRequest("GET", "/settings/outbound");

    const anthropic = page.getByRole("textbox", { name: "Anthropic API base URL" });
    await expect(anthropic).toHaveAttribute("placeholder", "https://api.anthropic.com");
    await anthropic.fill("http://llm-gateway.platform.internal");
    await expect(page.getByText("http:// is only allowed for localhost.", { exact: false })).toBeVisible();
    await anthropic.fill("https://llm-gateway.platform.internal");
    await expect(page.getByText("http:// is only allowed for localhost.", { exact: false })).toHaveCount(0);
    await page.getByRole("textbox", { name: "GitHub API base URL" }).fill("https://github.acme.test/api/v3");
    await page.getByRole("textbox", { name: "Vertex AI token URL" }).fill("https://oauth2.googleapis.com/token?x=1");
    await expect(page.getByText("Must not contain a query string or fragment.")).toBeVisible();
    await page.getByRole("textbox", { name: "Vertex AI token URL" }).fill("");

    await page.getByTestId("advanced-save").click();
    const put = await mockApi.waitForRequest("PUT", "/settings");
    expect((put.body as { endpoints: unknown }).endpoints).toEqual({
      anthropic_api_base: "https://llm-gateway.platform.internal",
      vertex_token_url: "",
      vertex_api_base: "",
      bedrock_endpoint: "",
      github_api_base: "https://github.acme.test/api/v3",
      github_web_base: "",
      github_raw_base: "",
    });
    await expect(page.getByText("Advanced settings saved")).toBeVisible();
    // The binding state is reloaded after the save.
    await expect.poll(() => mockApi.requests("GET", "/settings/outbound").length).toBe(2);
  });

  test("a stored key whose endpoint moved is flagged in both sections", async ({ app, page, mockState }) => {
    mockState.settings.endpoints = {
      anthropic_api_base: GATEWAY_URL,
      vertex_token_url: "",
      vertex_api_base: "",
      bedrock_endpoint: "",
      github_api_base: "",
      github_web_base: "",
      github_raw_base: "",
    };
    mockState.outbound = movedOutbound();
    await app.goto("/settings");

    await section(page, "AI Providers");
    await expect(page.getByTestId("anthropic-key-binding")).toHaveText(
      "Saved for api.anthropic.com, but the Anthropic API endpoint is now llm-gateway.platform.internal. " +
        "The key is not sent there; enter it again to use it with the new host.",
    );

    await section(page, "Advanced");
    await expect(page.getByTestId("binding-anthropic_api_key")).toContainText(
      "The endpoint now points to llm-gateway.platform.internal.",
    );
    await expect(page.getByTestId("outbound-policy")).toHaveText(
      "http:// allowed for any host (DECKWATCH_ALLOW_INSECURE_ENDPOINTS)",
    );
    await expect(page.getByTestId("outbound-extra-ca")).toHaveText(
      "System roots + 2 extra CA certificates (/etc/deckwatch/ca/corp-root.pem)",
    );
  });
});

test.describe("Plugin settings", () => {
  test("save posts the config fields and confirms with the global snackbar", async ({ app, page, mockApi }) => {
    await app.goto("/settings/plugins/otel");
    await choose(page, "Exporter", "zipkin");
    await page.getByLabel("Collector endpoint").fill("http://zipkin.observability.svc:9411");
    await page.getByRole("button", { name: "Save configuration" }).click();

    const post = await mockApi.waitForRequest("POST", "/plugins/otel/config");
    // The secret is left out while it is still "configured"; env-sourced
    // fields are never sent.
    expect(post.body).toEqual({
      exporter: "zipkin",
      endpoint: "http://zipkin.observability.svc:9411",
      sample_all: "false",
    });
    await expect(page.locator(".v-snackbar").filter({ hasText: "Plugin configuration saved" })).toBeVisible();
  });

  test("the sha256 truncates when narrow and keeps the full value in its title", async ({ app, page }) => {
    await page.setViewportSize({ width: 500, height: 900 });
    await app.goto("/settings/plugins/otel");
    const sha = page.getByTestId("plugin-sha256");
    await expect(sha).toHaveAttribute("title", "3f7a9c2e5b8d1f4a6c0e2b9d7f5a3c1e8b6d4f2a0c9e7b5d3f1a8c6e4b2d0f9a");
    await expect(sha).toHaveCSS("text-overflow", "ellipsis");
    await expect(sha).toHaveCSS("white-space", "nowrap");
    expect(await sha.evaluate((el) => el.scrollWidth > el.clientWidth)).toBe(true);
  });
});

test.describe("Marketplace", () => {
  test("search filters the catalog and install adds the plugin to settings", async ({ app, page, mockApi }) => {
    await app.goto("/marketplace");
    await expect(page.locator(".v-card", { hasText: "OpenTelemetry" }).getByText("Installed")).toBeVisible();
    await page.getByPlaceholder("Search plugins...").fill("slack");
    await expect(page.getByText("Cost explorer")).toBeHidden();

    await page.locator(".v-card", { hasText: "Slack notifications" }).getByRole("button", { name: "Install" }).click();
    await page.getByRole("dialog").getByRole("button", { name: "Install" }).click();

    const put = await mockApi.waitForRequest("PUT", "/settings");
    const plugins = (put.body as { plugins: { name: string }[] }).plugins;
    expect(plugins.at(-1)).toEqual({
      name: "slack",
      enabled: true,
      source: { type: "github", repo: "deckwatch/deckwatch-plugin-slack", ref: "v0.9.3", path: "", use_release: true },
      allowed_hosts: ["hooks.slack.com"],
      config: {},
      inherit_env_keys: [],
      inherit_env_file_keys: {},
    });
  });
});

test.describe("Registry", () => {
  test("selecting a repository lists its tags, and delete removes one", async ({ app, page, mockApi }) => {
    await app.goto("/registry");
    await page.getByText("acme/platform-api").click();
    // Nested repository names keep their slash in the path.
    await mockApi.waitForRequest("GET", "/registry/repositories/acme/platform-api/tags");
    await expect(page.getByRole("cell", { name: "v2.4.0" })).toBeVisible();

    await row(page, "v2.4.0").locator("button:has(.mdi-delete-outline)").click();
    await app.dialog("Delete tag?").getByRole("button", { name: "Delete" }).click();
    await mockApi.waitForRequest("DELETE", "/registry/repositories/acme/platform-api/tags/v2.4.0");
  });

  test("the manifest dialog shows the layers", async ({ app, page, mockApi }) => {
    await app.goto("/registry");
    await page.getByText("web", { exact: true }).click();
    await row(page, "latest").locator("button:has(.mdi-file-document-outline)").click();
    await mockApi.waitForRequest("GET", "/registry/repositories/web/tags/latest");
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByText("Layers (2)")).toBeVisible();
    await dialog.getByRole("button", { name: "Close" }).click();
    await expect(dialog).toBeHidden();
  });
});
