import { describe, expect, it } from "vitest";
import type { AuditEntry, LabeledSeries } from "@/types/api";
import {
  activityTotals,
  auditTarget,
  clientLabel,
  errorRateSeries,
  formatCount,
  formatPercent,
  formatSeconds,
  outcomeMeta,
  parseMcpDetail,
  sumSeries,
  windowStart,
} from "@/utils/mcpActivity";

function entry(overrides: Partial<AuditEntry> = {}): AuditEntry {
  return {
    id: "1",
    timestamp: "2026-11-24T12:00:00Z",
    action: "write",
    resource_type: "deployment",
    resource_name: "web",
    namespace: "prod",
    detail: "",
    user_identity: "",
    source: "mcp",
    tool: "apply_manifest",
    outcome: "ok",
    client_name: "",
    ...overrides,
  };
}

describe("parseMcpDetail", () => {
  it("reads an MCP row's JSON detail", () => {
    const d = parseMcpDetail(
      entry({
        detail: JSON.stringify({
          tool: "exec_pod",
          outcome: "denied",
          denied_rule: "namespace_allowlist",
          argument_keys: ["command", "namespace"],
          arguments: { namespace: "kube-system" },
          duration_ms: 1,
        }),
      }),
    );
    expect(d?.denied_rule).toBe("namespace_allowlist");
    expect(d?.argument_keys).toEqual(["command", "namespace"]);
  });

  it("ignores API rows, old free-text MCP rows and bad JSON", () => {
    expect(parseMcpDetail(entry({ source: "api", detail: '{"tool":"x"}' }))).toBeNull();
    expect(parseMcpDetail(entry({ detail: "MCP: created application 'web'" }))).toBeNull();
    expect(parseMcpDetail(entry({ detail: "{not json" }))).toBeNull();
    expect(parseMcpDetail(entry({ detail: '{"outcome":"ok"}' }))).toBeNull();
  });

  it("fills in missing lists", () => {
    const d = parseMcpDetail(entry({ detail: '{"tool":"delete_pod","outcome":"ok"}' }));
    expect(d?.argument_keys).toEqual([]);
    expect(d?.arguments).toEqual({});
  });
});

describe("clientLabel", () => {
  it("prefers the detail's name and version", () => {
    const d = parseMcpDetail(
      entry({
        detail: JSON.stringify({
          tool: "t",
          outcome: "ok",
          client: { name: "claude-code", version: "2.1.0", self_reported: true },
        }),
      }),
    );
    expect(clientLabel(entry({ client_name: "claude-code" }), d)).toBe("claude-code 2.1.0");
  });

  it("falls back to the column, and is null without a client", () => {
    expect(clientLabel(entry({ client_name: "cursor" }), null)).toBe("cursor");
    expect(clientLabel(entry(), null)).toBeNull();
  });
});

describe("row helpers", () => {
  it("names the target", () => {
    expect(auditTarget(entry())).toBe("deployment prod/web");
    expect(auditTarget(entry({ namespace: "", resource_type: "namespace", resource_name: "staging" }))).toBe(
      "namespace staging",
    );
  });

  it("gives every outcome an icon and a label", () => {
    for (const outcome of ["ok", "error", "denied", "cancelled", ""]) {
      const m = outcomeMeta(outcome);
      expect(m.icon).toMatch(/^mdi-/);
      expect(m.label).not.toBe("");
    }
    expect(outcomeMeta("denied").label).toBe("denied");
  });

  it("computes the window start", () => {
    const now = Date.parse("2026-11-24T12:00:00Z");
    expect(windowStart("1h", now)).toBe("2026-11-24T11:00:00.000Z");
    expect(windowStart("7d", now)).toBe("2026-11-17T12:00:00.000Z");
  });
});

describe("series helpers", () => {
  const series: LabeledSeries[] = [
    { label: "ok", points: [{ t: 1, v: 8 }, { t: 2, v: 0 }, { t: 3, v: 9 }] },
    { label: "error", points: [{ t: 1, v: 1 }, { t: 3, v: 0.5 }] },
    { label: "denied", points: [{ t: 1, v: 1 }] },
  ];

  it("adds the outcomes up per point", () => {
    expect(sumSeries(series)).toEqual([
      { t: 1, v: 10 },
      { t: 2, v: 0 },
      { t: 3, v: 9.5 },
    ]);
  });

  it("turns them into an error rate, leaving out points without calls", () => {
    const rate = errorRateSeries(series);
    expect(rate.map((p) => p.t)).toEqual([1, 3]);
    expect(rate[0].v).toBeCloseTo(20);
    expect(rate[1].v).toBeCloseTo(5.263, 2);
  });

  it("totals the tools", () => {
    expect(activityTotals([])).toEqual({ calls: 0, errors: 0, denied: 0, errorRate: null });
    const t = activityTotals([
      { tool: "a", calls: 90, errors: 5, denied: 1, p95_seconds: null },
      { tool: "b", calls: 10, errors: 0, denied: 4, p95_seconds: 0.2 },
    ]);
    expect(t).toEqual({ calls: 100, errors: 5, denied: 5, errorRate: 10 });
  });
});

describe("formatting", () => {
  it("formats counts, percentages and durations", () => {
    expect(formatCount(41.97)).toBe("42");
    expect(formatCount(1234)).toBe("1,234");
    expect(formatCount(12_900)).toBe("12.9K");
    expect(formatPercent(null)).toBe("–");
    expect(formatPercent(0)).toBe("0%");
    expect(formatPercent(0.5)).toBe("0.50%");
    expect(formatPercent(12.345)).toBe("12.3%");
    expect(formatSeconds(null)).toBe("–");
    expect(formatSeconds(0.0421)).toBe("42 ms");
    expect(formatSeconds(2.5)).toBe("2.50 s");
    expect(formatSeconds(31)).toBe("31.0 s");
  });
});
