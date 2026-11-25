import { expect, test } from "./fixtures";
import { FIXED_NOW } from "./support/data";
import { choose, openMenu } from "./support/ui";

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

    await openMenu(page.locator("main"), "Resource Type");
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
    await openMenu(page.locator("main"), "Items per page:");
    await expect(page.getByRole("listbox").getByRole("option")).toHaveText(["10", "25", "50", "100"]);
  });
});

test.describe("MCP activity", () => {
  const DAY_AGO = new Date(FIXED_NOW.getTime() - 24 * 3600 * 1000).toISOString();
  const WEEK_AGO = new Date(FIXED_NOW.getTime() - 7 * 24 * 3600 * 1000).toISOString();

  test("the app bar opens it, and it loads the metrics and the MCP rows of the last 24 hours", async ({
    app,
    page,
    mockApi,
  }) => {
    await app.goto("/applications");
    await page.getByTestId("mcp-activity-nav").click();
    await expect(page).toHaveURL(/\/mcp-activity$/);

    const metrics = await mockApi.waitForRequest("GET", "/prometheus/mcp_activity");
    expect(metrics.query.get("window")).toBe("24h");
    const rows = mockApi.requests("GET", "/audit").at(-1)!;
    expect(Object.fromEntries(rows.query)).toEqual({ source: "mcp", since: DAY_AGO, limit: "50" });

    await expect(page.getByTestId("kpi-calls")).toHaveText("3,286");
    await expect(page.getByTestId("kpi-denied")).toHaveText("7");
    await expect(page.getByTestId("audit-row")).toHaveCount(4);
    const denied = page.getByTestId("audit-row").filter({ hasText: "delete_pod" });
    await expect(denied).toContainText("denied");
    await expect(denied.getByTestId("denied-rule")).toHaveText("namespace_allowlist");
    await expect(denied).toContainText("name=etcd-0 namespace=kube-system");
    await expect(denied.getByTestId("client-says")).toHaveText("client says claude-code 2.1.0");
    await expect(page.getByText("1 denied", { exact: true })).toBeVisible();
  });

  test("the window toggle refetches both for the last 7 days", async ({ app, page, mockApi }) => {
    await app.goto("/mcp-activity");
    await page.getByRole("button", { name: "Last 7 days" }).click();
    await expect
      .poll(() => mockApi.requests("GET", "/prometheus/mcp_activity").map((c) => c.query.get("window")))
      .toContain("7d");
    await expect
      .poll(() => mockApi.requests("GET", "/audit").map((c) => c.query.get("since")))
      .toContain(WEEK_AGO);
  });

  test("the outcome filter narrows the rows", async ({ app, page, mockApi }) => {
    await app.goto("/mcp-activity");
    await choose(page.getByTestId("filter-outcome"), "Outcome", "denied");
    await expect.poll(() => mockApi.requests("GET", "/audit").some((c) => c.query.get("outcome") === "denied")).toBe(true);
    await expect(page.getByTestId("audit-row")).toHaveCount(1);
    await expect(page.getByTestId("audit-row")).toContainText("delete_pod");
  });

  test("without Prometheus it says so and still lists the audit rows", async ({ app, page, mockState }) => {
    mockState.features.prometheus = false;
    await app.goto("/mcp-activity");
    await expect(page.getByTestId("metrics-unavailable")).toContainText("prometheus not configured");
    await expect(page.getByTestId("kpi-row")).toHaveCount(0);
    await expect(page.getByTestId("audit-row")).toHaveCount(4);
  });

  test("the audit log links to it and shows each row's source", async ({ app, page }) => {
    await app.goto("/audit");
    const table = page.locator(".v-data-table");
    await expect(table.getByRole("row").filter({ hasText: "etcd-0" })).toContainText("MCP delete_pod");
    await expect(table.getByRole("row").filter({ hasText: "cache-warmer" })).toContainText("UI/API");
    await page.getByRole("link", { name: "MCP activity" }).click();
    await expect(page).toHaveURL(/\/mcp-activity$/);
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
