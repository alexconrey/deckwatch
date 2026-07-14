import { apiFetch } from "./client";
import type {
  CloneRequest,
  CloneResponse,
  CreateDeploymentRequest,
  DeploymentDetailResponse,
  HistoryResponse,
  RollbackRequest,
  UpdateDeploymentRequest,
  ValidateResponse,
} from "@/types/api";

export const deploymentsUxApi = {
  history: (ns: string, name: string) =>
    apiFetch<HistoryResponse>(
      `/namespaces/${ns}/deployments/${name}/history`,
    ),

  rollback: (ns: string, name: string, body: RollbackRequest) =>
    apiFetch<DeploymentDetailResponse>(
      `/namespaces/${ns}/deployments/${name}/rollback`,
      { method: "POST", body: JSON.stringify(body) },
    ),

  validate: (ns: string, body: CreateDeploymentRequest) =>
    apiFetch<ValidateResponse>(
      `/namespaces/${ns}/deployments/validate`,
      { method: "POST", body: JSON.stringify(body) },
    ),

  // PUT-path (edit-existing) dry-run. Same response envelope as `validate`,
  // but targets a per-deployment URL and posts the update-shape body so
  // the server dry-runs a replace() rather than a create().
  validateUpdate: (ns: string, name: string, body: UpdateDeploymentRequest) =>
    apiFetch<ValidateResponse>(
      `/namespaces/${ns}/deployments/${name}/validate`,
      { method: "POST", body: JSON.stringify(body) },
    ),

  clone: (ns: string, name: string, body: CloneRequest) =>
    apiFetch<CloneResponse>(
      `/namespaces/${ns}/deployments/${name}/clone`,
      { method: "POST", body: JSON.stringify(body) },
    ),
};
