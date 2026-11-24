import type { Locator } from "@playwright/test";
import { FIXED_NOW } from "../support/data";
import { ok } from "../support/mock-api";
import type { App, SnapOptions } from "../support/app";
import { eachTheme, expect, test } from "./visual-test";

/**
 * Before the shot, park the mouse and let click ripples finish. The last
 * click leaves the pointer over a control, and whether Chromium has applied
 * its hover state yet varies from run to run; Vuetify removes a ripple up to
 * ~550ms after the click, on a timer.
 */
async function snap(app: App, name: string, options?: SnapOptions): Promise<void> {
  await app.page.mouse.move(0, 0);
  await expect(app.page.locator(".v-ripple__container")).toHaveCount(0);
  await app.snap(name, options);
}

/**
 * The v-field around the select/autocomplete labelled `label`. A v-select's
 * <input> sits under the selection text, so clicks must go to the field.
 */
const field = (scope: Locator, label: string) =>
  scope.locator(".v-field").filter({ has: scope.page().getByRole("combobox", { name: label, exact: true }) });

/**
 * App shell: app bar namespace switcher, Create Namespace dialog, the global
 * snackbar and the cluster-alert toast stack.
 */

eachTheme(() => {
  test("namespace switcher menu", async ({ app, page }) => {
    await app.goto("/applications");
    await app.namespaceSwitcher().click();
    // The divider and the "Create Namespace…" row come from the #item slot,
    // which reads item.raw (Vuetify 3) — an empty menu means it threw.
    await expect(page.getByRole("option", { name: "Create Namespace…" })).toBeVisible();
    await snap(app, "shell/namespace-menu");
  });

  test("namespace switcher filtered by typing", async ({ app, page }) => {
    await app.goto("/applications");
    // Focusing the autocomplete puts the current selection in the input;
    // select it so typing replaces it.
    await app.namespaceSwitcher().click();
    await app.namespaceSwitcher().press("ControlOrMeta+a");
    await app.namespaceSwitcher().pressSequentially("team");
    await expect(page.getByRole("option")).toHaveCount(1);
    await snap(app, "shell/namespace-menu-filtered");
  });

  test("create namespace dialog", async ({ app, page }) => {
    await app.goto("/applications");
    await app.namespaceSwitcher().click();
    await page.getByRole("option", { name: "Create Namespace…" }).click();
    await expect(app.dialog("Create Namespace")).toBeVisible();
    await snap(app, "shell/create-namespace-dialog");
  });

  test("create namespace dialog with validation errors", async ({ app, page }) => {
    await app.goto("/applications");
    await app.namespaceSwitcher().click();
    await page.getByRole("option", { name: "Create Namespace…" }).click();
    const dialog = app.dialog("Create Namespace");
    await dialog.getByLabel("Name").fill("Bad_Name");
    await dialog.getByRole("button", { name: "Create" }).click();
    await expect(dialog.getByText("Invalid namespace name")).toBeVisible();
    await snap(app, "shell/create-namespace-invalid");
  });

  test("global snackbar", async ({ app, page }) => {
    await app.goto("/applications/default/billing");
    await page.getByRole("tab", { name: "Plugins" }).click();
    await page.getByRole("button", { name: "Add Plugin" }).click();
    const dialog = app.dialog("Add Plugin");
    await field(dialog, "Plugin").click();
    await page.getByRole("option", { name: "otel" }).click();
    await dialog.getByRole("button", { name: "Associate" }).click();
    await expect(page.getByText('Plugin "otel" associated')).toBeVisible();
    await snap(app, "shell/snackbar");
  });

  test("cluster alert toast", async ({ app, page, mockApi, mockState }) => {
    // The first events poll only records a watermark; a later poll that
    // returns a newer Warning in the selected namespace raises a toast.
    let polls = 0;
    mockApi.on("GET", "/events", () => {
      polls += 1;
      if (polls === 1) return ok({ events: mockState.events });
      const at = new Date(FIXED_NOW.getTime() + 60_000).toISOString();
      return ok({
        events: [
          ...mockState.events,
          {
            namespace: "default",
            name: "api-7f6d5c4b3-2hjkl.17d40",
            event_type: "Warning",
            reason: "Unhealthy",
            message: "Readiness probe failed: HTTP probe failed with statuscode: 503",
            involved_object_kind: "Pod",
            involved_object_name: "api-7f6d5c4b3-2hjkl",
            involved_object_namespace: "default",
            count: 3,
            first_timestamp: at,
            last_timestamp: at,
            source_component: "kubelet",
            source_host: "ip-10-0-2-37.ec2.internal",
          },
        ],
      });
    });
    await app.goto("/applications");
    await app.runTimers(15_000);
    await expect(page.getByRole("alert").filter({ hasText: "Unhealthy" })).toBeVisible();
    await snap(app, "shell/cluster-alert-toast");
  });
});
