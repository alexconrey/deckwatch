import { apiFetch } from "./client";
import type { ProvisionedResource, ProvisionRequest } from "@/types/api";

export const applicationResourcesApi = {
  /** GET /api/namespaces/{ns}/applications/{name}/resources */
  list: (ns: string, appName: string) =>
    apiFetch<ProvisionedResource[]>(
      `/namespaces/${ns}/applications/${appName}/resources`,
    ),

  /** POST /api/namespaces/{ns}/applications/{name}/resources/{plugin}/{resource_id} */
  provision: (
    ns: string,
    appName: string,
    pluginName: string,
    resourceId: string,
    body: ProvisionRequest,
  ) =>
    apiFetch<ProvisionedResource>(
      `/namespaces/${ns}/applications/${appName}/resources/${pluginName}/${resourceId}`,
      {
        method: "POST",
        body: JSON.stringify(body),
      },
    ),

  /** DELETE /api/namespaces/{ns}/applications/{name}/resources/{plugin}/{resource_id} */
  deprovision: (
    ns: string,
    appName: string,
    pluginName: string,
    resourceId: string,
  ) =>
    apiFetch<void>(
      `/namespaces/${ns}/applications/${appName}/resources/${pluginName}/${resourceId}`,
      { method: "DELETE" },
    ),
};
