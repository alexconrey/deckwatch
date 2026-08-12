import { apiFetch } from "./client";
import type {
  CreateServiceAccountRequest,
  PatchServiceAccountRequest,
  ServiceAccountDetail,
  ServiceAccountListResponse,
} from "@/types/api";

export const serviceaccountsApi = {
  list: (ns: string) =>
    apiFetch<ServiceAccountListResponse>(
      `/namespaces/${ns}/serviceaccounts`,
    ),

  get: (ns: string, name: string) =>
    apiFetch<ServiceAccountDetail>(
      `/namespaces/${ns}/serviceaccounts/${name}`,
    ),

  create: (ns: string, body: CreateServiceAccountRequest) =>
    apiFetch<ServiceAccountDetail>(`/namespaces/${ns}/serviceaccounts`, {
      method: "POST",
      body: JSON.stringify(body),
    }),

  patch: (ns: string, name: string, body: PatchServiceAccountRequest) =>
    apiFetch<ServiceAccountDetail>(
      `/namespaces/${ns}/serviceaccounts/${name}`,
      {
        method: "PATCH",
        body: JSON.stringify(body),
      },
    ),

  delete: (ns: string, name: string) =>
    apiFetch<void>(`/namespaces/${ns}/serviceaccounts/${name}`, {
      method: "DELETE",
    }),
};
