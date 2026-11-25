// Helpers for the MCP activity page: reading MCP audit rows and turning the
// activity metrics into the series and figures the page shows.

import type {
  AuditEntry,
  LabeledSeries,
  McpActivityWindow,
  McpAuditDetail,
  TimePoint,
  ToolActivity,
} from "@/types/api";

export const WINDOWS: { value: McpActivityWindow; title: string; seconds: number }[] = [
  { value: "1h", title: "Last hour", seconds: 3600 },
  { value: "24h", title: "Last 24 hours", seconds: 24 * 3600 },
  { value: "7d", title: "Last 7 days", seconds: 7 * 24 * 3600 },
];

export function windowSeconds(window: McpActivityWindow): number {
  return WINDOWS.find((w) => w.value === window)?.seconds ?? 24 * 3600;
}

/** The start of `window` ending at `nowMs`, as RFC 3339 (the audit API's `since`). */
export function windowStart(window: McpActivityWindow, nowMs: number = Date.now()): string {
  return new Date(nowMs - windowSeconds(window) * 1000).toISOString();
}

/**
 * The JSON detail of an MCP audit row, or null when it isn't one: API rows,
 * and MCP rows written before SEC-22, have free-text detail.
 */
export function parseMcpDetail(entry: AuditEntry): McpAuditDetail | null {
  if (entry.source !== "mcp" || !entry.detail.startsWith("{")) return null;
  try {
    const parsed = JSON.parse(entry.detail) as Partial<McpAuditDetail>;
    if (typeof parsed.tool !== "string") return null;
    return {
      argument_keys: [],
      arguments: {},
      duration_ms: 0,
      ...parsed,
    } as McpAuditDetail;
  } catch {
    return null;
  }
}

/**
 * What the client said about itself in `initialize` ("claude-code 2.1.0"),
 * or null when the call wasn't in a session. Self-reported: show it as such,
 * never as who made the call.
 */
export function clientLabel(entry: AuditEntry, detail: McpAuditDetail | null): string | null {
  const name = detail?.client?.name ?? (entry.client_name || undefined);
  const version = detail?.client?.version;
  if (!name && !version) return null;
  return [name ?? "unnamed client", version].filter(Boolean).join(" ");
}

/** The target of an audit row: "deployment prod/web", "namespace staging". */
export function auditTarget(entry: AuditEntry): string {
  const name = [entry.namespace, entry.resource_name].filter(Boolean).join("/");
  return [entry.resource_type, name].filter(Boolean).join(" ");
}

export interface OutcomeMeta {
  label: string;
  icon: string;
  color: string;
}

// Status colors always come with an icon and a label, never color alone.
const OUTCOMES: Record<string, OutcomeMeta> = {
  ok: { label: "ok", icon: "mdi-check-circle", color: "success" },
  error: { label: "error", icon: "mdi-alert-circle", color: "error" },
  denied: { label: "denied", icon: "mdi-cancel", color: "warning" },
  cancelled: { label: "cancelled", icon: "mdi-close-circle-outline", color: "secondary" },
};

export function outcomeMeta(outcome: string): OutcomeMeta {
  return OUTCOMES[outcome] ?? { label: outcome || "unknown", icon: "mdi-help-circle", color: "secondary" };
}

export const ACTIONS = ["write", "delete", "exec", "read"] as const;
export const OUTCOME_VALUES = ["ok", "error", "denied", "cancelled"] as const;

/** Point-wise sum of several series (the outcomes of one metric). */
export function sumSeries(series: LabeledSeries[]): TimePoint[] {
  const byTime = new Map<number, number>();
  for (const s of series) {
    for (const p of s.points) byTime.set(p.t, (byTime.get(p.t) ?? 0) + p.v);
  }
  return [...byTime.entries()].sort(([a], [b]) => a - b).map(([t, v]) => ({ t, v }));
}

/**
 * The share of calls that failed or were denied, in percent, per point.
 * Points without calls are left out rather than drawn as 0%.
 */
export function errorRateSeries(series: LabeledSeries[]): TimePoint[] {
  const total = sumSeries(series);
  const failed = sumSeries(series.filter((s) => s.label === "error" || s.label === "denied"));
  const failedAt = new Map(failed.map((p) => [p.t, p.v]));
  return total
    .filter((p) => p.v > 0)
    .map((p) => ({ t: p.t, v: (100 * (failedAt.get(p.t) ?? 0)) / p.v }));
}

export interface ActivityTotals {
  calls: number;
  errors: number;
  denied: number;
  // Percent of calls that failed or were denied; null without calls.
  errorRate: number | null;
}

/** Totals over the window, from the per-tool figures. */
export function activityTotals(tools: ToolActivity[]): ActivityTotals {
  const calls = tools.reduce((n, t) => n + t.calls, 0);
  const errors = tools.reduce((n, t) => n + t.errors, 0);
  const denied = tools.reduce((n, t) => n + t.denied, 0);
  return {
    calls,
    errors,
    denied,
    errorRate: calls > 0 ? (100 * (errors + denied)) / calls : null,
  };
}

/** A count from Prometheus `increase()`: rounded, compact past 10,000. */
export function formatCount(n: number): string {
  const rounded = Math.round(n);
  if (rounded >= 10_000) {
    return new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 }).format(
      rounded,
    );
  }
  return rounded.toLocaleString("en");
}

export function formatPercent(n: number | null): string {
  if (n === null) return "–";
  if (n === 0) return "0%";
  return n < 1 ? `${n.toFixed(2)}%` : `${n.toFixed(1)}%`;
}

export function formatSeconds(s: number | null | undefined): string {
  if (s === null || s === undefined) return "–";
  if (s < 1) return `${Math.round(s * 1000)} ms`;
  return `${s.toFixed(s < 10 ? 2 : 1)} s`;
}

export function formatRate(n: number): string {
  return n < 10 ? n.toFixed(2) : n.toFixed(1);
}

/** An axis tick: a clean number without the unit the chart title names. */
export function formatTick(n: number): string {
  if (Number.isInteger(n)) return n.toLocaleString("en");
  return n < 1 ? n.toFixed(2) : n.toFixed(1);
}
