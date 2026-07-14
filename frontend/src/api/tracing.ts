import { apiFetch } from "./client";

// Response shape mirrors src/handlers/tracing.rs::ListTracesResponse.
// Kept as a separate module (rather than folded into deployments.ts) so
// tracing can be feature-flagged out of a build later without touching
// unrelated code paths.

export interface TraceSummary {
  trace_id: string;
  root_span_name: string;
  duration_ms: number;
  span_count: number;
  timestamp_ms: number;
}

export interface ListTracesResponse {
  traces: TraceSummary[];
  // Present when the backend could not serve the request (tracing not
  // configured, backend unreachable, backend returned an error). UI shows
  // this as a callout, not an error banner.
  unavailable_reason?: string;
  // Backend-UI deep-link base; empty string means the "Open in UI" affordance
  // should be hidden. Echoed on every response so the card renders without a
  // second settings roundtrip.
  ui_url: string;
  // "tempo" | "jaeger" — drives which URL template to use in traceUrlFor().
  backend_kind: string;
}

export const tracingApi = {
  listTraces: (
    namespace: string,
    deployment: string,
    service: string,
    limit = 20,
  ) => {
    const qs = new URLSearchParams({
      service,
      limit: String(limit),
    });
    return apiFetch<ListTracesResponse>(
      `/namespaces/${encodeURIComponent(namespace)}/deployments/${encodeURIComponent(deployment)}/traces?${qs.toString()}`,
    );
  },
};

// Build a deep-link URL for a specific trace ID, given the backend kind and
// the UI base URL from settings. Returns null when uiUrl is empty so callers
// can hide the link affordance.
//
// Grafana Explore's URL structure is stable at the top level but the `left`
// param nesting shifts across Grafana versions -- we build the minimal form
// that's worked since Grafana 9. Operators who use a fancier layout can
// override the ui_url with a full pre-baked Explore URL and get the trace
// ID appended as a query string; the Tempo branch below handles either.
export function traceUrlFor(
  backendKind: string,
  uiUrl: string,
  traceId: string,
): string | null {
  if (!uiUrl || !traceId) return null;
  const base = uiUrl.replace(/\/+$/, "");
  const kind = (backendKind || "tempo").toLowerCase();
  if (kind === "jaeger") {
    return `${base}/trace/${encodeURIComponent(traceId)}`;
  }
  // Tempo: default to a Grafana Explore URL with a Tempo datasource query.
  // The `left` param is JSON but Grafana accepts it URL-encoded verbatim.
  const explore = {
    datasource: "tempo",
    queries: [{ query: traceId, queryType: "traceId" }],
    range: { from: "now-1h", to: "now" },
  };
  return `${base}/explore?left=${encodeURIComponent(JSON.stringify(explore))}`;
}
