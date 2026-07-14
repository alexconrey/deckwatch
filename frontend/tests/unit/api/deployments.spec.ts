import { describe, expect, it } from "vitest";
import { deploymentsApi } from "@/api/deployments";
import { ApiError } from "@/api/client";
import { mockFetchOnce } from "../helpers/mockFetch";

const detailFixture = {
  name: "web",
  namespace: "default",
  image: "nginx:1.25",
  replicas: { desired: 2, ready: 2, available: 2, updated: 2 },
  status: "available",
  created_at: null,
  labels: { app: "web" },
  conditions: [],
  env: [],
  command: [],
  args: [],
  resource_limits: null,
  resource_requests: null,
  liveness_probe: null,
  readiness_probe: null,
  startup_probe: null,
  pods: [],
  ingresses: [],
};

describe("deploymentsApi", () => {
  it("list() targets the namespace-scoped URL", async () => {
    const fetchMock = mockFetchOnce({ body: { deployments: [] } });
    await deploymentsApi.list("default");
    expect(fetchMock.mock.calls[0][0]).toBe("/api/namespaces/default/deployments");
  });

  it("get() targets the deployment-scoped URL", async () => {
    const fetchMock = mockFetchOnce({ body: detailFixture });
    await deploymentsApi.get("default", "web");
    expect(fetchMock.mock.calls[0][0]).toBe("/api/namespaces/default/deployments/web");
  });

  it("create() POSTs a JSON body", async () => {
    const fetchMock = mockFetchOnce({ status: 201, body: detailFixture });
    await deploymentsApi.create("default", { name: "web", image: "nginx:1.25" });
    const [, init] = fetchMock.mock.calls[0];
    expect(init?.method).toBe("POST");
    expect(JSON.parse(init?.body as string)).toEqual({
      name: "web",
      image: "nginx:1.25",
    });
  });

  it("update() PUTs a JSON body", async () => {
    const fetchMock = mockFetchOnce({ body: detailFixture });
    await deploymentsApi.update("default", "web", { image: "nginx:1.26" });
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/namespaces/default/deployments/web");
    expect(init?.method).toBe("PUT");
    expect(JSON.parse(init?.body as string)).toEqual({ image: "nginx:1.26" });
  });

  it("delete() sends DELETE and expects no body", async () => {
    const fetchMock = mockFetchOnce({ status: 204 });
    await deploymentsApi.delete("default", "web");
    expect(fetchMock.mock.calls[0][1]?.method).toBe("DELETE");
  });

  it("restart() sends POST to /restart", async () => {
    const fetchMock = mockFetchOnce({ body: { message: "rolling restart initiated" } });
    const r = await deploymentsApi.restart("default", "web");
    expect(fetchMock.mock.calls[0][0]).toBe(
      "/api/namespaces/default/deployments/web/restart",
    );
    expect(r.message).toBe("rolling restart initiated");
  });

  it("scale() posts the desired replica count", async () => {
    const fetchMock = mockFetchOnce({ body: detailFixture });
    await deploymentsApi.scale("default", "web", 5);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/namespaces/default/deployments/web/scale");
    expect(init?.method).toBe("POST");
    expect(JSON.parse(init?.body as string)).toEqual({ replicas: 5 });
  });

  it("getYaml() sets Accept: text/yaml and returns text", async () => {
    const fetchMock = mockFetchOnce({
      body: "apiVersion: apps/v1\nkind: Deployment",
      contentType: "text/yaml",
    });
    const yaml = await deploymentsApi.getYaml("default", "web");
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/namespaces/default/deployments/web/yaml");
    expect((init?.headers as Record<string, string>)?.Accept).toBe("text/yaml");
    expect(yaml).toContain("kind: Deployment");
  });

  it("getYaml() throws ApiError on non-2xx", async () => {
    mockFetchOnce({
      status: 404,
      body: { error: "not_found", message: "gone" },
    });
    await expect(deploymentsApi.getYaml("default", "web")).rejects.toBeInstanceOf(
      ApiError,
    );
  });

  it("updateYaml() sends Content-Type: text/yaml with raw body", async () => {
    const fetchMock = mockFetchOnce({ body: detailFixture });
    const yaml = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\n";
    await deploymentsApi.updateYaml("default", "web", yaml);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/namespaces/default/deployments/web/yaml");
    expect(init?.method).toBe("PUT");
    expect((init?.headers as Record<string, string>)?.["Content-Type"]).toBe(
      "text/yaml",
    );
    expect(init?.body).toBe(yaml);
  });

  it("updateProbes() sends PATCH", async () => {
    const fetchMock = mockFetchOnce({ body: detailFixture });
    await deploymentsApi.updateProbes("default", "web", {
      liveness_probe: null,
      readiness_probe: {
        probe_type: "httpGet",
        path: "/healthz",
        port: 8080,
      },
    });
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/namespaces/default/deployments/web/probes");
    expect(init?.method).toBe("PATCH");
  });

  it("addContainer() POSTs to /containers", async () => {
    const fetchMock = mockFetchOnce({ status: 201, body: detailFixture });
    await deploymentsApi.addContainer("default", "web", {
      name: "sidecar",
      image: "busybox",
    });
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/namespaces/default/deployments/web/containers");
    expect(init?.method).toBe("POST");
  });

  it("removeContainer() sends DELETE to /containers/:name", async () => {
    const fetchMock = mockFetchOnce({ body: detailFixture });
    await deploymentsApi.removeContainer("default", "web", "sidecar");
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/namespaces/default/deployments/web/containers/sidecar");
    expect(init?.method).toBe("DELETE");
  });
});
