import { apiFetch } from "./client";
import type {
  AddonListResponse,
  AttachAddonRequest,
  DeploymentDetailResponse,
  UpdateAddonRequest,
} from "@/types/api";

export const addonsApi = {
  list: () => apiFetch<AddonListResponse>("/addons"),

  attach: (ns: string, name: string, addonId: string, body?: AttachAddonRequest) =>
    apiFetch<DeploymentDetailResponse>(
      `/namespaces/${ns}/deployments/${name}/addons/${addonId}`,
      { method: "POST", body: JSON.stringify(body ?? {}) },
    ),

  updateAddon: (ns: string, name: string, addonId: string, body: UpdateAddonRequest) =>
    apiFetch<DeploymentDetailResponse>(
      `/namespaces/${ns}/deployments/${name}/addons/${addonId}`,
      { method: "PATCH", body: JSON.stringify(body) },
    ),

  detach: (ns: string, name: string, addonId: string) =>
    apiFetch<DeploymentDetailResponse>(
      `/namespaces/${ns}/deployments/${name}/addons/${addonId}`,
      { method: "DELETE" },
    ),
};
