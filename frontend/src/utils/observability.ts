// Helpers for Settings → Observability → OpenTelemetry export: the OTLP
// endpoints deckwatch exports its own telemetry to. Validation mirrors
// validate_endpoint in src/telemetry/endpoints.rs so operators get feedback
// while typing; the server validates again on save and is the authority.
import type {
  ObservabilitySettings,
  ObservabilityStatus,
  OtlpSignal,
  OtlpSignalExport,
  OtlpSignalStatus,
  OtlpSource,
} from "@/types/api";

export type OtlpField = keyof ObservabilitySettings;

const MAX_LENGTH = 2048;
const SIGNAL_PATHS = ["/v1/traces", "/v1/metrics", "/v1/logs"];

/** The signals each field carries. */
export const FIELD_SIGNALS: Record<OtlpField, OtlpSignal[]> = {
  otlp_endpoint: ["traces", "logs"],
  otlp_metrics_endpoint: ["metrics"],
};

export const SIGNAL_LABELS: Record<OtlpSignal, string> = {
  traces: "Traces",
  metrics: "Metrics",
  logs: "Logs",
};

export function emptyObservability(): ObservabilitySettings {
  return { otlp_endpoint: "", otlp_metrics_endpoint: "" };
}

/**
 * Validate an OTLP endpoint. Returns an error message, or null when the value
 * is acceptable (empty means "not set").
 */
export function validateOtlpEndpoint(field: OtlpField, raw: string): string | null {
  const value = raw.trim();
  if (value === "") return null;
  if (value.length > MAX_LENGTH) return `Longer than ${MAX_LENGTH} characters.`;
  // Without a scheme, "collector:4317" would parse as a URL with the scheme
  // "collector".
  if (!value.includes("://")) {
    return "Use an http:// or https:// URL: deckwatch exports OTLP over HTTP.";
  }
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return "Not a valid URL.";
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    return `Use http:// or https:// (got ${url.protocol}//).`;
  }
  if (url.username || url.password) {
    return "Must not contain a username or password.";
  }
  if (url.search || url.hash || /[?#]/.test(value)) {
    return "Must not contain a query string or fragment.";
  }
  if (!url.hostname) return "A host is required.";
  const path = url.pathname.replace(/\/+$/, "");
  if (field === "otlp_endpoint") {
    const suffix = SIGNAL_PATHS.find((p) => path.endsWith(p));
    if (suffix) {
      return `Enter the base URL, without ${suffix}: deckwatch appends /v1/traces and /v1/logs.`;
    }
  } else if (path === "") {
    return "Enter the full URL of the receiver, such as http://prometheus.monitoring:9090/api/v1/otlp/v1/metrics.";
  }
  return null;
}

/** The first invalid endpoint's error, prefixed with its field, or null. */
export function validateObservability(settings: ObservabilitySettings): string | null {
  const labels: Record<OtlpField, string> = {
    otlp_endpoint: "OTLP endpoint",
    otlp_metrics_endpoint: "OTLP metrics endpoint",
  };
  for (const field of Object.keys(labels) as OtlpField[]) {
    const error = validateOtlpEndpoint(field, settings[field]);
    if (error) return `${labels[field]}: ${error}`;
  }
  return null;
}

export function sourceLabel(source: OtlpSource): string {
  switch (source) {
    case "env":
      return "Environment";
    case "settings":
      return "Settings";
    default:
      return "Not set";
  }
}

/** Where a signal goes, as one line: the endpoint, or why there's none. */
export function exportSummary(e: OtlpSignalExport): string {
  if (e.source === "unset") return "Not exported";
  if (!e.exported) return `Not exported: ${e.reason ?? "turned off"}`;
  return e.endpoint;
}

/** What the signal is exported to now: as set up at startup. */
export function current(s: OtlpSignalStatus): OtlpSignalExport {
  return s.running ?? s.configured;
}

/** The saved settings send this signal somewhere else from the next start. */
export function changesOnRestart(s: OtlpSignalStatus): boolean {
  return (
    s.running !== null &&
    (s.running.source !== s.configured.source || s.running.endpoint !== s.configured.endpoint)
  );
}

/**
 * When environment variables win over `field` for some of its signals, a
 * sentence saying so, e.g. "Traces use OTEL_EXPORTER_OTLP_TRACES_ENDPOINT
 * instead."; otherwise null.
 */
export function overrideHint(status: ObservabilityStatus | null, field: OtlpField): string | null {
  const fromEnv = (status?.signals ?? []).filter(
    (s) => FIELD_SIGNALS[field].includes(s.signal) && s.configured.source === "env",
  );
  if (fromEnv.length === 0) return null;
  const signals = fromEnv.map((s) => s.signal).join(" and ");
  const vars = [...new Set(fromEnv.map((s) => s.configured.from ?? "an environment variable"))];
  const subject = signals.charAt(0).toUpperCase() + signals.slice(1);
  return `${subject} use ${vars.join(" and ")} instead: an environment variable wins over this setting.`;
}

/** localStorage key remembering the retired endpoint notice was dismissed. */
export const RETIRED_NOTICE_KEY = "deckwatch.retiredOtlpEndpointNotice";

type KeyValueStore = Pick<Storage, "getItem" | "setItem">;

/** The notice for the retired tracing.otlp_endpoint `value` was dismissed in this browser. */
export function retiredNoticeDismissed(value: string, store: KeyValueStore = localStorage): boolean {
  return store.getItem(RETIRED_NOTICE_KEY) === value;
}

/** Remember the notice for `value` was dismissed; a different value shows it again. */
export function dismissRetiredNotice(value: string, store: KeyValueStore = localStorage): void {
  store.setItem(RETIRED_NOTICE_KEY, value);
}
