import { describe, expect, it, beforeEach } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useDeploymentsStore } from "@/stores/deployments";
import { mockFetchOnce } from "../helpers/mockFetch";

const summaryFixture = {
  name: "web",
  namespace: "default",
  image: "nginx:1.25",
  replicas: { desired: 2, ready: 2, available: 2, updated: 2 },
  status: "available",
  created_at: null,
  labels: {},
};

describe("useDeploymentsStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("initializes with empty state", () => {
    const s = useDeploymentsStore();
    expect(s.deployments).toEqual([]);
    expect(s.loading).toBe(false);
    expect(s.error).toBeNull();
  });

  it("fetchDeployments is a no-op when namespace is empty", async () => {
    const s = useDeploymentsStore();
    await s.fetchDeployments("");
    // fetch was never called → deployments remain empty and no error is set
    expect(s.deployments).toEqual([]);
    expect(s.error).toBeNull();
    expect(s.loading).toBe(false);
  });

  it("fetchDeployments populates the list", async () => {
    mockFetchOnce({ body: { deployments: [summaryFixture] } });
    const s = useDeploymentsStore();
    await s.fetchDeployments("default");
    expect(s.deployments).toHaveLength(1);
    expect(s.deployments[0].name).toBe("web");
    expect(s.loading).toBe(false);
    expect(s.error).toBeNull();
  });

  it("fetchDeployments records the error message on failure", async () => {
    mockFetchOnce({
      status: 403,
      body: {
        error: "namespace_not_allowed",
        message: "Namespace 'kube-system' is not in the allowed list",
      },
    });
    const s = useDeploymentsStore();
    await s.fetchDeployments("kube-system");
    expect(s.error).toContain("not in the allowed list");
    expect(s.loading).toBe(false);
  });
});
