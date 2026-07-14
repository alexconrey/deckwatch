import { apiFetch } from "./client";
import type {
  CreateNamespaceRequest,
  CreateNamespaceResponse,
  NamespaceListResponse,
} from "@/types/api";

export const namespacesApi = {
  list: () => apiFetch<NamespaceListResponse>("/namespaces"),

  create: (body: CreateNamespaceRequest) =>
    apiFetch<CreateNamespaceResponse>("/namespaces", {
      method: "POST",
      body: JSON.stringify(body),
    }),
};
