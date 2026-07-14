import { test as base, expect, type Page, type Route } from "@playwright/test";

/**
 * Extended test fixture that installs default mock handlers for the
 * deckwatch API. Individual tests can override a handler by calling
 * `mockApi.override(pattern, handler)` before navigating.
 *
 * All routes fall through to the real backend if not matched — set
 * `PLAYWRIGHT_MOCK_MODE=off` to disable all mocking (for real-backend
 * runs).
 */

export interface Fixtures {
  namespace: string;
  deploymentName: string;
  mockApi: {
    override(
      pattern: string | RegExp,
      handler: (route: Route) => Promise<void> | void,
    ): Promise<void>;
  };
}

const useMocks = process.env.PLAYWRIGHT_MOCK_MODE !== "off";

const fixtureData = {
  namespaces: ["default", "team-a", "team-b"],
  deployments: (ns: string) => ({
    deployments: [
      {
        name: "web",
        namespace: ns,
        image: "nginx:1.25",
        replicas: { desired: 3, ready: 3, available: 3, updated: 3 },
        status: "available",
        created_at: new Date(Date.now() - 3 * 60 * 60 * 1000).toISOString(),
        labels: { app: "web" },
      },
      {
        name: "worker",
        namespace: ns,
        image: "worker:v1",
        replicas: { desired: 2, ready: 1, available: 1, updated: 2 },
        status: "degraded",
        created_at: new Date(Date.now() - 30 * 60 * 1000).toISOString(),
        labels: { app: "worker" },
      },
    ],
  }),
  deployment: (ns: string, name: string) => ({
    name,
    namespace: ns,
    image: "nginx:1.25",
    replicas: { desired: 3, ready: 3, available: 3, updated: 3 },
    status: "available",
    created_at: new Date(Date.now() - 3 * 60 * 60 * 1000).toISOString(),
    labels: { app: name },
    conditions: [
      {
        condition_type: "Available",
        status: "True",
        reason: "MinimumReplicasAvailable",
        message: null,
        last_transition: null,
      },
    ],
    env: [{ name: "PORT", value: "8080" }],
    command: [],
    args: [],
    resource_limits: { cpu: "500m", memory: "512Mi" },
    resource_requests: { cpu: "100m", memory: "128Mi" },
    liveness_probe: null,
    readiness_probe: null,
    startup_probe: null,
    pods: [
      {
        name: `${name}-abc123-x1`,
        phase: "Running",
        ready: true,
        restart_count: 0,
        node: "node-a",
        started_at: new Date().toISOString(),
        conditions: [],
        container_statuses: [
          {
            name,
            ready: true,
            restart_count: 0,
            state: "running",
            state_reason: null,
            image: "nginx:1.25",
          },
        ],
      },
    ],
    ingresses: [],
  }),
  nodes: {
    nodes: [
      {
        name: "node-a",
        status: "Ready",
        roles: ["worker"],
        cpu_capacity: "8",
        memory_capacity: "32Gi",
        cpu_allocatable: "7",
        memory_allocatable: "30Gi",
        os_image: "Ubuntu 22.04",
        kernel_version: "5.15.0",
        kubelet_version: "v1.32.0",
        conditions: [],
        created_at: new Date(Date.now() - 24 * 60 * 60 * 1000).toISOString(),
      },
    ],
  },
};

async function installDefaultRoutes(page: Page) {
  if (!useMocks) return;

  await page.route("**/api/namespaces", async (route) => {
    if (route.request().method() === "POST") {
      const req = JSON.parse(route.request().postData() ?? "{}");
      return route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify({
          name: req.name,
          created_at: new Date().toISOString(),
          labels: req.labels ?? {},
        }),
      });
    }
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ namespaces: fixtureData.namespaces }),
    });
  });

  await page.route(
    /\/api\/namespaces\/[^/]+\/deployments$/,
    async (route) => {
      const url = new URL(route.request().url());
      const ns = url.pathname.split("/")[3];
      if (route.request().method() === "POST") {
        return route.fulfill({
          status: 201,
          contentType: "application/json",
          body: JSON.stringify(fixtureData.deployment(ns, "web")),
        });
      }
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(fixtureData.deployments(ns)),
      });
    },
  );

  await page.route(
    /\/api\/namespaces\/[^/]+\/deployments\/[^/]+$/,
    async (route) => {
      const parts = new URL(route.request().url()).pathname.split("/");
      const ns = parts[3];
      const name = parts[5];
      const method = route.request().method();
      if (method === "DELETE") {
        return route.fulfill({ status: 204 });
      }
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(fixtureData.deployment(ns, name)),
      });
    },
  );

  await page.route(
    /\/api\/namespaces\/[^/]+\/pods\/[^/]+$/,
    async (route) => {
      const parts = new URL(route.request().url()).pathname.split("/");
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          name: parts[5],
          phase: "Running",
          ready: true,
          restart_count: 0,
          node: "node-a",
          started_at: new Date().toISOString(),
          conditions: [
            {
              condition_type: "Ready",
              status: true,
              reason: null,
              message: null,
            },
          ],
          container_statuses: [
            {
              name: "web",
              ready: true,
              restart_count: 0,
              state: "running",
              state_reason: null,
              image: "nginx:1.25",
            },
          ],
        }),
      });
    },
  );

  await page.route("**/api/nodes", async (route) => {
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(fixtureData.nodes),
    });
  });

  await page.route("**/api/addons", async (route) => {
    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        addons: [
          {
            id: "redis",
            name: "Redis",
            description: "cache",
            image: "redis:7-alpine",
            default_port: 6379,
            default_env: [],
            default_resources: { cpu: "100m", memory: "128Mi" },
          },
        ],
      }),
    });
  });
}

export const test = base.extend<Fixtures>({
  namespace: ["default", { option: true }],
  deploymentName: ["web", { option: true }],
  mockApi: async ({ page }, use) => {
    await installDefaultRoutes(page);
    await use({
      override: async (pattern, handler) => {
        await page.route(pattern, handler);
      },
    });
  },
});

export { expect };
