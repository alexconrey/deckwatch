import { describe, expect, it } from "vitest";
import { pluginStatusApi } from "@/api/pluginStatus";
import { statusTarget, worstHealth } from "@/utils/pluginEvents";
import { mockFetchOnce } from "../helpers/mockFetch";
import type { PluginStatus } from "@/types/api";

describe("pluginStatusApi", () => {
  it("asks for failing and degraded targets for the Problems view", async () => {
    const fetchMock = mockFetchOnce({ body: [] });
    await pluginStatusApi.problems();
    expect(fetchMock.mock.calls[0][0]).toBe("/api/plugin-status?health=failing%2Cdegraded");
  });

  it("builds the other URLs", async () => {
    let fetchMock = mockFetchOnce({ body: [] });
    await pluginStatusApi.list();
    expect(fetchMock.mock.calls[0][0]).toBe("/api/plugin-status");
    fetchMock = mockFetchOnce({ body: [] });
    await pluginStatusApi.list({ plugin: "aws", namespace: "shop" });
    expect(fetchMock.mock.calls[0][0]).toBe("/api/plugin-status?plugin=aws&namespace=shop");
    fetchMock = mockFetchOnce({ body: [] });
    await pluginStatusApi.forApplication("shop", "web");
    expect(fetchMock.mock.calls[0][0]).toBe("/api/namespaces/shop/applications/web/plugin-status");
    fetchMock = mockFetchOnce({ body: [] });
    await pluginStatusApi.forPlugin("my plugin");
    expect(fetchMock.mock.calls[0][0]).toBe("/api/plugins/my%20plugin/status");
  });
});

describe("status helpers", () => {
  const s = (overrides: Partial<PluginStatus>) =>
    ({ namespace: "shop", application: "web", plugin: "aws", resource_id: "", health: "healthy", ...overrides }) as PluginStatus;

  it("picks the worst health", () => {
    expect(worstHealth([])).toBeNull();
    expect(worstHealth([s({}), s({ health: "degraded" })])).toBe("degraded");
    expect(worstHealth([s({ health: "degraded" }), s({ health: "failing" }), s({})])).toBe("failing");
  });

  it("names the target", () => {
    expect(statusTarget(s({ resource_id: "rds" }))).toBe("shop/web · rds");
    expect(statusTarget(s({}))).toBe("shop/web · apply");
    expect(statusTarget(s({ namespace: "", application: "" }))).toBe("aws (load)");
  });
});
