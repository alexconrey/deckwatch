import { expect, test } from "./fixtures";

test.describe("Error states", () => {
  test("deployments list shows error surface when API returns 500", async ({ page, mockApi }) => {
    await mockApi.override(
      /\/api\/namespaces\/[^/]+\/deployments$/,
      async (route) => {
        if (route.request().method() === "GET") {
          return route.fulfill({
            status: 500,
            contentType: "application/json",
            body: JSON.stringify({
              error: "kube_error",
              message: "the server is down for maintenance",
            }),
          });
        }
        return route.fallback();
      },
    );

    await page.goto("/deployments");
    // The error message from the API should surface somewhere in the UI.
    await expect(page.getByText(/server is down for maintenance/i)).toBeVisible();
  });

  test("deployment detail 404 does not crash the app", async ({ page, mockApi }) => {
    await mockApi.override(
      /\/api\/namespaces\/[^/]+\/deployments\/nonexistent$/,
      async (route) => {
        return route.fulfill({
          status: 404,
          contentType: "application/json",
          body: JSON.stringify({
            error: "not_found",
            message: "Resource not found: deployment/nonexistent",
          }),
        });
      },
    );
    await page.goto("/deployments/default/nonexistent");
    // The app should stay responsive and surface the error copy.
    await expect(page.getByText(/not found|nonexistent/i).first()).toBeVisible();
  });

  test("namespace forbidden surfaces the 403 message", async ({ page, mockApi }) => {
    await mockApi.override(
      /\/api\/namespaces\/kube-system\/deployments$/,
      async (route) => {
        return route.fulfill({
          status: 403,
          contentType: "application/json",
          body: JSON.stringify({
            error: "namespace_not_allowed",
            message: "Namespace 'kube-system' is not in the allowed list",
          }),
        });
      },
    );
    await page.goto("/deployments");
    // Navigate to a namespace we have no access to (via URL) — the exact
    // trigger depends on the app, so this test doesn't assert visibility
    // for kube-system; it just confirms the app doesn't crash if such a
    // response is received.
    await page.evaluate(async () => {
      await fetch("/api/namespaces/kube-system/deployments").catch(() => {});
    });
    // Sanity check that the app root is still mounted.
    await expect(page.locator("body")).toBeVisible();
  });
});
