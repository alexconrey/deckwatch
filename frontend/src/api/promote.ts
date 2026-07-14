import { apiFetch } from "./client";
import type { PromoteRequest, PromoteResponse } from "@/types/api";

export const promoteApi = {
  // Two-step flow: preview() runs the diff without mutating; apply()
  // patches the target and returns the same diff plus the fresh target
  // detail. Server enforces this via a `dry_run` query param on a single
  // endpoint — keeping the wire shape identical between preview and apply
  // means the UI renders one component that binds to the same response.
  preview: (ns: string, name: string, body: PromoteRequest) =>
    apiFetch<PromoteResponse>(
      `/namespaces/${ns}/deployments/${name}/promote?dry_run=true`,
      { method: "POST", body: JSON.stringify(body) },
    ),

  apply: (ns: string, name: string, body: PromoteRequest) =>
    apiFetch<PromoteResponse>(
      `/namespaces/${ns}/deployments/${name}/promote`,
      { method: "POST", body: JSON.stringify(body) },
    ),
};
