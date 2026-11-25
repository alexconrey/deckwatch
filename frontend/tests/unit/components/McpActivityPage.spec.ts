import { beforeEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";
import type { AuditEntry, AuditPage, McpActivityResponse } from "@/types/api";

const { mockList, mockActivity, mockSettings } = vi.hoisted(() => ({
  mockList: vi.fn(),
  mockActivity: vi.fn(),
  mockSettings: vi.fn(),
}));

vi.mock("@/api/audit", () => ({ auditApi: { list: mockList } }));
vi.mock("@/api/prometheusQuery", () => ({ prometheusQueryApi: { mcpActivity: mockActivity } }));
vi.mock("@/api/settings", () => ({ settingsApi: { get: mockSettings } }));
// Chart.js needs a canvas; the chart itself has its own spec.
vi.mock("vue-chartjs", () => ({
  Line: { name: "Line", props: ["data", "options", "plugins"], template: "<canvas />" },
}));

import McpActivityPage from "@/components/pages/McpActivityPage.vue";

const vuetify = createVuetify({ components, directives });

function row(overrides: Partial<AuditEntry>, detail: object = {}): AuditEntry {
  return {
    id: overrides.id ?? "r1",
    timestamp: "2026-11-24T12:00:00Z",
    action: "write",
    resource_type: "deployment",
    resource_name: "web",
    namespace: "prod",
    user_identity: "",
    source: "mcp",
    tool: "apply_manifest",
    outcome: "ok",
    client_name: "",
    detail: JSON.stringify({
      tool: overrides.tool ?? "apply_manifest",
      outcome: overrides.outcome ?? "ok",
      argument_keys: ["manifest"],
      arguments: {},
      duration_ms: 12,
      ...detail,
    }),
    ...overrides,
  };
}

const rows: AuditEntry[] = [
  row(
    { id: "r1", client_name: "claude-code" },
    {
      client: { name: "claude-code", version: "2.1.0", self_reported: true },
      trace_id: "4bf92f3577b34da6a3ce929d0e0e4736",
    },
  ),
  row(
    {
      id: "r2",
      tool: "delete_pod",
      action: "delete",
      outcome: "denied",
      namespace: "kube-system",
      resource_type: "pod",
      resource_name: "etcd-0",
    },
    {
      denied_rule: "namespace_allowlist",
      argument_keys: ["name", "namespace"],
      arguments: { name: "etcd-0", namespace: "kube-system" },
    },
  ),
];

function page(items: AuditEntry[], next_cursor: string | null = null): AuditPage {
  return { items, next_cursor };
}

const activity: McpActivityResponse = {
  window_seconds: 86400,
  step_seconds: 300,
  source: "otlp",
  calls_per_minute: [
    { label: "ok", points: [{ t: 1, v: 9 }, { t: 2, v: 10 }] },
    { label: "error", points: [{ t: 2, v: 1 }] },
  ],
  apply_manifest_per_minute: [{ label: "ok", points: [{ t: 2, v: 0.5 }] }],
  p95_seconds: [{ t: 2, v: 0.25 }],
  tools: [
    { tool: "list_pods", calls: 180, errors: 2, denied: 0, p95_seconds: 0.1 },
    { tool: "apply_manifest", calls: 20, errors: 0, denied: 2, p95_seconds: 0.8 },
  ],
};

function mountPage() {
  return mount(McpActivityPage, { global: { plugins: [vuetify] } });
}

describe("McpActivityPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockList.mockResolvedValue(page(rows));
    mockActivity.mockResolvedValue(activity);
    mockSettings.mockResolvedValue({ tracing: { query_url: "", ui_url: "https://grafana.example.com" } });
  });

  it("lists MCP changes and denials for the window", async () => {
    const w = mountPage();
    await flushPromises();

    expect(mockList).toHaveBeenCalledWith(
      expect.objectContaining({ source: "mcp", limit: 50, since: expect.any(String) }),
    );
    const since = Date.parse(mockList.mock.calls[0][0].since);
    expect(Date.now() - since).toBeGreaterThan(23 * 3600 * 1000);

    const trs = w.findAll("[data-testid='audit-row']");
    expect(trs).toHaveLength(2);
    expect(trs[0].text()).toContain("apply_manifest");
    expect(trs[0].text()).toContain("deployment prod/web");
    expect(trs[1].text()).toContain("denied");
    expect(trs[1].find("[data-testid='denied-rule']").text()).toBe("namespace_allowlist");
    expect(trs[1].text()).toContain("name=etcd-0 namespace=kube-system");
    expect(w.text()).toContain("1 denied");
  });

  it("shows the client as what the client says, never as an identity", async () => {
    const w = mountPage();
    await flushPromises();
    const label = w.findAll("[data-testid='audit-row']")[0].find("[data-testid='client-says']");
    expect(label.text()).toBe("client says claude-code 2.1.0");
    // No session, no client.
    expect(w.findAll("[data-testid='audit-row']")[1].find("[data-testid='client-says']").exists()).toBe(
      false,
    );
  });

  it("links a trace only when the row has one and a tracing UI is set", async () => {
    const w = mountPage();
    await flushPromises();
    const links = w.findAll("[data-testid='view-trace']");
    expect(links).toHaveLength(1);
    const href = links[0].attributes("href") ?? "";
    expect(href.startsWith("https://grafana.example.com/explore?left=")).toBe(true);
    expect(decodeURIComponent(href)).toContain("4bf92f3577b34da6a3ce929d0e0e4736");
    expect(links[0].attributes("rel")).toContain("noopener");
  });

  it("links no trace without a tracing UI", async () => {
    mockSettings.mockResolvedValue({ tracing: null });
    const w = mountPage();
    await flushPromises();
    expect(w.find("[data-testid='view-trace']").exists()).toBe(false);
  });

  it("shows the metrics from Prometheus", async () => {
    const w = mountPage();
    await flushPromises();
    expect(mockActivity).toHaveBeenCalledWith("24h");
    expect(w.find("[data-testid='kpi-calls']").text()).toBe("200");
    expect(w.find("[data-testid='kpi-error-rate']").text()).toBe("2.0%");
    expect(w.find("[data-testid='kpi-denied']").text()).toBe("2");
    expect(w.find("[data-testid='kpi-p95']").text()).toBe("250 ms");
    expect(w.findAll("[data-testid='time-series-card']")).toHaveLength(4);
    const tools = w.find("[data-testid='tool-table']").text();
    expect(tools).toContain("list_pods");
    expect(tools).toContain("800 ms");
    const source = w.find("[data-testid='metrics-source']").text();
    expect(source).toContain("OTLP");
    expect(source).toContain("mcp_server_operation_duration_seconds");
  });

  it("says when the figures come from the /metrics fallback", async () => {
    mockActivity.mockResolvedValue({ ...activity, source: "scrape" });
    const w = mountPage();
    await flushPromises();
    const source = w.find("[data-testid='metrics-source']").text();
    expect(source).toContain("/metrics fallback");
    expect(source).toContain("no OTLP series");
    expect(w.find("[data-testid='kpi-calls']").text()).toBe("200");
  });

  it("says when Prometheus has no tool call series", async () => {
    mockActivity.mockResolvedValue({
      ...activity,
      source: null,
      calls_per_minute: [],
      apply_manifest_per_minute: [],
      p95_seconds: [],
      tools: [],
    });
    const w = mountPage();
    await flushPromises();
    expect(w.find("[data-testid='metrics-source']").text()).toContain("No data");
    expect(w.find("[data-testid='kpi-calls']").text()).toBe("0");
    expect(w.find("[data-testid='tool-table']").text()).toContain("No tool calls in this window.");
  });

  it("still shows the audit log when Prometheus isn't configured", async () => {
    mockActivity.mockResolvedValue({
      ...activity,
      source: null,
      calls_per_minute: [],
      apply_manifest_per_minute: [],
      p95_seconds: [],
      tools: [],
      unavailable_reason: "prometheus not configured (set PROMETHEUS_URL ...)",
    });
    const w = mountPage();
    await flushPromises();
    expect(w.find("[data-testid='metrics-unavailable']").text()).toContain("prometheus not configured");
    expect(w.find("[data-testid='metrics-source']").exists()).toBe(false);
    expect(w.find("[data-testid='kpi-row']").exists()).toBe(false);
    expect(w.findAll("[data-testid='time-series-card']")).toHaveLength(0);
    expect(w.findAll("[data-testid='audit-row']")).toHaveLength(2);
  });

  it("filters the rows and follows the window", async () => {
    const w = mountPage();
    await flushPromises();
    mockList.mockClear();

    const outcome = w.findComponent<typeof components.VSelect>("[data-testid='filter-outcome']");
    await outcome.setValue("denied");
    await flushPromises();
    expect(mockList).toHaveBeenLastCalledWith(expect.objectContaining({ outcome: "denied", source: "mcp" }));

    mockActivity.mockClear();
    const toggle = w.find("[data-testid='window-toggle']");
    await toggle.findAll("button")[0].trigger("click");
    await flushPromises();
    expect(mockActivity).toHaveBeenLastCalledWith("1h");
    const since = Date.parse(mockList.mock.calls.at(-1)![0].since);
    expect(Date.now() - since).toBeLessThan(2 * 3600 * 1000);
  });

  it("loads the next page", async () => {
    mockList.mockResolvedValueOnce(page(rows.slice(0, 1), "cursor-1"));
    const w = mountPage();
    await flushPromises();
    mockList.mockResolvedValueOnce(page(rows.slice(1)));
    await w.find("[data-testid='load-more']").trigger("click");
    await flushPromises();
    expect(mockList).toHaveBeenLastCalledWith(expect.objectContaining({ cursor: "cursor-1" }));
    expect(w.findAll("[data-testid='audit-row']")).toHaveLength(2);
    expect(w.find("[data-testid='load-more']").exists()).toBe(false);
  });

  it("says when nothing happened", async () => {
    mockList.mockResolvedValue(page([]));
    const w = mountPage();
    await flushPromises();
    expect(w.find("[data-testid='no-audit-rows']").exists()).toBe(true);
  });
});
