// Diagnostics API client.

import { apiFetch } from "./client";
import type {
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

  // EventSource doesn't ride through `apiFetch` (no fetch, no bearer header
  // support in the browser API), so we hand callers the raw URL and let them
  // instantiate the EventSource themselves. Cookie-auth deployments Just
  // Work; bearer-auth deployments will need to fall back to polling if the
  // browser refuses the unauthenticated SSE — the caller handles that.
  streamUrl: (ns: string, jobName: string) =>
    `/api/namespaces/${ns}/diagnostics/${jobName}/stream`,
};

export type { DiagAgent };
