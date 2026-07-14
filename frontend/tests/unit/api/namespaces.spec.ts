import { describe, expect, it } from "vitest";
import { namespacesApi } from "@/api/namespaces";
import { mockFetchOnce } from "../helpers/mockFetch";

describe("namespacesApi", () => {
  it("list() returns the namespace list", async () => {
    const fetchMock = mockFetchOnce({
      body: { namespaces: ["default", "team-a"] },
    });
    const result = await namespacesApi.list();
    expect(fetchMock.mock.calls[0][0]).toBe("/api/namespaces");
    expect(result.namespaces).toEqual(["default", "team-a"]);
  });

  it("create() POSTs the body", async () => {
    const fetchMock = mockFetchOnce({
      status: 201,
      body: { name: "new-ns", created_at: null, labels: {} },
    });
    const result = await namespacesApi.create({ name: "new-ns" });
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/namespaces");
    expect(init?.method).toBe("POST");
    expect(JSON.parse(init?.body as string)).toEqual({ name: "new-ns" });
    expect(result.name).toBe("new-ns");
  });
});
