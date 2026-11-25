import { describe, expect, it } from "vitest";
import { auditApi } from "@/api/audit";
import { prometheusQueryApi } from "@/api/prometheusQuery";
import { mockFetchOnce } from "../helpers/mockFetch";

describe("auditApi", () => {
  it("lists without parameters", async () => {
    const fetchMock = mockFetchOnce({ body: { items: [], next_cursor: null } });
    const page = await auditApi.list();
    expect(fetchMock.mock.calls[0][0]).toBe("/api/audit");
    expect(page).toEqual({ items: [], next_cursor: null });
  });

  it("passes the MCP filters and skips empty ones", async () => {
    const fetchMock = mockFetchOnce({ body: { items: [], next_cursor: "c" } });
    await auditApi.list({
      source: "mcp",
      action: "delete",
      client: "claude code",
      outcome: "denied",
      tool: "",
      since: "2026-11-24T00:00:00.000Z",
      limit: 50,
      cursor: "1795000000000000000~abc",
    });
    const url = new URL(fetchMock.mock.calls[0][0] as string, "http://deckwatch");
    expect(url.pathname).toBe("/api/audit");
    expect(Object.fromEntries(url.searchParams)).toEqual({
      source: "mcp",
      action: "delete",
      client: "claude code",
      outcome: "denied",
      since: "2026-11-24T00:00:00.000Z",
      limit: "50",
      cursor: "1795000000000000000~abc",
    });
  });
});

describe("prometheusQueryApi.mcpActivity", () => {
  it("asks for the window", async () => {
    const fetchMock = mockFetchOnce({
      body: {
        window_seconds: 3600,
        step_seconds: 60,
        calls_per_minute: [],
        apply_manifest_per_minute: [],
        p95_seconds: [],
        tools: [],
      },
    });
    const r = await prometheusQueryApi.mcpActivity("1h");
    expect(fetchMock.mock.calls[0][0]).toBe("/api/prometheus/mcp_activity?window=1h");
    expect(r.window_seconds).toBe(3600);
  });
});
