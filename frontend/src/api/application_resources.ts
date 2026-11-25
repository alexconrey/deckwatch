import { apiFetch } from "./client";
import type {
  DeprovisionResponse,
  ProvisionedResource,
  ProvisionRequest,
} from "@/types/api";

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

  /**
   * DELETE /api/namespaces/{ns}/applications/{name}/resources/{plugin}/{resource_id}
   *
   * Returns the plugin's message about what it did and didn't clean up, and
   * its events. The record is removed even when the plugin reports errors.
   */
  deprovision: (
    ns: string,
    appName: string,
    pluginName: string,
    resourceId: string,
  ) =>
    apiFetch<DeprovisionResponse>(
      `/namespaces/${ns}/applications/${appName}/resources/${pluginName}/${resourceId}`,
      { method: "DELETE" },
    ),
};
