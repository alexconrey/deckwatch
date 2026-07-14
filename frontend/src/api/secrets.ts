import { apiFetch } from "./client";
import type {
  CreateSecretRequest,
  SecretDetail,
  SecretListResponse,
} from "@/types/api";

export const secretsApi = {
  list: (ns: string) =>
    apiFetch<SecretListResponse>(`/namespaces/${ns}/secrets`),

  get: (ns: string, name: string, reveal = false) =>
    apiFetch<SecretDetail>(
      `/namespaces/${ns}/secrets/${name}${reveal ? "?reveal=true" : ""}`,
    ),

  create: (ns: string, body: CreateSecretRequest) =>
    apiFetch<SecretDetail>(`/namespaces/${ns}/secrets`, {
      method: "POST",
      body: JSON.stringify(body),
    }),

  update: (ns: string, name: string, body: CreateSecretRequest) =>
    apiFetch<SecretDetail>(`/namespaces/${ns}/secrets/${name}`, {
      method: "PUT",
      body: JSON.stringify(body),
    }),

  delete: (ns: string, name: string) =>
    apiFetch<void>(`/namespaces/${ns}/secrets/${name}`, {
      method: "DELETE",
    }),
};
