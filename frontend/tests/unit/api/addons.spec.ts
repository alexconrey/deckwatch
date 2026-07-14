import { describe, expect, it } from "vitest";
import { addonsApi } from "@/api/addons";
import { mockFetchOnce } from "../helpers/mockFetch";

describe("addonsApi", () => {
  it("list() returns the catalog", async () => {
    const fetchMock = mockFetchOnce({
      body: {
        addons: [
          {
            id: "redis",
            name: "Redis",
            description: "cache",
            image: "redis:7-alpine",
            default_port: 6379,
            default_env: [],
            default_resources: null,
          },
        ],
      },
    });
    const r = await addonsApi.list();
    expect(fetchMock.mock.calls[0][0]).toBe("/api/addons");
    expect(r.addons).toHaveLength(1);
    expect(r.addons[0].id).toBe("redis");
  });

  it("attach() defaults to empty body when overrides are not provided", async () => {
    const fetchMock = mockFetchOnce({ status: 201, body: {} });
    await addonsApi.attach("default", "web", "redis");
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/namespaces/default/deployments/web/addons/redis");
    expect(init?.method).toBe("POST");
    expect(init?.body).toBe("{}");
  });

  it("attach() forwards override body", async () => {
    const fetchMock = mockFetchOnce({ status: 201, body: {} });
    await addonsApi.attach("default", "web", "redis", { port: 16379 });
    const [, init] = fetchMock.mock.calls[0];
    expect(JSON.parse(init?.body as string)).toEqual({ port: 16379 });
  });

  it("detach() sends DELETE", async () => {
    const fetchMock = mockFetchOnce({ body: {} });
    await addonsApi.detach("default", "web", "redis");
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/namespaces/default/deployments/web/addons/redis");
    expect(init?.method).toBe("DELETE");
  });
});
