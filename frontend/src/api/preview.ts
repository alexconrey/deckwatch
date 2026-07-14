import { apiFetch } from "./client";
import type {
  CreatePreviewRequest,
  PreviewListResponse,
  PreviewSummary,
} from "@/types/api";

export const previewApi = {
  create: (ns: string, name: string, body: CreatePreviewRequest) =>
    apiFetch<PreviewSummary>(
      `/namespaces/${ns}/deployments/${name}/preview`,
      { method: "POST", body: JSON.stringify(body) },
    ),

  // Previews cloned from a specific source deployment. This is what the
  // GitOpsCard consumes so an operator sees only the previews attached to
  // the deployment they're looking at, not the whole namespace.
  listForSource: (ns: string, name: string) =>
    apiFetch<PreviewListResponse>(
      `/namespaces/${ns}/deployments/${name}/previews`,
    ),

  listNamespace: (ns: string) =>
    apiFetch<PreviewListResponse>(`/namespaces/${ns}/previews`),

  // Delete targets the preview deployment name, not the source. The
  // component that owns the row already has the preview name; passing
  // the source is a footgun that would try to delete production.
  delete: (ns: string, previewName: string) =>
    apiFetch<void>(
      `/namespaces/${ns}/deployments/${previewName}/preview`,
      { method: "DELETE" },
    ),
};
