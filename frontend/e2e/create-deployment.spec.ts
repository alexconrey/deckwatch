import type { Locator, Page } from "@playwright/test";
import { apiError } from "./support/mock-api";
import { expect, test } from "./fixtures";

/**
 * Create deployment (/deployments/create, DeploymentForm), the template
 * picker and the registry tag picker.
 */

const field = (page: Page, name: string) => page.getByRole("textbox", { name, exact: true });

/** The probe's own card (not the page card that also contains it). */
const probeCard = (page: Page, probe: string) =>
  page.locator(".v-card", { has: page.getByLabel(probe, { exact: true }), hasNot: page.locator(".v-card") });

/** Open a v-select by clicking its field (the inner input is 1 character wide). */
const openSelect = (combobox: Locator) =>
  combobox.locator("xpath=ancestor::div[contains(concat(' ', @class, ' '), ' v-field ')][1]").click();

async function chooseOption(page: Page, combobox: Locator, option: string): Promise<void> {
  await openSelect(combobox);
  await page.getByRole("option", { name: option, exact: true }).click();
}

async function openForm(app: { goto(path: string): Promise<void> }, page: Page): Promise<void> {
  await app.goto("/deployments/create");
  // The settings mock's cluster defaults fill the blank resource fields.
  await expect(field(page, "CPU Request")).toHaveValue("100m");
}

test.describe("Create deployment form", () => {
  test("every field is posted in the API's shape", async ({ app, page, mockApi }) => {
    await openForm(app, page);
    await field(page, "Deployment Name").fill("checkout");
    await field(page, "Container Image").fill("ghcr.io/acme/checkout:1.4.0");
    await page.getByRole("spinbutton", { name: "Replicas" }).fill("3");
    await expect(page.getByRole("slider")).toHaveAttribute("aria-valuenow", "3");

    // Two ports: TCP is the default and is not sent; UDP is.
    await page.getByRole("button", { name: "Add Port" }).click();
    await page.getByRole("button", { name: "Add Port" }).click();
    const ports = page.getByRole("spinbutton", { name: "Port" });
    await ports.nth(0).fill("8080");
    await page.getByRole("textbox", { name: "Name (optional)" }).nth(0).fill(" http ");
    await ports.nth(1).fill("8125");
    await chooseOption(page, page.getByRole("combobox", { name: "Protocol" }).nth(1), "UDP");

    // Env rows without a name are dropped.
    await page.getByRole("button", { name: "Add Variable" }).click();
    await page.getByRole("button", { name: "Add Variable" }).click();
    await field(page, "Name").nth(0).fill("NODE_ENV");
    await field(page, "Value").nth(0).fill("production");

    await field(page, "CPU Request").fill("250m");
    await field(page, "Memory Limit").fill("1Gi");

    const liveness = probeCard(page, "Liveness Probe");
    await liveness.getByLabel("Liveness Probe", { exact: true }).check();
    await liveness.getByRole("textbox", { name: "Path" }).fill("/healthz");
    await liveness.getByRole("spinbutton", { name: "Port" }).fill("8080");
    await liveness.getByRole("spinbutton", { name: "Delay (s)" }).fill("10");

    const readiness = probeCard(page, "Readiness Probe");
    await readiness.getByLabel("Readiness Probe", { exact: true }).check();
    await openSelect(readiness.getByRole("combobox", { name: "Type" }));
    await page.getByRole("option", { name: "TCP Socket" }).click();
    await readiness.getByRole("spinbutton", { name: "Port" }).fill("8080");

    const startup = probeCard(page, "Startup Probe");
    await startup.getByLabel("Startup Probe", { exact: true }).check();
    await openSelect(startup.getByRole("combobox", { name: "Type" }));
    await page.getByRole("option", { name: "Exec" }).click();
    await startup.getByRole("textbox", { name: "Command (comma-separated)" }).fill("cat, /tmp/ready, ");
    await startup.getByRole("spinbutton", { name: "Failure Threshold" }).fill("30");

    // The "Advanced" command field shares its label with the exec probe's.
    await field(page, "Command (comma-separated)").last().fill("/bin/sh, -c");
    await field(page, "Args (comma-separated)").fill("node server.js, --port, 8080");

    await page.getByRole("button", { name: "Create Deployment" }).click();
    const call = await mockApi.waitForRequest("POST", "/namespaces/default/deployments");
    expect(call.body).toEqual({
      name: "checkout",
      image: "ghcr.io/acme/checkout:1.4.0",
      replicas: 3,
      ports: [{ port: 8080, name: "http" }, { port: 8125, protocol: "UDP" }],
      env: [{ name: "NODE_ENV", value: "production" }],
      command: ["/bin/sh", "-c"],
      args: ["node server.js", "--port", "8080"],
      resource_requests: { cpu: "250m", memory: "128Mi" },
      resource_limits: { cpu: "500m", memory: "1Gi" },
      liveness_probe: { probe_type: "httpGet", path: "/healthz", port: 8080, initial_delay_seconds: 10 },
      readiness_probe: { probe_type: "tcpSocket", port: 8080 },
      startup_probe: { probe_type: "exec", command: ["cat", "/tmp/ready"], failure_threshold: 30 },
    });
    await expect(page).toHaveURL(/\/deployments\/default\/checkout$/);
  });

  test("validation messages appear as the user types", async ({ app, page }) => {
    await openForm(app, page);
    const name = field(page, "Deployment Name");

    await name.fill("my_app");
    await expect(page.getByText(/Resource names cannot contain underscores/)).toBeVisible();
    await name.fill("My-App");
    await expect(page.getByText("Use lowercase letters, digits, and hyphens only (no leading/trailing hyphen)")).toBeVisible();
    await name.fill("a".repeat(64));
    await expect(page.getByText("Name must be 63 characters or fewer")).toBeVisible();
    await name.fill("");
    await expect(page.getByText("Required").first()).toBeVisible();
    await name.fill("my-app");
    await expect(page.getByText("Lowercase letters, digits, and hyphens only (e.g. my-api)")).toBeVisible();

    const image = field(page, "Container Image");
    await image.fill(" nginx:1.27");
    await expect(page.getByText("Image must not have leading or trailing whitespace")).toBeVisible();
    await image.fill("nginx");
    await expect(page.getByText("No tag specified -- 'latest' will be used")).toBeVisible();

    await field(page, "CPU Limit").fill("50m");
    await expect(page.getByText("CPU limit is less than CPU request")).toBeVisible();
    await field(page, "Memory Limit").fill("64Mi");
    await expect(page.getByText("Memory limit is less than memory request")).toBeVisible();

    await page.getByRole("button", { name: "Add Port" }).click();
    await page.getByRole("spinbutton", { name: "Port" }).fill("70000");
    await expect(page.getByText("Port must be 1–65535")).toBeVisible();

    await page.getByRole("spinbutton", { name: "Replicas" }).fill("0");
    await expect(page.getByText("Setting replicas to 0 will stop all instances")).toBeVisible();
    // Out-of-range replica counts are clamped to the slider's 0-20.
    await page.getByRole("spinbutton", { name: "Replicas" }).fill("50");
    await expect(page.getByRole("slider")).toHaveAttribute("aria-valuenow", "20");
  });

  test("an API error keeps the user's input and shows the message", async ({ app, page, mockApi }) => {
    mockApi.on("POST", "/namespaces/:ns/deployments", apiError(409, 'deployments.apps "web" already exists', "conflict"));
    await openForm(app, page);
    await field(page, "Deployment Name").fill("web");
    await field(page, "Container Image").fill("nginx:1.27");
    await page.getByRole("button", { name: "Create Deployment" }).click();

    await expect(page.getByRole("alert").filter({ hasText: 'deployments.apps "web" already exists' })).toBeVisible();
    await expect(page).toHaveURL(/\/deployments\/create$/);
    await expect(field(page, "Deployment Name")).toHaveValue("web");
    await expect(field(page, "Container Image")).toHaveValue("nginx:1.27");
  });

  test("Validate runs a dry run and shows the result", async ({ app, page, mockApi }) => {
    await openForm(app, page);
    await field(page, "Deployment Name").fill("checkout");
    await field(page, "Container Image").fill("ghcr.io/acme/checkout:1.4.0");
    await page.getByRole("button", { name: "Validate" }).click();

    const call = await mockApi.waitForRequest("POST", "/namespaces/default/deployments/validate");
    expect(call.body).toEqual({
      name: "checkout",
      image: "ghcr.io/acme/checkout:1.4.0",
      replicas: 1,
      resource_requests: { cpu: "100m", memory: "128Mi" },
      resource_limits: { cpu: "500m", memory: "512Mi" },
    });
    const passed = page.getByRole("alert").filter({ hasText: "Dry-run passed — the API server accepted this spec." });
    await expect(passed).toBeVisible();
    // Validating never creates anything.
    expect(mockApi.requests("POST", "/namespaces/:ns/deployments")).toHaveLength(0);

    mockApi.on("POST", "/namespaces/:ns/deployments/validate", {
      json: { ok: false, errors: ["spec.template.spec.containers[0].image: Required value", "metadata.name: Invalid value"] },
    });
    await page.getByRole("button", { name: "Validate" }).click();
    const failed = page.getByRole("alert").filter({ hasText: "Validation failed:" });
    await expect(failed.getByRole("listitem")).toHaveText([
      "spec.template.spec.containers[0].image: Required value",
      "metadata.name: Invalid value",
    ]);
    await expect(passed).toBeHidden();
  });

  test("a failing dry-run request is reported as a validation error", async ({ app, page, mockApi }) => {
    mockApi.on("POST", "/namespaces/:ns/deployments/validate", apiError(500, "admission webhook unavailable"));
    await openForm(app, page);
    await page.getByRole("button", { name: "Validate" }).click();
    await expect(
      page.getByRole("alert").filter({ hasText: "Validation failed:" }).getByRole("listitem"),
    ).toHaveText(["admission webhook unavailable"]);
  });

  test("the replicas slider is keyboard operable", async ({ app, page }) => {
    await openForm(app, page);
    const slider = page.getByRole("slider");
    await expect(slider).toHaveAttribute("aria-valuemin", "0");
    await expect(slider).toHaveAttribute("aria-valuemax", "20");
    await slider.focus();
    await slider.press("ArrowRight");
    await slider.press("ArrowRight");
    await expect(page.getByRole("spinbutton", { name: "Replicas" })).toHaveValue("3");
    await slider.press("Home");
    await expect(page.getByRole("spinbutton", { name: "Replicas" })).toHaveValue("0");
  });
});

test.describe("Template picker", () => {
  test("a template pre-fills the form and its values are posted", async ({ app, page, mockApi }) => {
    await app.goto("/deployments/create");
    await page.getByRole("button", { name: "Templates" }).click();
    await expect(page).toHaveURL(/\/deployments\/templates$/);
    await mockApi.waitForRequest("GET", "/templates");

    await page.locator(".v-card", { hasText: "ACME Node.js service" }).getByRole("button", { name: "Use Template" }).click();
    await expect(page).toHaveURL(/\/deployments\/create\?template=acme-node$/);
    await expect(page.getByRole("alert").filter({ hasText: "Pre-filled from template: acme-node" })).toBeVisible();
    await expect(field(page, "Container Image")).toHaveValue("ghcr.io/acme/node-base:22");
    await expect(page.getByRole("spinbutton", { name: "Replicas" })).toHaveValue("2");
    // Template values win over the cluster defaults from settings.
    await expect(field(page, "Memory Request")).toHaveValue("256Mi");

    await field(page, "Deployment Name").fill("orders");
    await page.getByRole("button", { name: "Create Deployment" }).click();
    const call = await mockApi.waitForRequest("POST", "/namespaces/default/deployments");
    expect(call.body).toEqual({
      name: "orders",
      image: "ghcr.io/acme/node-base:22",
      replicas: 2,
      ports: [{ port: 3000, name: "http" }],
      env: [{ name: "NODE_ENV", value: "production" }],
      resource_requests: { cpu: "100m", memory: "256Mi" },
      resource_limits: { cpu: "500m", memory: "512Mi" },
    });
  });

  test("the stashed template is used once", async ({ app, page }) => {
    await app.goto("/deployments/templates");
    await page.locator(".v-card", { hasText: "Background Worker" }).click();
    await expect(field(page, "Container Image")).toHaveValue("busybox:1.36");
    await expect(field(page, "Command (comma-separated)").last()).toHaveValue("sh, -c, sleep infinity");

    // A reload starts from a blank form again.
    await app.reload();
    await expect(field(page, "Container Image")).toHaveValue("");
    await expect(page.getByText("Pre-filled from template:")).toBeHidden();
  });

  test("Start Blank opens an empty form", async ({ app, page }) => {
    await app.goto("/deployments/templates");
    await page.getByRole("button", { name: "Start Blank" }).click();
    await expect(page).toHaveURL(/\/deployments\/create$/);
    await expect(field(page, "Container Image")).toHaveValue("");
  });
});

test.describe("Registry tag picker", () => {
  test("picking a tag fills the image with the registry host", async ({ app, page, mockApi }) => {
    await openForm(app, page);
    await page.locator("button:has(.mdi-image-search)").click();

    const dialog = app.dialog("Browse Registry");
    await expect(dialog.getByText("registry.deckwatch.local", { exact: true })).toBeVisible();
    await expect(dialog.getByText("Pick a repository on the left to see its tags.")).toBeVisible();
    await mockApi.waitForRequest("GET", "/registry/repositories");

    await dialog.getByText("web", { exact: true }).click();
    await mockApi.waitForRequest("GET", "/registry/repositories/web/tags");
    // Newest first.
    await expect(dialog.locator("tbody tr").first()).toContainText("9f8e7d6");
    await dialog.getByRole("row").filter({ hasText: "9f8e7d6" }).click();

    await expect(dialog).toBeHidden();
    await expect(field(page, "Container Image")).toHaveValue("registry.deckwatch.local/web:9f8e7d6");
  });

  test("nested repository names keep their slash, and the filters narrow the lists", async ({ app, page, mockApi }) => {
    await openForm(app, page);
    await page.locator("button:has(.mdi-image-search)").click();
    const dialog = app.dialog("Browse Registry");

    await dialog.getByPlaceholder("Filter repositories").fill("platform");
    await expect(dialog.getByText("web", { exact: true })).toBeHidden();
    await dialog.getByText("acme/platform-api", { exact: true }).click();
    await mockApi.waitForRequest("GET", "/registry/repositories/acme/platform-api/tags");

    await dialog.getByPlaceholder("Filter tags").fill("v2.4.0");
    await expect(dialog.locator("tbody tr")).toHaveCount(1);
    await dialog.getByRole("row").filter({ hasText: "v2.4.0" }).click();
    await expect(field(page, "Container Image")).toHaveValue("registry.deckwatch.local/acme/platform-api:v2.4.0");
  });

  test("a disabled registry says so", async ({ app, page, mockState }) => {
    mockState.features.registry = false;
    await openForm(app, page);
    await page.locator("button:has(.mdi-image-search)").click();
    const dialog = app.dialog("Browse Registry");
    await expect(dialog.getByText("Registry not enabled")).toBeVisible();
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toBeHidden();
  });
});
