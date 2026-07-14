import { ApiError, apiFetch } from "./client";
import type {
  AddContainerRequest,
  CreateDeploymentRequest,
  DeploymentDetailResponse,
  DeploymentListResponse,
  UpdateDeploymentRequest,
  UpdateProbesRequest,
} from "@/types/api";

const BASE_URL = "/api";

async function fetchYamlText(path: string): Promise<string> {
  const response = await fetch(`${BASE_URL}${path}`, {
    headers: { Accept: "text/yaml" },
  });
  if (!response.ok) {
    const body = await response.json();
    throw new ApiError(response.status, body);
  }
  return response.text();
}

export const deploymentsApi = {
  list: (ns: string) =>
    apiFetch<DeploymentListResponse>(`/namespaces/${ns}/deployments`),

  get: (ns: string, name: string) =>
    apiFetch<DeploymentDetailResponse>(`/namespaces/${ns}/deployments/${name}`),

  create: (ns: string, body: CreateDeploymentRequest) =>
    apiFetch<DeploymentDetailResponse>(`/namespaces/${ns}/deployments`, {
      method: "POST",
      body: JSON.stringify(body),
    }),

  update: (ns: string, name: string, body: UpdateDeploymentRequest) =>
    apiFetch<DeploymentDetailResponse>(`/namespaces/${ns}/deployments/${name}`, {
      method: "PUT",
      body: JSON.stringify(body),
    }),

  delete: (ns: string, name: string) =>
    apiFetch<void>(`/namespaces/${ns}/deployments/${name}`, { method: "DELETE" }),

  restart: (ns: string, name: string) =>
    apiFetch<{ message: string }>(`/namespaces/${ns}/deployments/${name}/restart`, { method: "POST" }),

  scale: (ns: string, name: string, replicas: number) =>
    apiFetch<DeploymentDetailResponse>(`/namespaces/${ns}/deployments/${name}/scale`, {
      method: "POST",
      body: JSON.stringify({ replicas }),
    }),

  // YAML features
  getYaml: (ns: string, name: string): Promise<string> =>
    fetchYamlText(`/namespaces/${ns}/deployments/${name}/yaml`),

  updateYaml: async (ns: string, name: string, yaml: string): Promise<DeploymentDetailResponse> => {
    const response = await fetch(`${BASE_URL}/namespaces/${ns}/deployments/${name}/yaml`, {
      method: "PUT",
      headers: { "Content-Type": "text/yaml" },
      body: yaml,
    });
    if (!response.ok) {
      const body = await response.json();
      throw new ApiError(response.status, body);
    }
    return response.json() as Promise<DeploymentDetailResponse>;
  },

  // Probe management
  updateProbes: (ns: string, name: string, body: UpdateProbesRequest) =>
    apiFetch<DeploymentDetailResponse>(`/namespaces/${ns}/deployments/${name}/probes`, {
      method: "PATCH",
      body: JSON.stringify(body),
    }),

  // Sidecar container management
  addContainer: (ns: string, name: string, body: AddContainerRequest) =>
    apiFetch<DeploymentDetailResponse>(`/namespaces/${ns}/deployments/${name}/containers`, {
      method: "POST",
      body: JSON.stringify(body),
    }),

  removeContainer: (ns: string, name: string, containerName: string) =>
    apiFetch<DeploymentDetailResponse>(
      `/namespaces/${ns}/deployments/${name}/containers/${containerName}`,
      { method: "DELETE" },
    ),
};
