import { describe, expect, it, vi, beforeEach } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";
import { ref } from "vue";
import type { ApplicationDetail, PluginStatus } from "@/types/api";

const { mockGet, mockForApplication } = vi.hoisted(() => ({
  mockGet: vi.fn(),
  mockForApplication: vi.fn(),
}));

vi.mock("vue-router", () => ({ useRouter: () => ({ push: vi.fn() }) }));
vi.mock("@/api/applications", () => ({
  applicationsApi: { get: mockGet, listPlugins: vi.fn().mockResolvedValue([]) },
}));
vi.mock("@/api/pluginStatus", () => ({
  pluginStatusApi: { forApplication: mockForApplication },
}));
vi.mock("@/api/application_resources", () => ({
  applicationResourcesApi: { list: vi.fn().mockResolvedValue([]) },
}));
vi.mock("@/api/plugins", () => ({ pluginsApi: { list: vi.fn().mockResolvedValue([]) } }));
vi.mock("@/composables/useAiSettings", () => ({
  useAiSettings: () => ({ settings: ref({ enabled: false }), loaded: ref(true) }),
}));
vi.mock("@/composables/useResourceMetrics", () => ({
  usePodMetrics: () => ({
    series: ref(new Map()),
    latest: ref(new Map()),
    unavailableReason: ref(null),
  }),
}));

import ApplicationDetailPage from "@/components/pages/ApplicationDetailPage.vue";

const vuetify = createVuetify({ components, directives });

const detail: ApplicationDetail = {
  name: "shop",
  namespace: "shop",
  description: "",
  created_at: null,
  updated_at: null,
  git: null,
  deployments: [],
  cronjobs: [],
  health: "healthy",
};

function failing(): PluginStatus {
  return {
    namespace: "shop",
    application: "shop",
    plugin: "vault",
    resource_id: "",
    health: "failing",
    last_export: "apply",
    last_events: [
      {
        plugin: "vault",
        export: "apply",
        severity: "error",
        code: "plugin.unspecified",
        message: "vault is sealed",
        retryable: false,
      },
    ],
    first_failure_at: "2026-11-24T12:00:00+00:00",
    consecutive_failures: 5,
    last_success_at: null,
    last_changed_at: "2026-11-24T12:00:00+00:00",
    updated_at: "2026-11-24T12:00:50+00:00",
  };
}

function mountPage() {
  return mount(ApplicationDetailPage, {
    props: { namespace: "shop", name: "shop" },
    global: { plugins: [vuetify], stubs: { AddMemberDialog: true, MetricSparkline: true } },
  });
}

describe("ApplicationDetailPage — plugin status", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGet.mockResolvedValue(detail);
  });

  // The acceptance check for the UI: a failing plugin shows on the page.
  it("shows a failing plugin with its latest event and a badge", async () => {
    mockForApplication.mockResolvedValue([failing()]);
    const w = mountPage();
    await flushPromises();

    expect(mockForApplication).toHaveBeenCalledWith("shop", "shop");
    const chip = w.find("[data-testid='plugin-health-chip']");
    expect(chip.attributes("data-health")).toBe("failing");
    expect(chip.text()).toContain("Plugins: failing");
    const alert = w.find("[data-testid='app-plugin-problems']");
    expect(alert.text()).toContain("1 plugin target is failing");
    expect(alert.text()).toContain("vault");
    expect(alert.text()).toContain("shop/shop · apply");
    expect(alert.text()).toContain("5 failed calls in a row");
    expect(alert.text()).toContain("vault is sealed");
  });

  it("shows nothing about plugins when they're healthy or report nothing", async () => {
    mockForApplication.mockResolvedValue([{ ...failing(), health: "healthy", last_events: [] }]);
    let w = mountPage();
    await flushPromises();
    expect(w.find("[data-testid='app-plugin-problems']").exists()).toBe(false);
    expect(w.find("[data-testid='plugin-health-chip']").attributes("data-health")).toBe("healthy");

    mockForApplication.mockResolvedValue([]);
    w = mountPage();
    await flushPromises();
    expect(w.find("[data-testid='plugin-health-chip']").exists()).toBe(false);
  });

  it("keeps working when the status endpoint fails", async () => {
    mockForApplication.mockRejectedValue(new Error("404"));
    const w = mountPage();
    await flushPromises();
    expect(w.find("[data-testid='plugin-health-chip']").exists()).toBe(false);
    expect(w.text()).toContain("shop");
  });
});
