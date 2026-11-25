import { apiFetch } from "./client";
import type { AuditEntry, AuditPage, AuditQuery } from "@/types/api";

export type { AuditEntry, AuditPage, AuditQuery };

export const auditApi = {
  /** One page of audit rows, newest first unless `order: "asc"`. */
  list: (query: AuditQuery = {}) => {
    const params = new URLSearchParams();
    for (const [key, value] of Object.entries(query)) {
      if (value !== undefined && value !== null && value !== "") {
        params.set(key, String(value));
      }
    }
    const qs = params.toString();
    return apiFetch<AuditPage>(`/audit${qs ? "?" + qs : ""}`);
  },
};
