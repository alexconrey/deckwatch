// Diagnostics API client.
//
// Adds `quota(ns)` — the standalone snapshot the UI polls before showing
// the "Diagnose with AI" button so operators see remaining slots without
// needing to submit a request.

import { apiFetch } from "./client";
import type {
  AiQuotaSnapshot,
  DiagAgent,
  DiagnoseRequest,
  DiagnoseResponse,
  DiagnosticHistoryResponse,
  DiagnosticStatusResponse,
  DiagnosticResultResponse,
} from "@/types/api";

export const diagnosticsApi = {
  create: (ns: string, req: DiagnoseRequest) =>
    apiFetch<DiagnoseResponse>(`/namespaces/${ns}/diagnostics`, {
      method: "POST",
      body: JSON.stringify(req),
    }),

  list: (ns: string) =>
    apiFetch<DiagnosticHistoryResponse>(`/namespaces/${ns}/diagnostics`),

  status: (ns: string, jobName: string) =>
    apiFetch<DiagnosticStatusResponse>(
      `/namespaces/${ns}/diagnostics/${jobName}`,
    ),

  result: (ns: string, jobName: string) =>
    apiFetch<DiagnosticResultResponse>(
      `/namespaces/${ns}/diagnostics/${jobName}/result`,
    ),

  // Cheap read; the backend prunes expired entries as a side effect so
  // the reported `used` is always in-window. Polling once on mount and
  // again after a submit is enough — no need for a periodic timer.
  quota: (ns: string) =>
    apiFetch<AiQuotaSnapshot>(`/namespaces/${ns}/ai-quota`),

  // EventSource doesn't ride through `apiFetch` (no fetch, no bearer header
  // support in the browser API), so we hand callers the raw URL and let them
  // instantiate the EventSource themselves. Cookie-auth deployments Just
  // Work; bearer-auth deployments will need to fall back to polling if the
  // browser refuses the unauthenticated SSE — the caller handles that.
  streamUrl: (ns: string, jobName: string) =>
    `/api/namespaces/${ns}/diagnostics/${jobName}/stream`,
};

export type { DiagAgent };
