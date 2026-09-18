import { apiFetch } from "./client";
import type {
  CronJobDetailResponse,
  CronJobListResponse,
  CronJobSummary,
} from "@/types/api";

export interface TriggerResponse {
  job_name: string;
}

export interface CronJobLogsResponse {
  job_name: string;
  pod_name: string;
  logs: string;
}

export interface CreateCronJobRequest {
  name: string;
  schedule: string;
  image: string;
  command?: string[];
  args?: string[];
  suspend?: boolean;
  restart_policy?: string;
}

export interface UpdateCronJobRequest {
  schedule?: string;
  image?: string;
  suspend?: boolean;
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

  create: (ns: string, body: CreateCronJobRequest) =>
    apiFetch<CronJobSummary>(`/namespaces/${ns}/cronjobs`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),

  update: (ns: string, name: string, body: UpdateCronJobRequest) =>
    apiFetch<CronJobSummary>(`/namespaces/${ns}/cronjobs/${name}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),

  delete: (ns: string, name: string) =>
    apiFetch<void>(`/namespaces/${ns}/cronjobs/${name}`, {
      method: "DELETE",
    }),
};
