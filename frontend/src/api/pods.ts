import { apiFetch } from "./client";
import type { PodSummary } from "@/types/api";

export const podsApi = {
  listForDeployment: (ns: string, deploymentName: string) =>
    apiFetch<{ pods: PodSummary[] }>(
      `/namespaces/${ns}/deployments/${deploymentName}/pods`,
    ),

  get: (ns: string, podName: string) =>
    apiFetch<PodSummary>(`/namespaces/${ns}/pods/${podName}`),
};
