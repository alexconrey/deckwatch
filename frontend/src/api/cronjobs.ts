import { apiFetch } from "./client";
import type {
  CronJobDetailResponse,
  CronJobListResponse,
} from "@/types/api";

export const cronjobsApi = {
  list: (ns: string) =>
    apiFetch<CronJobListResponse>(`/namespaces/${ns}/cronjobs`),

  get: (ns: string, name: string) =>
    apiFetch<CronJobDetailResponse>(`/namespaces/${ns}/cronjobs/${name}`),
};
