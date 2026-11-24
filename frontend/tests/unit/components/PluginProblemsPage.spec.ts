import { describe, expect, it, vi, beforeEach } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";
import type { PluginStatus } from "@/types/api";

const { mockProblems, mockPush } = vi.hoisted(() => ({
  mockProblems: vi.fn(),
  mockPush: vi.fn(),
}));

vi.mock("vue-router", () => ({ useRouter: () => ({ push: mockPush }) }));
vi.mock("@/api/pluginStatus", () => ({ pluginStatusApi: { problems: mockProblems } }));

import PluginProblemsPage from "@/components/pages/PluginProblemsPage.vue";
import PluginHealthChip from "@/components/common/PluginHealthChip.vue";

const vuetify = createVuetify({ components, directives });

function status(overrides: Partial<PluginStatus>): PluginStatus {
  return {
    namespace: "shop",
    application: "web",
    plugin: "aws",
    resource_id: "rds",
    health: "failing",
    last_export: "provision",
    last_events: [
      {
        plugin: "aws",
        export: "provision",
        severity: "error",
        code: "plugin.trap",
        message: "provision() trapped: wasm `unreachable` instruction executed",
        retryable: false,
      },
    ],
    first_failure_at: new Date(Date.now() - 120_000).toISOString(),
    consecutive_failures: 3,
    last_success_at: null,
    last_changed_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    ...overrides,
  };
}

function mountPage() {
  return mount(PluginProblemsPage, { global: { plugins: [vuetify] } });
}

describe("PluginProblemsPage", () => {
  beforeEach(() => vi.clearAllMocks());

  it("lists failing plugin targets with their latest events", async () => {
    mockProblems.mockResolvedValue([
      status({}),
      status({ health: "degraded", resource_id: "", last_export: "apply", consecutive_failures: 0 }),
    ]);
    const w = mountPage();
    await flushPromises();

    const cards = w.findAll("[data-testid='plugin-problem']");
    expect(cards).toHaveLength(2);
    expect(cards[0].text()).toContain("shop/web · rds");
    expect(cards[0].text()).toContain("3 failed provision() calls in a row");
    expect(cards[0].text()).toContain("plugin.trap");
    expect(cards[0].text()).toContain("no success recorded");
    expect(cards[1].text()).toContain("shop/web · apply");
    expect(w.text()).toContain("1 failing");
    expect(w.text()).toContain("1 degraded");
  });

  it("opens the application, or the plugin for plugin-wide problems", async () => {
    mockProblems.mockResolvedValue([
      status({}),
      status({ namespace: "", application: "", resource_id: "", last_export: "fetch" }),
    ]);
    const w = mountPage();
    await flushPromises();
    const buttons = w.findAll("[data-testid='plugin-problem'] button");
    await buttons[0].trigger("click");
    expect(mockPush).toHaveBeenCalledWith({
      name: "ApplicationDetail",
      params: { namespace: "shop", name: "web" },
    });
    await buttons[1].trigger("click");
    expect(mockPush).toHaveBeenCalledWith({ name: "plugin-settings", params: { name: "aws" } });
    expect(w.text()).toContain("aws (load)");
  });

  it("says so when nothing is wrong", async () => {
    mockProblems.mockResolvedValue([]);
    const w = mountPage();
    await flushPromises();
    expect(w.find("[data-testid='no-plugin-problems']").exists()).toBe(true);
  });

  it("shows an error when the status can't be loaded", async () => {
    mockProblems.mockRejectedValue(new Error("boom"));
    const w = mountPage();
    await flushPromises();
    expect(w.text()).toContain("boom");
    expect(w.find("[data-testid='no-plugin-problems']").exists()).toBe(false);
  });
});

describe("PluginHealthChip", () => {
  it.each([
    ["failing", "error", "failing"],
    ["degraded", "warning", "degraded"],
    ["healthy", "success", "healthy"],
  ] as const)("renders %s", (health, color, text) => {
    const w = mount(PluginHealthChip, {
      props: { health, label: "Plugins" },
      global: { plugins: [vuetify] },
    });
    expect(w.text()).toContain(`Plugins: ${text}`);
    expect(w.attributes("data-health")).toBe(health);
    expect(w.classes().join(" ")).toContain(`text-${color}`);
  });
});
