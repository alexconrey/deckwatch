import { describe, expect, it } from "vitest";
import {
  MANAGED_ATTRIBUTES,
  confirmCopy,
  describeWorkloads,
  formatResourceAttributes,
  injectionChange,
  injectionStateColor,
  injectionStateLabel,
  isAttributeKey,
  isEncodedAttributeValue,
  parseResourceAttributes,
  validateServiceName,
} from "@/utils/appObservability";
import type { OtelInjectionSettings, WorkloadInjection } from "@/types/api";

const off: OtelInjectionSettings = { enabled: false, service_name: null, resource_attributes: [] };
const on: OtelInjectionSettings = { ...off, enabled: true };

describe("validateServiceName", () => {
  it("accepts empty (each workload's name) and up to 255 characters", () => {
    expect(validateServiceName("")).toBeNull();
    expect(validateServiceName("  checkout  ")).toBeNull();
    expect(validateServiceName("é".repeat(255))).toBeNull();
  });

  it("rejects longer names and control characters", () => {
    expect(validateServiceName("x".repeat(256))).toMatch(/255/);
    expect(validateServiceName("a\nb")).toMatch(/control/);
    expect(validateServiceName("a\u0085b")).toMatch(/control/);
  });
});

describe("resource attributes", () => {
  it("checks keys and percent-encoded values like the server", () => {
    expect(isAttributeKey("deployment.environment")).toBe(true);
    expect(isAttributeKey("a-b_c.d")).toBe(true);
    expect(isAttributeKey("bad key")).toBe(false);
    expect(isAttributeKey("")).toBe(false);
    expect(isEncodedAttributeValue("payments%20eu")).toBe(true);
    expect(isEncodedAttributeValue("a=b")).toBe(true);
    for (const bad of ["", "two words", '"q"', "a;b", "a\\b", "é", "50%", "%zz"]) {
      expect(isEncodedAttributeValue(bad), bad).toBe(false);
    }
  });

  it("parses key=value pairs, trimming and skipping empty ones", () => {
    expect(parseResourceAttributes("")).toEqual({ attributes: [], error: null });
    expect(parseResourceAttributes(" team = payments ,, tier=web, ")).toEqual({
      attributes: [
        { key: "team", value: "payments" },
        { key: "tier", value: "web" },
      ],
      error: null,
    });
    // Only the first = separates the key.
    expect(parseResourceAttributes("q=a=b").attributes).toEqual([{ key: "q", value: "a=b" }]);
  });

  it("reports the first problem", () => {
    expect(parseResourceAttributes("novalue").error).toMatch(/key=value/);
    expect(parseResourceAttributes("bad key=1").error).toMatch(/letters, digits/);
    expect(parseResourceAttributes("team=two words").error).toMatch(/Percent-encode/);
    expect(parseResourceAttributes("a=1, a=2").error).toMatch(/repeated/);
    expect(parseResourceAttributes("service.name=x").error).toMatch(/service name/);
    for (const key of MANAGED_ATTRIBUTES) {
      expect(parseResourceAttributes(`${key}=x`).error).toMatch(/set by deckwatch/);
    }
    const many = Array.from({ length: 33 }, (_, i) => `k${i}=v`).join(",");
    expect(parseResourceAttributes(many).error).toMatch(/32/);
  });

  it("formats them back as the field shows them", () => {
    const text = "deployment.environment=prod, team=payments%20eu";
    expect(formatResourceAttributes(parseResourceAttributes(text).attributes)).toBe(text);
  });
});

describe("injectionChange", () => {
  it("tells turning on and off from changing the settings", () => {
    expect(injectionChange(off, on)).toBe("enable");
    expect(injectionChange(on, off)).toBe("disable");
    expect(injectionChange(on, { ...on })).toBeNull();
    expect(injectionChange(on, { ...on, service_name: "shop" })).toBe("update");
    expect(
      injectionChange(on, { ...on, resource_attributes: [{ key: "team", value: "x" }] }),
    ).toBe("update");
  });
});

describe("confirmCopy", () => {
  const workloads: { kind: WorkloadInjection["kind"] }[] = [
    { kind: "Deployment" },
    { kind: "Deployment" },
    { kind: "StatefulSet" },
    { kind: "CronJob" },
  ];

  it("names the workloads and says their pods restart once", () => {
    expect(describeWorkloads(workloads)).toBe("2 Deployments, 1 StatefulSet and 1 CronJob");
    expect(describeWorkloads([{ kind: "CronJob" }])).toBe("1 CronJob");
    expect(describeWorkloads([])).toBe("");
    const copy = confirmCopy("enable", true, workloads, true)!;
    expect(copy.title).toBe("Turn on OpenTelemetry injection?");
    expect(copy.message).toContain("2 Deployments, 1 StatefulSet and 1 CronJob");
    expect(copy.message).toContain("restart once");
    expect(copy.confirmText).toBe("Turn on");
  });

  it("says when nothing is injected yet", () => {
    expect(confirmCopy("enable", true, workloads, false)!.message).toContain(
      "No OTLP endpoint is set",
    );
    expect(confirmCopy("enable", true, [], true)!.message).toContain("no Deployments");
  });

  it("asks before turning off, and before changes only while injecting", () => {
    expect(confirmCopy("disable", false, workloads, true)!.message).toContain(
      "values your workloads set themselves stay",
    );
    expect(confirmCopy("update", true, workloads, true)!.confirmText).toBe("Save");
    expect(confirmCopy("update", false, workloads, true)).toBeNull();
  });
});

describe("injection state", () => {
  const workload = (state: WorkloadInjection["state"]): WorkloadInjection => ({
    kind: "Deployment",
    name: "web",
    container: "app",
    state,
    injected: ["OTEL_SERVICE_NAME", "OTEL_EXPORTER_OTLP_ENDPOINT"],
    kept: [],
    exports_itself: false,
  });

  it("labels and colours each state", () => {
    expect(injectionStateLabel(workload("injected"), true)).toBe("Injected (2)");
    expect(injectionStateLabel(workload("pending"), true)).toBe("Applying");
    expect(injectionStateLabel(workload("pending"), false)).toBe("Removing");
    expect(injectionStateLabel(workload("off"), false)).toBe("Off");
    expect(injectionStateLabel(workload("foreign"), true)).toBe("Another deckwatch");
    expect(injectionStateColor("foreign")).toBe("warning");
    expect(injectionStateColor("injected")).toBe("success");
    expect(injectionStateColor("pending")).toBe("info");
    expect(injectionStateColor("off")).toBeUndefined();
  });
});
