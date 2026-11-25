import { expect, test } from "./fixtures";

test.describe("Deployment detail page", () => {
  test("renders replica counts, conditions, and pods", async ({ page }) => {
    await page.goto("/deployments/default/web");
    await expect(page.getByRole("heading", { name: "web", exact: true })).toBeVisible();
    await expect(page.locator(".v-chip", { hasText: "nginx:1.25" }).first()).toBeVisible();
    // Replica gauge in the status card (the rollout history shows "3/3" too).
    await expect(page.locator(".v-card").first().getByText("3/3", { exact: true })).toBeVisible();

    const conditions = page.locator(".v-card", { has: page.getByText("Conditions", { exact: true }) });
    await expect(conditions.getByRole("cell", { name: "Available", exact: true })).toBeVisible();
    await expect(conditions.getByRole("cell", { name: "MinimumReplicasAvailable" })).toBeVisible();

    await expect(page.getByText("Pods (3)")).toBeVisible();
    for (const pod of ["web-6c9f7d8b5-4xk2p", "web-6c9f7d8b5-9bqzt", "web-6c9f7d8b5-mr7wd"]) {
      await expect(page.getByRole("link", { name: pod })).toBeVisible();
    }
  });

  test("the edit form shows the resource requests and limits", async ({ page }) => {
    await page.goto("/deployments/default/web");
    await page.getByRole("button", { name: "Edit" }).first().click();

    const dialog = page.getByRole("dialog");
    await expect(dialog.getByLabel("CPU Request")).toHaveValue("250m");
    await expect(dialog.getByLabel("Memory Request")).toHaveValue("256Mi");
    await expect(dialog.getByLabel("CPU Limit")).toHaveValue("500m");
    await expect(dialog.getByLabel("Memory Limit")).toHaveValue("512Mi");
  });

  test("restart asks for confirmation and posts to /restart", async ({ page, mockApi }) => {
    await page.goto("/deployments/default/web");
    await page.getByRole("button", { name: "Restart" }).click();

    const dialog = page.getByRole("dialog");
    await expect(dialog.getByText("This will trigger a rolling restart of 'web'.")).toBeVisible();
    expect(mockApi.requests("POST", "/namespaces/:ns/deployments/:name/restart")).toHaveLength(0);

    await dialog.getByRole("button", { name: "Restart" }).click();
    const call = await mockApi.waitForRequest("POST", "/namespaces/default/deployments/web/restart");
    expect(call.body).toBeNull();
    await expect(dialog).toBeHidden();
  });
});
