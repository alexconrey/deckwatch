import type { Page } from "@playwright/test";
import { eachTheme, expect, test } from "./visual-test";
import { openSelect } from "../support/ui";

/**
 * Template picker (/deployments/templates) and the create form
 * (/deployments/create, DeploymentForm).
 */

/** The probe's own card (not the page card that also contains it). */
const probeCard = (page: Page, probe: string) =>
  page.locator(".v-card", { has: page.getByLabel(probe, { exact: true }), hasNot: page.locator(".v-card") });

/**
 * Open a v-select by clicking its field. The combobox <input> inside is a
 * 1-character-wide element under the selection text, so it never receives
 * the click itself.
 */
async function openForm(app: { goto(path: string): Promise<void> }, page: Page): Promise<void> {
  await app.goto("/deployments/create");
  // Cluster defaults from the settings mock fill the resource fields.
  await expect(page.getByRole("textbox", { name: "CPU Request" })).toHaveValue("100m");
}

eachTheme(() => {
  test("template picker", async ({ app, page }) => {
    await app.goto("/deployments/templates");
    await expect(page.getByText("ACME Node.js service")).toBeVisible();
    await app.snap("create-deployment/template-picker", { fullPage: true });
  });

  test("empty form", async ({ app, page }) => {
    await openForm(app, page);
    await app.snap("create-deployment/empty-form", { fullPage: true });
  });

  test("validation errors", async ({ app, page }) => {
    await openForm(app, page);
    await page.getByRole("textbox", { name: "Deployment Name" }).fill("my_app");
    await page.getByRole("textbox", { name: "Container Image" }).fill(" nginx:1.27");
    await page.getByRole("textbox", { name: "CPU Limit" }).fill("50m");
    await page.getByRole("textbox", { name: "Memory Limit" }).fill("64Mi");
    await expect(page.getByText("CPU limit is less than CPU request")).toBeVisible();
    await expect(page.getByText("Memory limit is less than memory request")).toBeVisible();
    await expect(page.getByText(/cannot contain underscores/)).toBeVisible();
    await expect(page.getByText("Image must not have leading or trailing whitespace")).toBeVisible();
    await app.snap("create-deployment/validation-errors", { fullPage: true });
  });

  test("filled form with a port, an env var and a probe", async ({ app, page }) => {
    await openForm(app, page);
    await page.getByRole("textbox", { name: "Deployment Name" }).fill("checkout");
    await page.getByRole("textbox", { name: "Container Image" }).fill("ghcr.io/acme/checkout:1.4.0");
    await page.getByRole("button", { name: "Add Port" }).click();
    await page.getByRole("spinbutton", { name: "Port" }).fill("8080");
    await page.getByRole("textbox", { name: "Name (optional)" }).fill("http");
    await page.getByRole("button", { name: "Add Variable" }).click();
    await page.getByRole("textbox", { name: "Name", exact: true }).fill("NODE_ENV");
    await page.getByRole("textbox", { name: "Value", exact: true }).fill("production");
    await page.getByLabel("Liveness Probe", { exact: true }).check();
    await probeCard(page, "Liveness Probe").getByRole("textbox", { name: "Path" }).fill("/healthz");
    await app.snap("create-deployment/filled-form", { fullPage: true });
  });

  test("probe type menu", async ({ app, page }) => {
    await openForm(app, page);
    await page.getByLabel("Readiness Probe", { exact: true }).check();
    await openSelect(probeCard(page, "Readiness Probe").getByRole("combobox", { name: "Type" }));
    await expect(page.getByRole("option", { name: "TCP Socket" })).toBeVisible();
    await app.snap("create-deployment/probe-type-menu");
  });

  test("protocol menu", async ({ app, page }) => {
    await openForm(app, page);
    await page.getByRole("button", { name: "Add Port" }).click();
    await openSelect(page.getByRole("combobox", { name: "Protocol" }));
    await expect(page.getByRole("option", { name: "UDP" })).toBeVisible();
    await app.snap("create-deployment/protocol-menu");
  });

  test("replicas slider thumb label", async ({ app, page }) => {
    await openForm(app, page);
    const slider = page.getByRole("slider");
    await slider.focus();
    await slider.press("ArrowRight");
    await slider.press("ArrowRight");
    await expect(slider).toHaveAttribute("aria-valuenow", "3");
    await app.snap("create-deployment/replicas-thumb-label");
  });

  test("replicas set to zero", async ({ app, page }) => {
    await openForm(app, page);
    await page.getByRole("spinbutton", { name: "Replicas" }).fill("0");
    await expect(page.getByText("Setting replicas to 0 will stop all instances")).toBeVisible();
    await app.snap("create-deployment/replicas-zero");
  });

  test("validate passed", async ({ app, page }) => {
    await openForm(app, page);
    await page.getByRole("textbox", { name: "Deployment Name" }).fill("checkout");
    await page.getByRole("textbox", { name: "Container Image" }).fill("ghcr.io/acme/checkout:1.4.0");
    await page.getByRole("button", { name: "Validate" }).click();
    await expect(page.getByText("Dry-run passed — the API server accepted this spec.")).toBeVisible();
    await app.snap("create-deployment/validate-passed", { fullPage: true });
  });

  test("validate failed", async ({ app, page, mockApi }) => {
    mockApi.on("POST", "/namespaces/:ns/deployments/validate", {
      json: {
        ok: false,
        errors: [
          'Deployment.apps "checkout" is invalid: spec.template.spec.containers[0].resources.requests: Invalid value: "100m": must be less than or equal to cpu limit',
          "spec.replicas: Invalid value: -1: must be greater than or equal to 0",
        ],
      },
    });
    await openForm(app, page);
    await page.getByRole("textbox", { name: "Deployment Name" }).fill("checkout");
    await page.getByRole("textbox", { name: "Container Image" }).fill("ghcr.io/acme/checkout:1.4.0");
    await page.getByRole("button", { name: "Validate" }).click();
    await expect(page.getByText("Validation failed:")).toBeVisible();
    await app.snap("create-deployment/validate-failed", { fullPage: true });
  });

  test("pre-filled from a template", async ({ app, page }) => {
    await app.goto("/deployments/templates");
    await page
      .locator(".v-card", { hasText: "ACME Node.js service" })
      .getByRole("button", { name: "Use Template" })
      .click();
    await expect(page.getByText("Pre-filled from template:")).toBeVisible();
    await expect(page.getByRole("textbox", { name: "Container Image" })).toHaveValue("ghcr.io/acme/node-base:22");
    await app.snap("create-deployment/from-template", { fullPage: true });
  });

  test("registry picker with a repository selected", async ({ app, page }) => {
    await openForm(app, page);
    await page.locator("button:has(.mdi-image-search)").click();
    const dialog = app.dialog("Browse Registry");
    await dialog.getByText("acme/platform-api", { exact: true }).click();
    await expect(dialog.getByRole("cell", { name: "v2.4.1" })).toBeVisible();
    await app.snap("create-deployment/registry-picker");
  });
});

for (const width of [1200, 900]) {
  test.describe(`at ${width}px`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("filled form", async ({ app, page }) => {
      await openForm(app, page);
      await page.getByRole("textbox", { name: "Deployment Name" }).fill("checkout");
      await page.getByRole("textbox", { name: "Container Image" }).fill("ghcr.io/acme/checkout:1.4.0");
      await page.getByRole("button", { name: "Add Port" }).click();
      await page.getByRole("button", { name: "Add Variable" }).click();
      await page.getByLabel("Liveness Probe", { exact: true }).check();
      await app.snap("create-deployment/filled-form", { fullPage: true });
    });
  });
}
