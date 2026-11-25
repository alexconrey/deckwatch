import type { Page } from "@playwright/test";
import { GATEWAY_URL, type MockState, movedOutbound } from "../support/data";
import { apiError } from "../support/mock-api";
import { openMenu, openSelect, row } from "../support/ui";
import { eachTheme, expect, test } from "./visual-test";

/**
 * Settings (every section), plugin settings, the marketplace and the
 * registry page, with their dialogs and menus.
 */

/** Open a Settings section from the sidebar. */
async function section(page: Page, title: string): Promise<void> {
  await page.locator(".v-list--nav .v-list-item").filter({ has: page.getByText(title, { exact: true }) }).click();
}

/** The Anthropic endpoint moved to a gateway after the API key was saved. */
function movedEndpoint(state: MockState): void {
  state.settings.endpoints = {
    anthropic_api_base: GATEWAY_URL,
    vertex_token_url: "",
    vertex_api_base: "",
    bedrock_endpoint: "",
    github_api_base: "",
    github_web_base: "",
    github_raw_base: "",
  };
  state.outbound = movedOutbound();
}

const SECTIONS: [title: string, heading: string][] = [
  ["General", "Allowed namespaces"],
  ["Storage", "Default storage class"],
  ["Networking", "Ingress classes"],
  ["Authentication", "Enable Entra authentication"],
  ["AI Providers", "AI Integrations"],
  ["Observability", "Prometheus monitoring"],
  ["Agent Feedback", "Agent Feedback"],
  ["Templates", "Deployment templates"],
  ["Git Repositories", "Managed repositories"],
  ["Container Registries", "Managed registries"],
  ["Plugins", "Loaded plugins"],
  ["MCP Tuning", "Global instructions"],
  ["Advanced", "Marketplace URL"],
  ["Audit Log", "Audit Log"],
];

eachTheme(() => {
  test.describe("settings sections", () => {
    for (const [title, heading] of SECTIONS) {
      test(title, async ({ app, page }) => {
        await app.goto("/settings");
        await section(page, title);
        await expect(page.getByText(heading, { exact: true }).last()).toBeVisible();
        await app.snap(`settings/${title.toLowerCase().replace(/ /g, "-")}`, { fullPage: true });
      });
    }
  });

  test.describe("settings dialogs and menus", () => {
    test("default storage class menu", async ({ app, page }) => {
      await app.goto("/settings");
      await section(page, "Storage");
      const menu = await openMenu(page, "Default Storage Class");
      await expect(menu.getByRole("option", { name: "None (cluster default)" })).toBeVisible();
      await app.snap("settings/storage-default-class-menu");
    });

    test("create storage class dialog with validation errors", async ({ app, page }) => {
      await app.goto("/settings");
      await section(page, "Storage");
      await page.getByRole("button", { name: "Create Storage Class" }).click();
      const dialog = app.dialog("Create Storage Class");
      await dialog.getByLabel("Name").fill("x");
      await dialog.getByLabel("Name").fill("");
      await dialog.getByLabel("Provisioner").fill("x");
      await dialog.getByLabel("Provisioner").fill("");
      await expect(dialog.getByText("Name is required")).toBeVisible();
      await expect(dialog.getByText("Provisioner is required")).toBeVisible();
      await app.snap("settings/storage-class-dialog-invalid");
    });

    test("edit storage class dialog", async ({ app, page }) => {
      await app.goto("/settings");
      await section(page, "Storage");
      await row(page, "io2-retain").locator("button:has(.mdi-pencil)").click();
      await expect(app.dialog("Edit Storage Class").getByLabel("Provisioner")).toHaveValue("ebs.csi.aws.com");
      await app.snap("settings/storage-class-dialog-edit");
    });

    test("delete storage class confirmation", async ({ app, page }) => {
      await app.goto("/settings");
      await section(page, "Storage");
      await row(page, "efs-shared").locator("button:has(.mdi-delete)").click();
      await expect(app.dialog("Delete Storage Class")).toBeVisible();
      await app.snap("settings/storage-class-delete");
    });

    test("ingress class dialog with parameters", async ({ app, page }) => {
      await app.goto("/settings");
      await section(page, "Networking");
      await page.getByRole("button", { name: "Create Ingress Class" }).click();
      const dialog = app.dialog("Create Ingress Class");
      await dialog.getByLabel("Name", { exact: true }).fill("traefik");
      await dialog.getByLabel("Controller").fill("traefik.io/ingress-controller");
      await dialog.getByLabel("Configure parameters reference").check();
      await expect(dialog.getByLabel("Kind")).toBeVisible();
      await app.snap("settings/ingress-class-dialog");
    });

    test("endpoints after the Anthropic endpoint moved", async ({ app, page, mockState }) => {
      movedEndpoint(mockState);
      await app.goto("/settings");
      await section(page, "Advanced");
      await expect(page.getByTestId("binding-anthropic_api_key")).toContainText("The endpoint now points to");
      await page.getByRole("textbox", { name: "GitHub API base URL" }).fill("https://github.acme.test/api/v3?per_page=100");
      await expect(page.getByText("Must not contain a query string or fragment.")).toBeVisible();
      await app.snap("settings/advanced-endpoints-moved", { fullPage: true });
    });

    test("AI providers with the key bound to the old endpoint", async ({ app, page, mockState }) => {
      movedEndpoint(mockState);
      await app.goto("/settings");
      await section(page, "AI Providers");
      await expect(page.getByTestId("anthropic-key-binding")).toContainText("but the Anthropic API endpoint is now");
      await app.snap("settings/ai-providers-key-moved", { fullPage: true });
    });

    test("ai provider menu", async ({ app, page }) => {
      await app.goto("/settings");
      await section(page, "AI Providers");
      const menu = await openMenu(page, "API Provider");
      await expect(menu.getByRole("option", { name: "Google Vertex AI" })).toBeVisible();
      await app.snap("settings/ai-provider-menu");
    });

    test("deployment template expanded", async ({ app, page }) => {
      await app.goto("/settings");
      await section(page, "Templates");
      await page.locator(".v-expansion-panel-title", { hasText: "Web Application" }).click();
      await expect(page.getByLabel("JSON payload")).toBeVisible();
      await app.snap("settings/template-expanded", { fullPage: true });
    });

    test("ingress template expanded", async ({ app, page }) => {
      await app.goto("/settings");
      await section(page, "Templates");
      await page.locator(".v-expansion-panel-title", { hasText: "public-alb" }).click();
      await expect(page.getByLabel("Template name")).toHaveValue("public-alb");
      await app.snap("settings/ingress-template-expanded", { fullPage: true });
    });

    test("git token dialog", async ({ app, page }) => {
      await app.goto("/settings");
      await section(page, "Git Repositories");
      await page.getByRole("button", { name: "Add token" }).click();
      await expect(app.dialog("Add Git Token")).toBeVisible();
      await app.snap("settings/git-token-dialog");
    });

    test("container registry type menu", async ({ app, page }) => {
      await app.goto("/settings");
      await section(page, "Container Registries");
      // Second registry row (ecr-prod); every row has a "Type" select.
      const menu = await openSelect(page.getByRole("combobox", { name: "Type", exact: true }).nth(1));
      await expect(menu.getByRole("option", { name: "Harbor" })).toBeVisible();
      await app.snap("settings/registry-type-menu");
    });

    test("marketplace inside plugins", async ({ app, page }) => {
      await app.goto("/settings");
      await section(page, "Plugins");
      await page.getByText("Browse Marketplace").click();
      await expect(page.getByText("Slack notifications")).toBeVisible();
      await app.snap("settings/plugins-marketplace", { fullPage: true });
    });

    test("snackbar after save", async ({ app, page }) => {
      await app.goto("/settings");
      await page.getByRole("button", { name: "Save" }).first().click();
      await expect(page.getByText("Settings saved")).toBeVisible();
      await app.snap("settings/saved-snackbar");
    });

    test("load error", async ({ app, page, mockApi }) => {
      mockApi.on("GET", "/templates", apiError(500, "templates ConfigMap is unreadable"));
      await app.goto("/settings");
      await expect(page.getByText("templates ConfigMap is unreadable")).toBeVisible();
      await app.snap("settings/load-error");
    });
  });

  test.describe("plugin settings", () => {
    test("page", async ({ app, page }) => {
      await app.goto("/settings/plugins/otel");
      await expect(page.getByTestId("plugin-sha256")).toBeVisible();
      await app.snap("plugin-settings/otel", { fullPage: true });
    });

    test("select menu", async ({ app, page }) => {
      await app.goto("/settings/plugins/otel");
      const menu = await openMenu(page, "Exporter");
      await expect(menu.getByRole("option", { name: "zipkin" })).toBeVisible();
      await app.snap("plugin-settings/exporter-menu");
    });

    test("saved snackbar", async ({ app, page }) => {
      await app.goto("/settings/plugins/otel");
      await page.getByRole("button", { name: "Save configuration" }).click();
      await expect(page.getByText("Plugin configuration saved")).toBeVisible();
      await app.snap("plugin-settings/saved-snackbar");
    });

    test("plugin not loaded", async ({ app, page }) => {
      await app.goto("/settings/plugins/unknown-plugin");
      await expect(page.getByText("is not currently loaded")).toBeVisible();
      await app.snap("plugin-settings/not-loaded", { fullPage: true });
    });
  });

  test.describe("marketplace", () => {
    test("catalog", async ({ app, page }) => {
      await app.goto("/marketplace");
      await expect(page.getByText("Cost explorer")).toBeVisible();
      await app.snap("marketplace/catalog", { fullPage: true });
    });

    test("no results", async ({ app, page }) => {
      await app.goto("/marketplace");
      await page.getByPlaceholder("Search plugins...").fill("no-such-plugin");
      await expect(page.getByText("No plugins found")).toBeVisible();
      await app.snap("marketplace/no-results", { fullPage: true });
    });

    test("install dialog for a community plugin", async ({ app, page }) => {
      await app.goto("/marketplace");
      await page.locator(".v-card", { hasText: "Cost explorer" }).getByRole("button", { name: "Install" }).click();
      await expect(page.getByText("This plugin has not been reviewed")).toBeVisible();
      await app.snap("marketplace/install-dialog");
    });

    test("disabled", async ({ app, page, mockState }) => {
      mockState.settings.marketplace_url = "";
      await app.goto("/marketplace");
      await expect(page.getByText("Marketplace is disabled.")).toBeVisible();
      await app.snap("marketplace/disabled", { fullPage: true });
    });
  });

  test.describe("registry", () => {
    test("repository selected", async ({ app, page }) => {
      await app.goto("/registry");
      await page.getByText("acme/platform-api").click();
      await expect(page.getByRole("cell", { name: "v2.4.1" })).toBeVisible();
      await app.snap("registry/repository", { fullPage: true });
    });

    test("manifest dialog", async ({ app, page }) => {
      await app.goto("/registry");
      await page.getByText("web", { exact: true }).click();
      await row(page, "9f8e7d6").locator("button:has(.mdi-file-document-outline)").click();
      await expect(page.getByText("Layers (2)")).toBeVisible();
      await app.snap("registry/manifest-dialog");
    });

    test("delete tag confirmation", async ({ app, page }) => {
      await app.goto("/registry");
      await page.getByText("web", { exact: true }).click();
      await row(page, "0f1e2d3").locator("button:has(.mdi-delete-outline)").click();
      await expect(app.dialog("Delete tag?")).toBeVisible();
      await app.snap("registry/delete-dialog");
    });

    test("not enabled", async ({ app, page, mockState }) => {
      mockState.features.registry = false;
      await app.goto("/registry");
      await expect(page.getByText("Registry not enabled")).toBeVisible();
      await app.snap("registry/not-enabled", { fullPage: true });
    });
  });
});

test.describe("responsive", () => {
  for (const width of [1200, 900]) {
    test.describe(`${width}px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("settings general", async ({ app }) => {
        await app.goto("/settings");
        await app.snap("settings/general", { fullPage: true });
      });
    });
  }
});
