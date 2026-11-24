import { apiError } from "./support/mock-api";
import { expect, test } from "./fixtures";

test.describe("Error states", () => {
  test("deployments list shows the API error message on a 500", async ({ page, mockApi }) => {
    mockApi.on("GET", "/namespaces/:ns/deployments", apiError(500, "the server is down for maintenance", "kube_error"));

    await page.goto("/deployments");
    await expect(page.getByRole("alert").filter({ hasText: "the server is down for maintenance" })).toBeVisible();
  });

  test("deployment detail 404 shows the error and keeps the app usable", async ({ page, mockApi }) => {
    mockApi.on(
      "GET",
      "/namespaces/:ns/deployments/nonexistent",
      apiError(404, "Resource not found: deployment/nonexistent", "not_found"),
    );

    await page.goto("/deployments/default/nonexistent");
    await expect(page.getByRole("alert").filter({ hasText: "Resource not found: deployment/nonexistent" })).toBeVisible();
    await page.locator(".v-app-bar").getByRole("link", { name: "Resources" }).click();
    await expect(page).toHaveURL(/\/deployments$/);
  });

  test("a forbidden namespace surfaces the 403 message", async ({ page, mockApi }) => {
    mockApi.on(
      "GET",
      "/namespaces/team-a/deployments",
      apiError(403, "Namespace 'team-a' is not in the allowed list", "namespace_not_allowed"),
    );

    await page.goto("/deployments");
    await expect(page.getByRole("cell", { name: "web", exact: true })).toBeVisible();

    await page.getByTestId("namespace-switcher").locator("input").click();
    await page.getByRole("option", { name: "team-a" }).click();

    await expect(
      page.getByRole("alert").filter({ hasText: "Namespace 'team-a' is not in the allowed list" }),
    ).toBeVisible();
  });
});
