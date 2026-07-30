import { apiFetch } from "./client";
import type {
  GitTokenSecretRequest,
  GitTokenSecretResponse,
} from "@/types/api";

export const gitTokensApi = {
  upsert: (req: GitTokenSecretRequest) =>
    apiFetch<GitTokenSecretResponse>("/git-token-secrets", {
      method: "PUT",
      body: JSON.stringify(req),
    }),

  remove: (secretName: string) =>
    apiFetch<void>(`/git-token-secrets/${encodeURIComponent(secretName)}`, {
      method: "DELETE",
    }),
};
