import { FIXED_NOW } from "./support/data";
import { ok } from "./support/mock-api";
import { expect, test } from "./fixtures";
import { openMenu } from "./support/ui";

test.describe("App shell", () => {
  test.describe("namespace switcher", () => {
    test("lists the namespaces, a divider and Create Namespace…", async ({ app, page }) => {
      await app.goto("/applications");
      await app.namespaceSwitcher().click();

      const menu = page.getByRole("listbox");
      await expect(menu.getByRole("option")).toHaveText(["default", "staging", "team-a", "Create Namespace…"]);
      // The divider is rendered by the custom #item slot between the
      // namespaces and the create action.
      await expect(menu.locator(".v-divider")).toHaveCount(1);
      await expect(menu.getByRole("option", { name: "Create Namespace…" }).locator(".mdi-plus")).toBeVisible();
    });

    test("choosing Create Namespace… opens the dialog and keeps the selection", async ({ app, page, mockApi }) => {
      await app.goto("/applications");
      await app.namespaceSwitcher().click();
      await page.getByRole("option", { name: "Create Namespace…" }).click();

      await expect(app.dialog("Create Namespace")).toBeVisible();
      await expect(page.getByTestId("namespace-switcher")).toContainText("default");
      expect(await page.evaluate(() => localStorage.getItem("deckwatch-namespace"))).toBe("default");
      // The sentinel must never reach the API as a namespace.
      expect(mockApi.calls.some((c) => c.path.includes("__create_namespace__"))).toBe(false);
    });

    test("typing filters the namespaces", async ({ app, page }) => {
      await app.goto("/applications");
      await app.namespaceSwitcher().click();
      await app.namespaceSwitcher().press("ControlOrMeta+a");
      await app.namespaceSwitcher().pressSequentially("stag");

      await expect(page.getByRole("listbox").getByRole("option")).toHaveText(["staging"]);
    });

    test("can be operated with the keyboard", async ({ app, page, mockApi }) => {
      await app.goto("/applications");
      // The switcher is the first focusable control on the page.
      await page.keyboard.press("Tab");
      await expect(app.namespaceSwitcher()).toBeFocused();

      await page.keyboard.press("ArrowDown");
      await expect(page.getByRole("listbox")).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(page.getByRole("listbox")).toBeHidden();

      // Type to filter; Enter picks the first match (auto-select-first).
      await page.keyboard.press("ControlOrMeta+a");
      await page.keyboard.type("stag");
      await expect(page.getByRole("listbox").getByRole("option")).toHaveText(["staging"]);
      await page.keyboard.press("Enter");

      await mockApi.waitForRequest("GET", "/namespaces/staging/applications");
      await expect(page.getByTestId("namespace-switcher")).toContainText("staging");
    });
  });

  test.describe("create namespace dialog", () => {
    test("posts the name and labels and selects the new namespace", async ({ app, page, mockApi }) => {
      await app.goto("/applications");
      await app.namespaceSwitcher().click();
      await page.getByRole("option", { name: "Create Namespace…" }).click();

      const dialog = app.dialog("Create Namespace");
      await app.autofocused(dialog.getByLabel("Name"));
      await dialog.getByLabel("Name").fill("payments");
      await dialog.getByLabel("Labels (optional)").fill("team=billing\ncost-center=42");
      await dialog.getByRole("button", { name: "Create" }).click();

      const call = await mockApi.waitForRequest("POST", "/namespaces");
      expect(call.body).toEqual({ name: "payments", labels: { team: "billing", "cost-center": "42" } });
      await expect(dialog).toBeHidden();
      await expect(page.getByTestId("namespace-switcher")).toContainText("payments");
      await mockApi.waitForRequest("GET", "/namespaces/payments/applications");
    });

    test("rejects an invalid name without calling the API", async ({ app, page, mockApi }) => {
      await app.goto("/applications");
      await app.namespaceSwitcher().click();
      await page.getByRole("option", { name: "Create Namespace…" }).click();

      const dialog = app.dialog("Create Namespace");
      await app.autofocused(dialog.getByLabel("Name"));
      await dialog.getByLabel("Name").fill("Bad_Name");
      await expect(dialog.getByText("Resource names cannot contain underscores", { exact: false })).toBeVisible();
      await dialog.getByRole("button", { name: "Create" }).click();

      await expect(dialog.getByText("Invalid namespace name")).toBeVisible();
      expect(mockApi.requests("POST", "/namespaces")).toHaveLength(0);
    });

    test("is a modal dialog that takes focus and closes on Escape", async ({ app, page }) => {
      await app.goto("/applications");
      await app.namespaceSwitcher().click();
      await page.getByRole("option", { name: "Create Namespace…" }).click();

      const dialog = page.getByRole("dialog");
      await expect(dialog).toHaveAttribute("aria-modal", "true");
      await expect(dialog.getByLabel("Name")).toBeFocused();

      await page.keyboard.press("Escape");
      await expect(dialog).toBeHidden();
    });
  });

  test("the theme toggle switches theme and survives a reload", async ({ app, page }) => {
    await app.goto("/applications");
    const appRoot = page.locator(".v-application");
    await expect(appRoot).toHaveClass(/v-theme--deckwatchDark/);

    await page.locator(".v-app-bar").locator("button:has(.mdi-weather-sunny)").click();
    await expect(appRoot).toHaveClass(/v-theme--deckwatchLight/);
    expect(await page.evaluate(() => localStorage.getItem("deckwatch-theme"))).toBe("deckwatchLight");

    await app.reload();
    await expect(appRoot).toHaveClass(/v-theme--deckwatchLight/);
    await expect(page.locator(".v-app-bar").locator("button:has(.mdi-weather-night)")).toBeVisible();
  });

  test("the global snackbar appears top right and Close dismisses it", async ({ app, page }) => {
    await app.goto("/applications/default/billing");
    await page.getByRole("tab", { name: "Plugins" }).click();
    await page.getByRole("button", { name: "Add Plugin" }).click();
    const dialog = app.dialog("Add Plugin");
    await openMenu(dialog, "Plugin");
    await page.getByRole("option", { name: "otel" }).click();
    await dialog.getByRole("button", { name: "Associate" }).click();

    const snackbar = page.locator(".v-snackbar").filter({ hasText: 'Plugin "otel" associated' });
    await expect(snackbar).toBeVisible();
    const box = await snackbar.locator(".v-snackbar__wrapper").boundingBox();
    const viewport = page.viewportSize()!;
    expect(box!.y).toBeLessThan(100);
    expect(box!.x + box!.width).toBeGreaterThan(viewport.width - 100);

    await snackbar.getByRole("button", { name: "Close" }).click();
    await expect(snackbar).toBeHidden();
  });

  test("a new warning event raises a cluster alert toast that can be dismissed", async ({ app, page, mockApi, mockState }) => {
    let polls = 0;
    mockApi.on("GET", "/events", () => {
      polls += 1;
      const at = new Date(FIXED_NOW.getTime() + 60_000).toISOString();
      const fresh = {
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
      };
      return ok({ events: polls === 1 ? mockState.events : [...mockState.events, fresh] });
    });
    await app.goto("/applications");
    // Existing warnings never toast: the first poll only sets the watermark.
    await expect(page.locator(".alert-toast")).toHaveCount(0);

    await app.runTimers(15_000);
    const toast = page.getByRole("alert").filter({ hasText: "Unhealthy" });
    await expect(toast).toContainText("default / Pod/api-7f6d5c4b3-2hjkl");
    await toast.getByRole("button", { name: "Dismiss alert" }).click();
    await expect(toast).toBeHidden();
  });

  test("an open dialog locks page scroll", async ({ app, page }) => {
    await app.goto("/deployments/default/web");
    const scrollTop = () => page.evaluate(() => document.body.scrollTop);
    // The app scrolls <body> (index.css: html, body { height: 100%; overflow: auto }).
    // Scroll a trigger far down the page into view, so opening the dialog
    // does not need to scroll.
    const trigger = page.getByRole("button", { name: "Roll back" }).first();
    await trigger.scrollIntoViewIfNeeded();
    const before = await scrollTop();
    expect(before).toBeGreaterThan(300);

    await trigger.click();
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByText("Roll back deployment")).toBeVisible();
    await app.settle();
    expect(await page.evaluate(() => getComputedStyle(document.body).overflowY)).toBe("hidden");
    expect(await scrollTop()).toBe(before);

    // Wheel over the backdrop, away from the dialog card.
    await page.mouse.move(60, 700);
    await page.mouse.wheel(0, 600);
    await app.settle();
    expect(await scrollTop()).toBe(before);

    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toBeHidden();
    await app.settle();
    expect(await page.evaluate(() => getComputedStyle(document.body).overflowY)).toBe("auto");
    expect(await scrollTop()).toBe(before);
  });
});
