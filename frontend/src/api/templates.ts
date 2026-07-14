import { apiFetch } from "./client";
import type {
  DeploymentTemplate,
  TemplateListResponse,
  TemplatesUpdateRequest,
} from "@/types/api";

export const templatesApi = {
  list: () => apiFetch<TemplateListResponse>("/templates"),

  update: (templates: DeploymentTemplate[]) =>
    apiFetch<TemplateListResponse>("/templates", {
      method: "PUT",
      body: JSON.stringify({ templates } satisfies TemplatesUpdateRequest),
    }),
};
