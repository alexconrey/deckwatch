import { apiFetch } from "./client";
import type { EventListResponse } from "@/types/api";

export interface ListEventsParams {
  involvedObject?: string;
  fieldSelector?: string;
}

function buildQuery(params?: ListEventsParams): string {
  if (!params) return "";
  const search = new URLSearchParams();
  if (params.involvedObject) search.set("involved_object", params.involvedObject);
  if (params.fieldSelector) search.set("field_selector", params.fieldSelector);
  const qs = search.toString();
  return qs ? `?${qs}` : "";
}

export const eventsApi = {
  // Namespaced events. When `involvedObject` is provided the backend applies
  // `involvedObject.name=<name>` as a field selector so the response is
  // pre-filtered to a single object (deployment, pod, etc.).
  list: (ns: string, params?: ListEventsParams) =>
    apiFetch<EventListResponse>(
      `/namespaces/${ns}/events${buildQuery(params)}`,
    ),

  // Cluster-wide events across all namespaces the caller is allowed to see.
  // The backend filters against the namespace allowlist before returning.
  listCluster: (params?: ListEventsParams) =>
    apiFetch<EventListResponse>(`/events${buildQuery(params)}`),
};
