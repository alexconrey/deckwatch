import { ApiError, apiFetch } from "./client";
import type { MonitorConfigRequest, MonitorSettings } from "@/types/monitoring";

// The backend returns 503 with a populated `unavailable_reason` when the
// prometheus-operator CRDs are missing. That is a valid state to render (the
// UI shows an "install prometheus-operator" callout), not an error condition,
// so we treat any parseable JSON body as a success and only throw on network
// errors or malformed responses.
//
// ApiError.body is typed as `{ error, message }` (the generic 4xx/5xx shape
// used by the rest of the backend) but at runtime it holds whatever JSON the
// server returned. The graceful-degrade path returns a full MonitorSettings
// blob with `unavailable_reason` set, so we cast through `unknown` to reach
// it without loosening the ApiError type for every other caller.
function unwrap503(e: unknown): MonitorSettings {
  if (e instanceof ApiError && e.status === 503 && e.body) {
    const body = e.body as unknown as MonitorSettings;
    if (typeof body.unavailable_reason === "string") {
      return body;
    }
  }
  throw e;
}

async function fetchMonitor(
  ns: string,
  name: string,
  init?: RequestInit,
): Promise<MonitorSettings> {
  try {
    return await apiFetch<MonitorSettings>(
      `/namespaces/${ns}/deployments/${name}/monitor`,
      init,
    );
  } catch (e) {
    return unwrap503(e);
  }
}

export const monitoringApi = {
  get: (ns: string, name: string) => fetchMonitor(ns, name),

  upsert: (ns: string, name: string, body: MonitorConfigRequest) =>
    fetchMonitor(ns, name, {
      method: "PUT",
      body: JSON.stringify(body),
    }),

  disable: (ns: string, name: string) =>
    apiFetch<void>(`/namespaces/${ns}/deployments/${name}/monitor`, {
      method: "DELETE",
    }),
};
