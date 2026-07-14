import { apiFetch } from "./client";

// Curated query keys the backend accepts. Kept in sync with `QueryKind` in
// src/handlers/prometheus_query.rs -- new entries require both sides to
// grow together.
export type PrometheusQueryKind =
  | "cpu_usage"
  | "memory_usage"
  | "request_rate"
  | "error_rate";

// Match the response shape declared by src/handlers/prometheus_query.rs.
// `t` is milliseconds since epoch (backend multiplies Prometheus's seconds
// by 1000) so it drops directly into the same `Sample.t` field the
// ring-buffer path uses.
export interface RangePoint {
  t: number;
  v: number;
}

export interface PodSeries {
  pod: string;
  points: RangePoint[];
}

export interface RangeResponse {
  series: PodSeries[];
  unit: "cores" | "bytes" | "req/s" | string;
  unavailable_reason?: string;
  warnings?: string[];
}

export interface RangeQueryParams {
  query: PrometheusQueryKind;
  namespace: string;
  deployment: string;
  // Unix seconds. Prometheus's native format -- matches the backend.
  start: number;
  end: number;
  // Optional. Backend clamps to keep <= 2000 points regardless.
  step?: number;
}

// Named time windows exposed to the frontend. The composable turns these
// into `start`/`end` and picks a sensible `step` for each -- see
// `useResourceMetrics.ts`.
export type PromRangeWindow = "1d" | "1w";

function windowToSeconds(w: PromRangeWindow): number {
  switch (w) {
    case "1d":
      return 24 * 60 * 60;
    case "1w":
      return 7 * 24 * 60 * 60;
  }
}

// Sensible default `step` per window. Keeps the panel readable without
// leaning on the backend clamp to save us. Values chosen so 1d -> ~288 pts
// (5m step) and 1w -> ~672 pts (15m step) -- well under the 2000 cap.
function windowToStep(w: PromRangeWindow): number {
  switch (w) {
    case "1d":
      return 5 * 60;
    case "1w":
      return 15 * 60;
  }
}

export const prometheusQueryApi = {
  queryRange: (params: RangeQueryParams) => {
    const qs = new URLSearchParams({
      query: params.query,
      namespace: params.namespace,
      deployment: params.deployment,
      start: String(params.start),
      end: String(params.end),
      ...(params.step ? { step: String(params.step) } : {}),
    });
    return apiFetch<RangeResponse>(`/prometheus/query_range?${qs.toString()}`);
  },

  // Convenience wrapper for the ring-buffer -> Prometheus fallback path in
  // `useResourceMetrics.ts`. Callers pass the named window; we compute
  // start/end/step so every caller does not re-implement the arithmetic.
  queryWindow: (
    kind: PrometheusQueryKind,
    ns: string,
    deployment: string,
    window: PromRangeWindow,
    nowMs: number = Date.now(),
  ) => {
    const end = Math.floor(nowMs / 1000);
    const start = end - windowToSeconds(window);
    return prometheusQueryApi.queryRange({
      query: kind,
      namespace: ns,
      deployment,
      start,
      end,
      step: windowToStep(window),
    });
  },
};

// Small helper for the composable's polling cadence. A 1d panel does not
// need a 10s refresh -- half the step is plenty and keeps us off the
// operator's Prometheus.
export function pollIntervalForWindow(w: PromRangeWindow): number {
  return (windowToStep(w) * 1000) / 2;
}
