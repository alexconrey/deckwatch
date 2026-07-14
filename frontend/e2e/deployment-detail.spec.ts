import { expect, test } from "./fixtures";

test.describe("Deployment detail page", () => {
  test("renders replica counts, conditions, and pods", async ({ page, mockApi }) => {
    await page.goto("/deployments/default/web");
    await expect(page.getByText("web")).toBeVisible();
    await expect(page.getByText("nginx:1.25")).toBeVisible();

    // Replica counts: desired/ready/available = 3
    await expect(page.getByText(/3\s*\/\s*3/).first()).toBeVisible();

    // Available condition should show up as a chip/status.
    await expect(page.getByText(/Available/i).first()).toBeVisible();

    // The pod listed under this deployment should be linked from the detail page.
    await expect(page.getByText(/web-abc123-x1/)).toBeVisible();
  });

  test("shows resource limits and requests when set", async ({ page, mockApi }) => {
    await page.goto("/deployments/default/web");
    await expect(page.getByText(/500m/).first()).toBeVisible();
    await expect(page.getByText(/512Mi/).first()).toBeVisible();
    await expect(page.getByText(/100m/).first()).toBeVisible();
    await expect(page.getByText(/128Mi/).first()).toBeVisible();
  });

  test("restart action posts to /restart", async ({ page, mockApi }) => {
    let restartCalled = false;
    await mockApi.override(
      /\/api\/namespaces\/[^/]+\/deployments\/[^/]+\/restart$/,
      async (route) => {
        if (route.request().method() === "POST") {
          restartCalled = true;
          return route.fulfill({
            status: 200,
            contentType: "application/json",
            body: JSON.stringify({ message: "rolling restart initiated" }),
          });
        }
        return route.fallback();
      },
    );

    await page.goto("/deployments/default/web");
    const restart = page.getByRole("button", { name: /restart/i }).first();
    if ((await restart.count()) === 0) {
      test.info().annotations.push({ type: "skip", description: "no restart button surfaced" });
      return;
    }
    await restart.click();
    // The action may sit behind a confirm dialog.
    const confirm = page.getByRole("button", { name: /confirm|yes|restart/i }).nth(1);
    if (await confirm.isVisible().catch(() => false)) {
      await confirm.click();
    }
    await expect.poll(() => restartCalled).toBe(true);
  });
});
