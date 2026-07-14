import { apiFetch } from "./client";

export interface AuditEntry {
  id: string;
  timestamp: string;
  action: string;
  resource_type: string;
  resource_name: string;
  namespace: string;
  detail: string;
  user_identity: string;
}

export const auditApi = {
  list: (opts?: { resource_type?: string; namespace?: string; limit?: number }) => {
    const params = new URLSearchParams();
    if (opts?.resource_type) params.set("resource_type", opts.resource_type);
    if (opts?.namespace) params.set("namespace", opts.namespace);
    if (opts?.limit) params.set("limit", String(opts.limit));
    const qs = params.toString();
    return apiFetch<{ entries: AuditEntry[] }>(`/audit${qs ? "?" + qs : ""}`);
  },
};
