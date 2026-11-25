import { apiFetch } from "./client";
import type {
  DeckwatchSettings,
  ObservabilityStatus,
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

  /** Where each signal of deckwatch's own telemetry goes, now and after a restart. */
  observability: () => apiFetch<ObservabilityStatus>("/settings/observability"),
};
