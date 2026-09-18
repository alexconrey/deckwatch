import { apiFetch } from "./client";
import type {
  CronJobDetailResponse,
  CronJobListResponse,
} from "@/types/api";

export interface TriggerResponse {
  job_name: string;
}

export interface CronJobLogsResponse {
  job_name: string;
  pod_name: string;
  logs: string;
}

export const cronjobsApi = {
  list: (ns: string) =>
    apiFetch<CronJobListResponse>(`/namespaces/${ns}/cronjobs`),

  get: (ns: string, name: string) =>
    apiFetch<CronJobDetailResponse>(`/namespaces/${ns}/cronjobs/${name}`),

  trigger: (ns: string, name: string) =>
    apiFetch<TriggerResponse>(`/namespaces/${ns}/cronjobs/${name}/trigger`, {
      method: "POST",
    }),

  getLogs: (ns: string, name: string) =>
    apiFetch<CronJobLogsResponse>(`/namespaces/${ns}/cronjobs/${name}/logs`),
};
