import { describe, expect, it } from "vitest";
import { nodesApi } from "@/api/nodes";
import { mockFetchOnce } from "../helpers/mockFetch";

describe("nodesApi", () => {
  it("list() returns nodes", async () => {
    const fetchMock = mockFetchOnce({
      body: {
        nodes: [
          {
            name: "n1",
            status: "Ready",
            roles: ["worker"],
            cpu_capacity: "8",
            memory_capacity: "32Gi",
            cpu_allocatable: null,
            memory_allocatable: null,
            os_image: null,
            kernel_version: null,
            kubelet_version: null,
            conditions: [],
            created_at: null,
          },
        ],
      },
    });
    const r = await nodesApi.list();
    expect(fetchMock.mock.calls[0][0]).toBe("/api/nodes");
    expect(r.nodes[0].status).toBe("Ready");
  });
});
