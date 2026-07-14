import { apiFetch } from "./client";
import type {
  CreateIngressRequest,
  IngressDetail,
  IngressListResponse,
} from "@/types/api";

export const ingressesApi = {
  list: (ns: string) =>
    apiFetch<IngressListResponse>(`/namespaces/${ns}/ingresses`),

  get: (ns: string, name: string) =>
    apiFetch<IngressDetail>(`/namespaces/${ns}/ingresses/${name}`),

  create: (ns: string, body: CreateIngressRequest) =>
    apiFetch<IngressDetail>(`/namespaces/${ns}/ingresses`, {
      method: "POST",
      body: JSON.stringify(body),
    }),

  update: (ns: string, name: string, body: CreateIngressRequest) =>
    apiFetch<IngressDetail>(`/namespaces/${ns}/ingresses/${name}`, {
      method: "PUT",
      body: JSON.stringify(body),
    }),

  delete: (ns: string, name: string) =>
    apiFetch<void>(`/namespaces/${ns}/ingresses/${name}`, {
      method: "DELETE",
    }),
};
