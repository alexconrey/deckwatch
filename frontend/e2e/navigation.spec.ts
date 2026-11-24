import { expect, test } from "./fixtures";

test.describe("Navigation", () => {
  test("landing page redirects to the applications list", async ({ page }) => {
    await page.goto("/");
    await expect(page).toHaveURL(/\/applications$/);
    await expect(page.getByRole("heading", { name: "Applications" })).toBeVisible();
    await expect(page.getByRole("cell", { name: "storefront" })).toBeVisible();
    await expect(page.getByRole("cell", { name: "billing" })).toBeVisible();
  });

  test("deployments list → deployment detail → pod detail", async ({ page }) => {
    await page.goto("/deployments");
    await page.getByRole("row").filter({ hasText: "nginx:1.25" }).getByText("web", { exact: true }).click();

    await expect(page).toHaveURL(/\/deployments\/default\/web$/);
    await expect(page.getByRole("heading", { name: "web", exact: true })).toBeVisible();
    await expect(page.locator(".v-chip", { hasText: "nginx:1.25" }).first()).toBeVisible();

    await page.getByRole("link", { name: "web-6c9f7d8b5-4xk2p" }).click();
    await expect(page).toHaveURL(/\/pods\/default\/web-6c9f7d8b5-4xk2p$/);
    await expect(page.getByRole("heading", { name: "web-6c9f7d8b5-4xk2p" })).toBeVisible();
  });

  test("app bar links reach every top-level page", async ({ page }) => {
    await page.goto("/applications");
    const appBar = page.locator(".v-app-bar");

    await appBar.getByRole("link", { name: "Resources" }).click();
    await expect(page).toHaveURL(/\/deployments$/);
    await expect(page.getByRole("heading", { name: "Resources" })).toBeVisible();

    await appBar.getByRole("link", { name: "Registry" }).click();
    await expect(page).toHaveURL(/\/registry$/);

    await appBar.getByRole("button", { name: "Cluster" }).click();
    await expect(page).toHaveURL(/\/cluster$/);

    await appBar.locator("button:has(.mdi-cog)").click();
    await expect(page).toHaveURL(/\/settings$/);

    await appBar.getByText("Deckwatch", { exact: true }).click();
    await expect(page).toHaveURL(/\/applications$/);
  });

  test("cluster overview page loads and shows node info", async ({ page }) => {
    await page.goto("/cluster");
    const table = page.locator(".v-data-table");
    await expect(table.getByText("ip-10-0-1-21.ec2.internal")).toBeVisible();
    await expect(table.getByRole("row").filter({ hasText: "ip-10-0-1-21" }).getByText("Ready")).toBeVisible();
    await expect(table.getByRole("row").filter({ hasText: "ip-10-0-3-12" }).getByText("NotReady")).toBeVisible();
    await expect(table.getByText("v1.32.3-eks-1234567").first()).toBeVisible();
  });

  test("unknown route renders the NotFound page", async ({ page }) => {
    await page.goto("/this/route/does/not/exist");
    await expect(page.getByRole("heading", { name: "Page Not Found" })).toBeVisible();
    await page.getByRole("button", { name: "Go to Deployments" }).click();
    await expect(page).toHaveURL(/\/deployments$/);
  });
});
