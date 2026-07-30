import { apiFetch } from "./client";
import type {
  CreateStorageClassRequest,
  StorageClassListResponse,
  StorageClassSummary,
} from "@/types/api";

export const storageclassesApi = {
  list: () => apiFetch<StorageClassListResponse>("/storageclasses"),

  get: (name: string) => apiFetch<StorageClassSummary>(`/storageclasses/${name}`),

  create: (req: CreateStorageClassRequest) =>
    apiFetch<StorageClassSummary>("/storageclasses", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(req),
    }),

  update: (name: string, req: CreateStorageClassRequest) =>
    apiFetch<StorageClassSummary>(`/storageclasses/${name}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(req),
    }),

  delete: (name: string) =>
    apiFetch<void>(`/storageclasses/${name}`, { method: "DELETE" }),
};
