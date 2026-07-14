import { describe, expect, it } from "vitest";
import { podsApi } from "@/api/pods";
import { mockFetchOnce } from "../helpers/mockFetch";

describe("podsApi", () => {
  it("listForDeployment() targets the deployment-scoped pods URL", async () => {
    const fetchMock = mockFetchOnce({ body: { pods: [] } });
    await podsApi.listForDeployment("default", "web");
    expect(fetchMock.mock.calls[0][0]).toBe(
      "/api/namespaces/default/deployments/web/pods",
    );
  });

  it("get() targets the pod URL", async () => {
    const fetchMock = mockFetchOnce({
      body: {
        name: "web-abc",
        phase: "Running",
        ready: true,
        restart_count: 0,
        node: "n1",
        started_at: null,
        conditions: [],
        container_statuses: [],
      },
    });
    const p = await podsApi.get("default", "web-abc");
    expect(fetchMock.mock.calls[0][0]).toBe("/api/namespaces/default/pods/web-abc");
    expect(p.phase).toBe("Running");
  });
});
