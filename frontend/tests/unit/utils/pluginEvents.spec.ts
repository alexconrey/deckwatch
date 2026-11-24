import { describe, expect, it } from "vitest";
import {
  hasProblems,
  severityColor,
  severityIcon,
  sortEvents,
  summarizeEvents,
  worstSeverity,
} from "@/utils/pluginEvents";
import type { PluginEventReport, PluginEventSeverity } from "@/types/api";

const ev = (severity: PluginEventSeverity, message: string = severity): PluginEventReport => ({
  plugin: "aws",
  export: "apply",
  severity,
  code: "aws.x",
  message,
  retryable: false,
});

describe("plugin event helpers", () => {
  it("summarizes counts per severity", () => {
    expect(summarizeEvents([])).toBe("");
    expect(summarizeEvents(undefined)).toBe("");
    expect(summarizeEvents([ev("error")])).toBe("1 error");
    expect(summarizeEvents([ev("warning"), ev("warning"), ev("info"), ev("error")])).toBe(
      "1 error, 2 warnings, 1 notice",
    );
  });

  it("treats only warnings and errors as problems", () => {
    expect(hasProblems(undefined)).toBe(false);
    expect(hasProblems([ev("info")])).toBe(false);
    expect(hasProblems([ev("info"), ev("warning")])).toBe(true);
  });

  it("finds the worst severity and sorts stably", () => {
    expect(worstSeverity([])).toBeNull();
    expect(worstSeverity([ev("info"), ev("warning")])).toBe("warning");
    expect(
      sortEvents([ev("info", "i"), ev("warning", "w1"), ev("error", "e"), ev("warning", "w2")]).map(
        (e) => e.message,
      ),
    ).toEqual(["e", "w1", "w2", "i"]);
  });

  it("maps severities to icons and colors", () => {
    expect(severityIcon("error")).toBe("mdi-alert-circle");
    expect(severityIcon("warning")).toBe("mdi-alert");
    expect(severityIcon("info")).toBe("mdi-information");
    expect(severityColor("warning")).toBe("warning");
    expect(severityColor("info")).toBe("info");
  });
});
