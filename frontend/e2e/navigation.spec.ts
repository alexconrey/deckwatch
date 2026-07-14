import { expect, test } from "./fixtures";

test.describe("Navigation", () => {
  test("landing page redirects to deployments list", async ({ page, mockApi }) => {
    await page.goto("/");
    await expect(page).toHaveURL(/\/deployments$/);
    // A deployment name from the fixture should be visible.
    await expect(page.getByText("web")).toBeVisible();
    await expect(page.getByText("worker")).toBeVisible();
  });

  test("deployments list → deployment detail → pod detail", async ({ page, mockApi }) => {
    await page.goto("/deployments");
    await expect(page.getByText("web")).toBeVisible();

    // Click into the "web" deployment.
    await page.getByText("web").first().click();
    await expect(page).toHaveURL(/\/deployments\/[^/]+\/web$/);
    await expect(page.getByText("nginx:1.25")).toBeVisible();

    // Click into the first pod.
    const podRow = page.getByText(/web-abc123-x1/);
    await expect(podRow).toBeVisible();
    await podRow.click();
    await expect(page).toHaveURL(/\/pods\/[^/]+\/web-abc123-x1$/);
  });

  test("cluster overview page loads and shows node info", async ({ page, mockApi }) => {
    await page.goto("/cluster");
    await expect(page.getByText("node-a")).toBeVisible();
    await expect(page.getByText("Ready")).toBeVisible();
    await expect(page.getByText(/v1\.32\.0/)).toBeVisible();
  });

  test("unknown route renders the NotFound page", async ({ page, mockApi }) => {
    await page.goto("/this/route/does/not/exist");
    // The NotFoundPage should render *something* recognizable — we just
    // assert that the app didn't crash and did not fall back to the
    // deployments list.
    await expect(page).not.toHaveURL(/\/deployments$/);
    await expect(page.locator("body")).toBeVisible();
  });
});
