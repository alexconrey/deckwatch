import { apiFetch } from "./client";
import type {
  AddMemberRequest,
  ApplicationDetail,
  ApplicationListResponse,
  CreateApplicationRequest,
  UpdateApplicationRequest,
} from "@/types/api";

export const applicationsApi = {
  list: (ns: string) =>
    apiFetch<ApplicationListResponse>(`/namespaces/${ns}/applications`),

  get: (ns: string, name: string) =>
    apiFetch<ApplicationDetail>(`/namespaces/${ns}/applications/${name}`),

  create: (ns: string, body: CreateApplicationRequest) =>
    apiFetch<ApplicationDetail>(`/namespaces/${ns}/applications`, {
      method: "POST",
      body: JSON.stringify(body),
    }),

  update: (ns: string, name: string, body: UpdateApplicationRequest) =>
    apiFetch<ApplicationDetail>(`/namespaces/${ns}/applications/${name}`, {
      method: "PUT",
      body: JSON.stringify(body),
    }),

  delete: (ns: string, name: string, cascade = false) =>
    apiFetch<void>(
      `/namespaces/${ns}/applications/${name}?cascade=${cascade}`,
      { method: "DELETE" },
    ),

  addMember: (ns: string, name: string, body: AddMemberRequest) =>
    apiFetch<ApplicationDetail>(
      `/namespaces/${ns}/applications/${name}/members`,
      {
        method: "POST",
        body: JSON.stringify(body),
      },
    ),

  removeMember: (ns: string, name: string, kind: string, resource: string) =>
    apiFetch<ApplicationDetail>(
      `/namespaces/${ns}/applications/${name}/members/${kind}/${resource}`,
      { method: "DELETE" },
    ),
};
