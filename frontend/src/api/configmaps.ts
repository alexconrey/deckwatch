import { apiFetch } from "./client";
import type {
  ConfigMapDetail,
  ConfigMapListResponse,
  CreateConfigMapRequest,
} from "@/types/api";

export const configmapsApi = {
  list: (ns: string) =>
    apiFetch<ConfigMapListResponse>(`/namespaces/${ns}/configmaps`),

  get: (ns: string, name: string) =>
    apiFetch<ConfigMapDetail>(`/namespaces/${ns}/configmaps/${name}`),

  create: (ns: string, body: CreateConfigMapRequest) =>
    apiFetch<ConfigMapDetail>(`/namespaces/${ns}/configmaps`, {
      method: "POST",
      body: JSON.stringify(body),
    }),

  update: (ns: string, name: string, body: CreateConfigMapRequest) =>
    apiFetch<ConfigMapDetail>(`/namespaces/${ns}/configmaps/${name}`, {
      method: "PUT",
      body: JSON.stringify(body),
    }),

  delete: (ns: string, name: string) =>
    apiFetch<void>(`/namespaces/${ns}/configmaps/${name}`, {
      method: "DELETE",
    }),
};
