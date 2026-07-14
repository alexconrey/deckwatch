import { ApiError, apiFetch } from "./client";
import type { HpaConfigRequest, HpaResponse } from "@/types/api";

export const autoscalingApi = {
  // Returns null when no HPA exists (404) so callers can render an
  // "enable autoscaling" UI without treating the missing resource as an error.
  get: async (ns: string, name: string): Promise<HpaResponse | null> => {
    try {
      return await apiFetch<HpaResponse>(
        `/namespaces/${ns}/deployments/${name}/hpa`,
      );
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) return null;
      throw e;
    }
  },

  upsert: (ns: string, name: string, body: HpaConfigRequest) =>
    apiFetch<HpaResponse>(`/namespaces/${ns}/deployments/${name}/hpa`, {
      method: "PUT",
      body: JSON.stringify(body),
    }),

  delete: (ns: string, name: string) =>
    apiFetch<void>(`/namespaces/${ns}/deployments/${name}/hpa`, {
      method: "DELETE",
    }),
};
