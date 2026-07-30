import { apiFetch } from "./client";
import type { StorageClassListResponse } from "@/types/api";

export const storageclassesApi = {
  list: () => apiFetch<StorageClassListResponse>("/storageclasses"),
};
