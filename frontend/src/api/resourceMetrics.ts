import { apiFetch } from "./client";

export interface ContainerUsage {
  name: string;
  cpu: string;
  cpu_millicores: number;
  memory: string;
  memory_bytes: number;
}

export interface PodUsage {
  name: string;
  namespace: string;
  timestamp: string;
  window: string;
  containers: ContainerUsage[];
  total_cpu_millicores: number;
  total_memory_bytes: number;
  /** Aggregate restart count across all containers, sampled from the Pod
   *  object at the same tick as the metrics-server read. Omitted when the
   *  backend could not resolve the Pod (e.g. RBAC scope mismatch). */
  restart_count?: number;
}

export interface PodMetricsResponse {
  pods: PodUsage[];
  unavailable_reason?: string;
}

export interface NodeUsage {
  name: string;
  timestamp: string;
  window: string;
  cpu: string;
  cpu_millicores: number;
  memory: string;
  memory_bytes: number;
}

export interface NodeMetricsResponse {
  nodes: NodeUsage[];
  unavailable_reason?: string;
}

export const resourceMetricsApi = {
  listPods: (ns: string, labelSelector?: string) => {
    const q = labelSelector ? `?label_selector=${encodeURIComponent(labelSelector)}` : "";
    return apiFetch<PodMetricsResponse>(`/namespaces/${encodeURIComponent(ns)}/pods/metrics${q}`);
  },
  listNodes: () => apiFetch<NodeMetricsResponse>("/nodes/metrics"),
};
