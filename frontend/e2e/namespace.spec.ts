import { expect, test } from "./fixtures";

test.describe("Namespace selection", () => {
  test("namespace list is fetched on landing", async ({ page, mockApi }) => {
    await page.goto("/deployments");
    await mockApi.waitForRequest("GET", "/namespaces");
    await expect(page.getByTestId("namespace-switcher")).toContainText("default");
  });

  test("switching namespaces re-fetches deployments and remembers the choice", async ({ page, mockApi }) => {
    await page.goto("/deployments");
    await mockApi.waitForRequest("GET", "/namespaces/default/deployments");

    // The switcher is the v-autocomplete with data-testid="namespace-switcher"
    // (layouts/AppLayout.vue); its menu entries are role="option".
    await page.getByTestId("namespace-switcher").locator("input").click();
    await page.getByRole("option", { name: "team-a" }).click();

    await mockApi.waitForRequest("GET", "/namespaces/team-a/deployments");
    await expect(page.getByText("No deployments in this namespace")).toBeVisible();
    expect(await page.evaluate(() => localStorage.getItem("deckwatch-namespace"))).toBe("team-a");
  });
});
