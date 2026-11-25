import { beforeEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";
import type { ApplicationObservability } from "@/types/api";

const { mockGet, mockUpdate, mockUpsert, mockDisable } = vi.hoisted(() => ({
  mockGet: vi.fn(),
  mockUpdate: vi.fn(),
  mockUpsert: vi.fn(),
  mockDisable: vi.fn(),
}));

vi.mock("@/api/applications", () => ({
  applicationsApi: { getObservability: mockGet, updateObservability: mockUpdate },
}));
vi.mock("@/api/monitoring", () => ({
  monitoringApi: { upsert: mockUpsert, disable: mockDisable },
}));

import ObservabilityTab from "@/components/views/application/ObservabilityTab.vue";

const vuetify = createVuetify({ components, directives });
const COLLECTOR = "http://otel-collector.observability:4318";

function observability(enabled = false): ApplicationObservability {
  return {
    prometheus: {
      available: true,
      unavailable_reason: null,
      deployments: [
        { deployment: "web", enabled: true, name: "web-deckwatch", port: "metrics", path: "/metrics", interval: "30s" },
        { deployment: "api", enabled: false, name: "api-deckwatch", port: "http", path: "/metrics", interval: "30s" },
      ],
    },
    opentelemetry: {
      enabled,
      service_name: null,
      resource_attributes: [],
      otlp_endpoint: COLLECTOR,
      otlp_metrics_endpoint: null,
      signals: [
        { signal: "traces", endpoint: `${COLLECTOR}/v1/traces` },
        { signal: "metrics", endpoint: null },
        { signal: "logs", endpoint: `${COLLECTOR}/v1/logs` },
      ],
      workloads: [
        { kind: "Deployment", name: "web", container: "app", state: "off", injected: [], kept: [], exports_itself: false },
        { kind: "CronJob", name: "nightly", container: "job", state: "off", injected: [], kept: [], exports_itself: false },
      ],
      workloads_error: null,
    },
  };
}

function mountTab() {
  return mount(ObservabilityTab, {
    props: { namespace: "default", name: "storefront" },
    global: { plugins: [vuetify], stubs: { RouterLink: true } },
    attachTo: document.body,
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  document.body.innerHTML = "";
  // Vuetify's VOverlay reads visualViewport, which happy-dom doesn't provide.
  vi.stubGlobal("visualViewport", {
    addEventListener() {},
    removeEventListener() {},
    width: 1024,
    height: 768,
  });
});

describe("ObservabilityTab", () => {
  it("shows the PodMonitors and where each signal goes", async () => {
    mockGet.mockResolvedValue(observability());
    const wrapper = mountTab();
    await flushPromises();
    expect(mockGet).toHaveBeenCalledWith("default", "storefront");
    expect(wrapper.get('[data-testid="monitor-web"]').text()).toContain("On");
    expect(wrapper.get('[data-testid="monitor-api"]').text()).toContain("Enable");
    const signals = wrapper.get('[data-testid="app-otel-signals"]').text();
    expect(signals).toContain(`${COLLECTOR}/v1/traces`);
    expect(signals).toContain("OTEL_METRICS_EXPORTER=none");
  });

  it("asks before turning injection on, then saves the settings", async () => {
    mockGet.mockResolvedValue(observability());
    mockUpdate.mockResolvedValue(observability(true));
    const wrapper = mountTab();
    await flushPromises();

    await wrapper.get('[data-testid="app-otel-enabled"] input').setValue(true);
    await wrapper.get('[data-testid="app-otel-attributes"] input').setValue("team=payments");
    const save = wrapper.findAll("button").find((b) => b.text() === "Save")!;
    await save.trigger("click");
    await flushPromises();
    expect(mockUpdate).not.toHaveBeenCalled();
    expect(document.body.textContent).toContain("Turn on OpenTelemetry injection?");
    expect(document.body.textContent).toContain("1 Deployment and 1 CronJob");

    const turnOn = [...document.querySelectorAll("button")].find((b) => b.textContent?.trim() === "Turn on")!;
    turnOn.click();
    await flushPromises();
    expect(mockUpdate).toHaveBeenCalledWith("default", "storefront", {
      opentelemetry: {
        enabled: true,
        service_name: null,
        resource_attributes: [{ key: "team", value: "payments" }],
      },
    });
  });

  it("won't save attributes the server would refuse", async () => {
    mockGet.mockResolvedValue(observability(true));
    const wrapper = mountTab();
    await flushPromises();
    await wrapper.get('[data-testid="app-otel-attributes"] input').setValue("k8s.namespace.name=x");
    expect(wrapper.text()).toContain("k8s.namespace.name is set by deckwatch.");
    const save = wrapper.findAll("button").find((b) => b.text() === "Save")!;
    expect(save.attributes("disabled")).toBeDefined();
  });

  it("turns a deployment's PodMonitor on with the dialog", async () => {
    mockGet.mockResolvedValue(observability());
    mockUpsert.mockResolvedValue({});
    const wrapper = mountTab();
    await flushPromises();
    await wrapper.get('[data-testid="monitor-api"] button').trigger("click");
    await flushPromises();
    expect(document.body.textContent).toContain("Enable Monitor");
    const enable = [...document.querySelectorAll(".v-dialog button")].find(
      (b) => b.textContent?.trim() === "Enable",
    ) as HTMLButtonElement;
    enable.click();
    await flushPromises();
    expect(mockUpsert).toHaveBeenCalledWith("default", "api", {
      enabled: true,
      port: "http",
      path: "/metrics",
      interval: "30s",
    });
  });
});
