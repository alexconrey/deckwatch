import type {
  PluginEventReport,
  PluginEventSeverity,
  PluginHealth,
  PluginStatus,
} from "@/types/api";

/** Most severe first. */
const RANK: Record<PluginEventSeverity, number> = { error: 0, warning: 1, info: 2 };

export function severityIcon(severity: PluginEventSeverity): string {
  switch (severity) {
    case "error":
      return "mdi-alert-circle";
    case "warning":
      return "mdi-alert";
    default:
      return "mdi-information";
  }
}

export function severityColor(severity: PluginEventSeverity): string {
  return severity === "info" ? "info" : severity;
}

/** Events sorted most severe first; stable within a severity. */
export function sortEvents(events: PluginEventReport[]): PluginEventReport[] {
  return [...events].sort((a, b) => (RANK[a.severity] ?? 1) - (RANK[b.severity] ?? 1));
}

/** True when any event is a warning or an error. */
export function hasProblems(events: PluginEventReport[] | undefined): boolean {
  return (events ?? []).some((e) => e.severity !== "info");
}

/** The worst severity among `events`, or null for none. */
export function worstSeverity(
  events: PluginEventReport[] | undefined,
): PluginEventSeverity | null {
  const sorted = sortEvents(events ?? []);
  return sorted.length ? sorted[0].severity : null;
}

/** "1 error, 2 warnings" — empty string for no events. */
export function summarizeEvents(events: PluginEventReport[] | undefined): string {
  const counts = { error: 0, warning: 0, info: 0 };
  for (const e of events ?? []) counts[e.severity in counts ? e.severity : "warning"]++;
  const parts: string[] = [];
  if (counts.error) parts.push(`${counts.error} error${counts.error === 1 ? "" : "s"}`);
  if (counts.warning) parts.push(`${counts.warning} warning${counts.warning === 1 ? "" : "s"}`);
  if (counts.info) parts.push(`${counts.info} notice${counts.info === 1 ? "" : "s"}`);
  return parts.join(", ");
}

const HEALTH_RANK: Record<PluginHealth, number> = { failing: 0, degraded: 1, healthy: 2 };

/** The worst health among `statuses`, or null for none. */
export function worstHealth(statuses: PluginStatus[] | undefined): PluginHealth | null {
  let worst: PluginHealth | null = null;
  for (const s of statuses ?? []) {
    if (worst === null || HEALTH_RANK[s.health] < HEALTH_RANK[worst]) worst = s.health;
  }
  return worst;
}

/** What a status row is about, for people: "shop/web · rds", "vault (load)". */
export function statusTarget(s: PluginStatus): string {
  if (!s.application) return `${s.plugin} (load)`;
  const app = `${s.namespace}/${s.application}`;
  return s.resource_id ? `${app} · ${s.resource_id}` : `${app} · apply`;
}
