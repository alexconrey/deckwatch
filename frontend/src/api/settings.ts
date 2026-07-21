import { apiFetch } from "./client";
import type { CredentialStatus, DeckwatchSettings } from "@/types/api";

export interface SetCredentialsRequest {
  anthropic_api_key?: string;
  gcp_sa_key?: string;
}

export const settingsApi = {
  get: () => apiFetch<DeckwatchSettings>("/settings"),

  update: (settings: DeckwatchSettings) =>
    apiFetch<DeckwatchSettings>("/settings", {
      method: "PUT",
      body: JSON.stringify(settings),
    }),

  setCredentials: (req: SetCredentialsRequest) =>
    apiFetch<CredentialStatus>("/settings/credentials", {
      method: "POST",
      body: JSON.stringify(req),
    }),

  testNotification: () =>
    apiFetch<void>("/notifications/test", { method: "POST" }),
};
