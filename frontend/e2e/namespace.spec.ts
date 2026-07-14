import { expect, test } from "./fixtures";

test.describe("Namespace selection", () => {
  test("namespace list is fetched on landing", async ({ page, mockApi }) => {
    let namespacesFetched = false;
    await mockApi.override("**/api/namespaces", async (route) => {
      if (route.request().method() === "GET") {
        namespacesFetched = true;
      }
      return route.fallback();
    });

    await page.goto("/deployments");
    await expect.poll(() => namespacesFetched).toBe(true);
  });

  test("switching namespaces triggers a re-fetch of deployments", async ({ page, mockApi }) => {
    const seenNamespaces: string[] = [];
    await mockApi.override(
      /\/api\/namespaces\/[^/]+\/deployments$/,
      async (route) => {
        const parts = new URL(route.request().url()).pathname.split("/");
        seenNamespaces.push(parts[3]);
        return route.fallback();
      },
    );

    await page.goto("/deployments");
    await expect.poll(() => seenNamespaces.length).toBeGreaterThan(0);

    // The namespace switcher exposes `role="combobox"` +
    // `aria-label="Select namespace"` (see frontend/src/layouts/AppLayout.vue).
    // The role selector is the primary path; the data-testid is a stable
    // fallback in case Vuetify wraps the v-select DOM differently in a
    // future upgrade.
    const nsSwitcher = page
      .getByRole("combobox", { name: /select namespace/i })
      .or(page.getByTestId("namespace-switcher"))
      .first();

    await expect(nsSwitcher).toBeVisible();

    const before = seenNamespaces.length;
    await nsSwitcher.click();

    // Menu items are exposed with `role="option"` and each carries the
    // namespace name as its aria-label so `getByRole("option", { name })`
    // works regardless of Vuetify's internal DOM structure.
    const teamA = page.getByRole("option", { name: "team-a" }).first();
    await teamA.click();

    await expect.poll(() => seenNamespaces.length).toBeGreaterThan(before);
    expect(seenNamespaces).toContain("team-a");
  });
});
