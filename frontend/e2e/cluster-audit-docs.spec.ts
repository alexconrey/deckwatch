import { expect, test } from "./fixtures";
import { field } from "./support/ui";

test.describe("Cluster overview", () => {
  test("shows node counts and expands a node to its conditions", async ({ app, page }) => {
    await app.goto("/cluster");
    const stat = (label: string) => page.locator(".v-card").filter({ has: page.getByText(label, { exact: true }) });
    await expect(stat("Total Nodes")).toContainText("3");
    await expect(stat("Ready")).toContainText("2");
    await expect(stat("Not Ready")).toContainText("1");

    const row = page.locator(".v-data-table").getByRole("row").filter({ hasText: "ip-10-0-3-12" });
    await row.getByRole("button").first().click();
    await expect(page.getByText("Kubelet stopped posting node status.")).toBeVisible();
    await expect(page.getByText("6.1.92-99.174.amzn2023.x86_64").first()).toBeVisible();
  });

  test("the events feed shows warnings by default and all events on request", async ({ app, page }) => {
    await app.goto("/cluster");
    const feed = page.locator(".v-card").filter({ hasText: "Cluster Events" });
    await expect(feed.getByText("Back-off restarting failed container", { exact: false })).toBeVisible();
    await expect(feed.getByText("Scaled up replica set web-6c9f7d8b5 to 3")).toBeHidden();

    await feed.getByRole("button", { name: "All", exact: true }).click();
    await expect(feed.getByText("Scaled up replica set web-6c9f7d8b5 to 3")).toBeVisible();
    await feed.getByRole("button", { name: "Normal", exact: true }).click();
    await expect(feed.getByText("Back-off restarting failed container", { exact: false })).toBeHidden();
  });

  test("polls nodes and metrics every 10 seconds", async ({ app, mockApi }) => {
    await app.goto("/cluster");
    const before = mockApi.requests("GET", "/nodes").length;
    await app.runTimers(10_000);
    expect(mockApi.requests("GET", "/nodes").length).toBeGreaterThan(before);
    expect(mockApi.requests("GET", "/nodes/metrics").length).toBeGreaterThan(1);
  });
});

test.describe("Audit log", () => {
  test("filtering by resource type refetches with the filter in the query", async ({ app, page, mockApi }) => {
    await app.goto("/audit");
    const first = await mockApi.waitForRequest("GET", "/audit");
    expect(first.query.get("limit")).toBe("200");
    expect(first.query.has("resource_type")).toBe(false);

    await field(page.locator("main"), "Resource Type").click();
    await page.getByRole("option", { name: "cronjob" }).click();

    await expect
      .poll(() => mockApi.requests("GET", "/audit").some((c) => c.query.get("resource_type") === "cronjob"))
      .toBe(true);
    const table = page.locator(".v-data-table");
    await expect(table.getByRole("cell", { name: "cache-warmer" })).toBeVisible();
    await expect(table.getByRole("row")).toHaveCount(2); // header + one entry
  });

  test("items per page offers 10, 25, 50 and 100", async ({ app, page }) => {
    await app.goto("/audit");
    await field(page.locator("main"), "Items per page:").click();
    await expect(page.getByRole("listbox").getByRole("option")).toHaveText(["10", "25", "50", "100"]);
  });
});

test.describe("Docs", () => {
  test("opens the first page and navigates from the sidebar", async ({ app, page, mockApi }) => {
    await app.goto("/docs");
    await expect(page).toHaveURL(/\/docs\/getting-started$/);
    await expect(page.locator("article").getByRole("heading", { name: "Getting started" })).toBeVisible();
    await expect(page.locator("article pre")).toContainText("helm install deckwatch");

    await page.getByRole("link", { name: "GitOps", exact: true }).click();
    await mockApi.waitForRequest("GET", "/docs/pages/gitops");
    await expect(page).toHaveURL(/\/docs\/gitops$/);
    await expect(page.locator("article").getByRole("heading", { name: "GitOps" })).toBeVisible();
  });
});

test.describe("Agent feedback", () => {
  test("Mark reviewed patches the status and moves the item", async ({ app, page, mockApi }) => {
    await app.goto("/agent-feedback");
    const card = page.locator(".v-card").filter({ hasText: "No tool to read HPA events" });
    await card.getByRole("button", { name: "Mark reviewed" }).click();

    const call = await mockApi.waitForRequest("PATCH", "/agent-feedback/fb-001");
    expect(call.body).toEqual({ status: "reviewed" });
    await expect(card.getByRole("button", { name: "Reset to pending" })).toBeVisible();

    await page.getByRole("tab", { name: /Pending/ }).click();
    await expect(page.getByText("No agent feedback recorded yet")).toBeVisible();
  });
});

test.describe("Auth callback", () => {
  test("a visit without an id_token shows the sign-in error", async ({ app, page }) => {
    await app.goto("/auth/callback");
    await expect(page.getByRole("heading", { name: "Sign-in failed" })).toBeVisible();
    await expect(page.getByText("Entra redirect missing id_token")).toBeVisible();
    await expect(page.getByRole("button", { name: "Try again" })).toBeVisible();
  });
});
