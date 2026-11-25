import { afterEach, describe, expect, it } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";
import OtlpExportSection from "@/components/settings/OtlpExportSection.vue";
import type { ObservabilitySettings, ObservabilityStatus, OtlpSignalExport } from "@/types/api";
import { RETIRED_NOTICE_KEY, emptyObservability } from "@/utils/observability";
import { mockFetchOnce } from "../helpers/mockFetch";

const vuetify = createVuetify({ components, directives });

const COLLECTOR = "http://otel-collector.observability:4318";

const UNSET: OtlpSignalExport = { source: "unset", endpoint: "", from: null, exported: false, reason: null };

function fromSettings(signal: "traces" | "logs", base = COLLECTOR): OtlpSignalExport {
  return {
    source: "settings",
    endpoint: `${base}/v1/${signal}`,
    from: "observability.otlp_endpoint",
    exported: true,
    reason: null,
  };
}

function status(overrides: Partial<ObservabilityStatus> = {}): ObservabilityStatus {
  return {
    signals: [
      { signal: "traces", running: fromSettings("traces"), configured: fromSettings("traces") },
      { signal: "metrics", running: UNSET, configured: UNSET },
      { signal: "logs", running: fromSettings("logs"), configured: fromSettings("logs") },
    ],
    restart_required: false,
    retired_otlp_endpoint: null,
    ...overrides,
  };
}

function mountSection(initial: ObservabilitySettings = emptyObservability()) {
  const wrapper = mount(OtlpExportSection, {
    props: {
      modelValue: initial,
      "onUpdate:modelValue": (value: ObservabilitySettings) => wrapper.setProps({ modelValue: value }),
    },
    global: { plugins: [vuetify] },
  });
  return wrapper;
}

afterEach(() => localStorage.clear());

describe("OtlpExportSection", () => {
  it("loads the export status and shows where each signal goes", async () => {
    const fetchMock = mockFetchOnce({ body: status() });
    const wrapper = mountSection({ otlp_endpoint: COLLECTOR, otlp_metrics_endpoint: "" });
    await flushPromises();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0][0]).toBe("/api/settings/observability");
    expect(wrapper.find('[data-testid="otlp-endpoint"] input').element).toHaveProperty("value", COLLECTOR);

    const traces = wrapper.find('[data-testid="otlp-signal-traces"]').text();
    expect(traces).toContain("Settings");
    expect(traces).toContain(`${COLLECTOR}/v1/traces`);
    expect(wrapper.find('[data-testid="otlp-signal-metrics"]').text()).toContain("Not exported");
    expect(wrapper.find('[data-testid="otlp-restart-note"]').text()).toContain("restart");
    // The same endpoints reach applications with injection on, never the
    // environment's.
    expect(wrapper.find('[data-testid="otlp-restart-note"]').text()).toContain(
      "deckwatch's environment variables never apply to them",
    );
    expect(wrapper.find('[data-testid="otlp-restart-required"]').exists()).toBe(false);
  });

  it("says what changes after a restart", async () => {
    const moved = fromSettings("traces", "https://otel-collector.example.com");
    mockFetchOnce({
      body: status({
        signals: [
          { signal: "traces", running: fromSettings("traces"), configured: moved },
          { signal: "metrics", running: UNSET, configured: UNSET },
          { signal: "logs", running: fromSettings("logs"), configured: fromSettings("logs") },
        ],
        restart_required: true,
      }),
    });
    const wrapper = mountSection();
    await flushPromises();

    expect(wrapper.find('[data-testid="otlp-restart-required"]').text()).toContain("Restart deckwatch");
    expect(wrapper.find('[data-testid="otlp-after-restart-traces"]').text()).toBe(
      "After a restart: https://otel-collector.example.com/v1/traces (settings)",
    );
    expect(wrapper.find('[data-testid="otlp-after-restart-logs"]').exists()).toBe(false);
  });

  it("shows the environment variable that wins over a field", async () => {
    const fromEnv: OtlpSignalExport = {
      source: "env",
      endpoint: "http://env-collector:4318/v1/traces",
      from: "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
      exported: true,
      reason: null,
    };
    mockFetchOnce({
      body: status({
        signals: [
          { signal: "traces", running: fromEnv, configured: fromEnv },
          { signal: "metrics", running: UNSET, configured: UNSET },
          { signal: "logs", running: fromSettings("logs"), configured: fromSettings("logs") },
        ],
      }),
    });
    const wrapper = mountSection();
    await flushPromises();

    expect(wrapper.find('[data-testid="otlp-signal-traces"]').text()).toContain("Environment");
    expect(wrapper.find('[data-testid="otlp-signal-traces"]').text()).toContain(
      "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
    );
    expect(wrapper.text()).toContain("Traces use OTEL_EXPORTER_OTLP_TRACES_ENDPOINT instead");
  });

  it("asks for the OTLP/HTTP endpoint while the retired one is set, until dismissed", async () => {
    mockFetchOnce({
      body: status({
        retired_otlp_endpoint: { value: "deckwatch-tempo.deckwatch.svc:4317", message: "retired" },
      }),
    });
    const wrapper = mountSection();
    await flushPromises();

    const notice = wrapper.find('[data-testid="otlp-retired-notice"]');
    expect(notice.text()).toContain("deckwatch-tempo.deckwatch.svc:4317");
    expect(notice.text()).toContain("OTLP/HTTP");
    await notice.find("button").trigger("click");
    expect(wrapper.find('[data-testid="otlp-retired-notice"]').exists()).toBe(false);
    expect(localStorage.getItem(RETIRED_NOTICE_KEY)).toBe("deckwatch-tempo.deckwatch.svc:4317");

    // Dismissed in this browser: it stays hidden.
    mockFetchOnce({
      body: status({
        retired_otlp_endpoint: { value: "deckwatch-tempo.deckwatch.svc:4317", message: "retired" },
      }),
    });
    const again = mountSection();
    await flushPromises();
    expect(again.find('[data-testid="otlp-retired-notice"]').exists()).toBe(false);
  });

  it("validates while typing and updates the model", async () => {
    mockFetchOnce({ body: status() });
    const wrapper = mountSection();
    await flushPromises();

    const input = wrapper.find('[data-testid="otlp-endpoint"] input');
    await input.setValue("https://user:secret@otlp.example.com");
    await flushPromises();
    expect(wrapper.text()).toContain("Must not contain a username or password.");
    expect(wrapper.props("modelValue").otlp_endpoint).toBe("https://user:secret@otlp.example.com");

    await input.setValue(COLLECTOR);
    await flushPromises();
    expect(wrapper.text()).not.toContain("Must not contain a username or password.");
    expect(wrapper.props("modelValue")).toEqual({ otlp_endpoint: COLLECTOR, otlp_metrics_endpoint: "" });
  });

  it("reports a status that can't be loaded", async () => {
    mockFetchOnce({ status: 500, body: { error: "internal", message: "boom" } });
    const wrapper = mountSection();
    await flushPromises();
    expect(wrapper.find('[data-testid="otlp-status-error"]').text()).toBe("boom");
    expect(wrapper.find('[data-testid="otlp-signals"]').exists()).toBe(false);
  });
});
