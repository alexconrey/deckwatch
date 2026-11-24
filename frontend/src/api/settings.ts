import { apiFetch } from "./client";
import type {
  DeckwatchSettings,
  OutboundStatus,
  SetCredentialsRequest,
  SetCredentialsResponse,
} from "@/types/api";

export const settingsApi = {
  get: () => apiFetch<DeckwatchSettings>("/settings"),

  update: (settings: DeckwatchSettings) =>
    apiFetch<DeckwatchSettings>("/settings", {
      method: "PUT",
      body: JSON.stringify(settings),
    }),

  setCredentials: (req: SetCredentialsRequest) =>
    apiFetch<SetCredentialsResponse>("/settings/credentials", {
      method: "POST",
      body: JSON.stringify(req),
    }),

  testNotification: () =>
    apiFetch<void>("/notifications/test", { method: "POST" }),

  /** Endpoint defaults, credential bindings and the TLS/endpoint policy. */
  outbound: () => apiFetch<OutboundStatus>("/settings/outbound"),
};
