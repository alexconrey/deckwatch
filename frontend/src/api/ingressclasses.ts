import { apiFetch } from "./client";
import type {
  CreateIngressClassRequest,
  IngressClassListResponse,
  IngressClassSummary,
} from "@/types/api";

export const ingressclassesApi = {
  list: () => apiFetch<IngressClassListResponse>("/ingressclasses"),

  get: (name: string) => apiFetch<IngressClassSummary>(`/ingressclasses/${name}`),

  create: (req: CreateIngressClassRequest) =>
    apiFetch<IngressClassSummary>("/ingressclasses", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(req),
    }),

  update: (name: string, req: CreateIngressClassRequest) =>
    apiFetch<IngressClassSummary>(`/ingressclasses/${name}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(req),
    }),

  delete: (name: string) =>
    apiFetch<void>(`/ingressclasses/${name}`, { method: "DELETE" }),
};
