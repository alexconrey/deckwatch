import { describe, expect, it } from "vitest";
import type { ObservabilityStatus, OtlpSignalExport, OtlpSignalStatus } from "@/types/api";
import {
  changesOnRestart,
  current,
  dismissRetiredNotice,
  emptyObservability,
  exportSummary,
  overrideHint,
  retiredNoticeDismissed,
  RETIRED_NOTICE_KEY,
  sourceLabel,
  validateObservability,
  validateOtlpEndpoint,
} from "@/utils/observability";

const COLLECTOR = "http://otel-collector.observability:4318";
const PROMETHEUS = "http://prometheus.monitoring:9090/api/v1/otlp/v1/metrics";

function exported(overrides: Partial<OtlpSignalExport> = {}): OtlpSignalExport {
  return {
    source: "settings",
    endpoint: `${COLLECTOR}/v1/traces`,
    from: "observability.otlp_endpoint",
    exported: true,
    reason: null,
    ...overrides,
  };
}

const UNSET: OtlpSignalExport = { source: "unset", endpoint: "", from: null, exported: false, reason: null };

describe("validateOtlpEndpoint", () => {
  it("accepts empty and http(s) URLs, plain http for any host", () => {
    for (const value of ["", "  ", COLLECTOR, `${COLLECTOR}/`, "https://otlp.example.com/otlp", "http://[::1]:4318"]) {
      expect(validateOtlpEndpoint("otlp_endpoint", value), value).toBeNull();
    }
    expect(validateOtlpEndpoint("otlp_metrics_endpoint", PROMETHEUS)).toBeNull();
    expect(validateOtlpEndpoint("otlp_metrics_endpoint", "https://collector.example.com/v1/metrics")).toBeNull();
  });

  it("refuses the retired gRPC host:port format without converting it", () => {
    expect(validateOtlpEndpoint("otlp_endpoint", "deckwatch-tempo.deckwatch.svc:4317")).toContain(
      "http:// or https:// URL",
    );
    expect(validateOtlpEndpoint("otlp_endpoint", "grpc://collector:4317")).toContain("Use http:// or https://");
  });

  it("refuses credentials, query strings and fragments", () => {
    expect(validateOtlpEndpoint("otlp_endpoint", "https://user:secret@otlp.example.com")).toContain(
      "username or password",
    );
    expect(validateOtlpEndpoint("otlp_endpoint", "https://token@otlp.example.com")).toContain(
      "username or password",
    );
    expect(validateOtlpEndpoint("otlp_endpoint", "https://otlp.example.com/?api-key=x")).toContain("query string");
    expect(validateOtlpEndpoint("otlp_endpoint", "https://otlp.example.com/#x")).toContain("query string");
  });

  it("wants a base URL for traces and logs, and a full URL for metrics", () => {
    expect(validateOtlpEndpoint("otlp_endpoint", `${COLLECTOR}/v1/traces`)).toContain("without /v1/traces");
    expect(validateOtlpEndpoint("otlp_endpoint", `${COLLECTOR}/v1/logs/`)).toContain("without /v1/logs");
    expect(validateOtlpEndpoint("otlp_metrics_endpoint", "http://prometheus.monitoring:9090")).toContain(
      "full URL of the receiver",
    );
    expect(validateOtlpEndpoint("otlp_metrics_endpoint", "http://prometheus.monitoring:9090/")).toContain(
      "full URL of the receiver",
    );
  });

  it("refuses overlong values", () => {
    expect(validateOtlpEndpoint("otlp_endpoint", `${COLLECTOR}/${"a".repeat(2048)}`)).toContain("Longer than 2048");
  });
});

describe("validateObservability", () => {
  it("names the first invalid field", () => {
    expect(validateObservability(emptyObservability())).toBeNull();
    expect(validateObservability({ otlp_endpoint: COLLECTOR, otlp_metrics_endpoint: PROMETHEUS })).toBeNull();
    expect(
      validateObservability({ otlp_endpoint: COLLECTOR, otlp_metrics_endpoint: "http://prometheus:9090" }),
    ).toMatch(/^OTLP metrics endpoint: /);
  });
});

describe("signal status helpers", () => {
  it("labels sources and summarizes where a signal goes", () => {
    expect(sourceLabel("env")).toBe("Environment");
    expect(sourceLabel("settings")).toBe("Settings");
    expect(sourceLabel("unset")).toBe("Not set");
    expect(exportSummary(exported())).toBe(`${COLLECTOR}/v1/traces`);
    expect(exportSummary(UNSET)).toBe("Not exported");
    expect(exportSummary(exported({ exported: false, reason: "OTEL_TRACES_EXPORTER=none" }))).toBe(
      "Not exported: OTEL_TRACES_EXPORTER=none",
    );
  });

  it("detects a signal that goes elsewhere after a restart", () => {
    const same: OtlpSignalStatus = { signal: "traces", running: exported(), configured: exported() };
    expect(changesOnRestart(same)).toBe(false);
    expect(current(same)).toBe(same.running);

    const moved: OtlpSignalStatus = {
      signal: "traces",
      running: exported(),
      configured: exported({ endpoint: "https://otel-collector.example.com/v1/traces" }),
    };
    expect(changesOnRestart(moved)).toBe(true);
    expect(changesOnRestart({ signal: "traces", running: exported(), configured: UNSET })).toBe(true);

    // Before startup has read the settings, there is nothing to compare.
    const notStarted: OtlpSignalStatus = { signal: "logs", running: null, configured: exported() };
    expect(changesOnRestart(notStarted)).toBe(false);
    expect(current(notStarted)).toBe(notStarted.configured);
  });

  it("says when an environment variable wins over a field", () => {
    const status: ObservabilityStatus = {
      signals: [
        {
          signal: "traces",
          running: null,
          configured: exported({ source: "env", from: "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT" }),
        },
        { signal: "metrics", running: null, configured: UNSET },
        { signal: "logs", running: null, configured: exported({ endpoint: `${COLLECTOR}/v1/logs` }) },
      ],
      restart_required: false,
      retired_otlp_endpoint: null,
    };
    expect(overrideHint(status, "otlp_endpoint")).toBe(
      "Traces use OTEL_EXPORTER_OTLP_TRACES_ENDPOINT instead: an environment variable wins over this setting.",
    );
    expect(overrideHint(status, "otlp_metrics_endpoint")).toBeNull();
    expect(overrideHint(null, "otlp_endpoint")).toBeNull();

    status.signals[2].configured = exported({ source: "env", from: "OTEL_EXPORTER_OTLP_ENDPOINT" });
    status.signals[0].configured.from = "OTEL_EXPORTER_OTLP_ENDPOINT";
    expect(overrideHint(status, "otlp_endpoint")).toBe(
      "Traces and logs use OTEL_EXPORTER_OTLP_ENDPOINT instead: an environment variable wins over this setting.",
    );
  });
});

describe("retired endpoint notice", () => {
  it("is dismissed per value", () => {
    const stored = new Map<string, string>();
    const store = {
      getItem: (key: string) => stored.get(key) ?? null,
      setItem: (key: string, value: string) => void stored.set(key, value),
    };
    expect(retiredNoticeDismissed("tempo:4317", store)).toBe(false);
    dismissRetiredNotice("tempo:4317", store);
    expect(stored.get(RETIRED_NOTICE_KEY)).toBe("tempo:4317");
    expect(retiredNoticeDismissed("tempo:4317", store)).toBe(true);
    // A different old value shows the notice again.
    expect(retiredNoticeDismissed("jaeger:4317", store)).toBe(false);
  });
});
