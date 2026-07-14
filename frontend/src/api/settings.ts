import { apiFetch } from "./client";
import type { DeckwatchSettings } from "@/types/api";

export const settingsApi = {
  get: () => apiFetch<DeckwatchSettings>("/settings"),

  update: (settings: DeckwatchSettings) =>
    apiFetch<DeckwatchSettings>("/settings", {
      method: "PUT",
      body: JSON.stringify(settings),
    }),
  testNotification: () =>
    apiFetch<void>("/notifications/test", { method: "POST" }),
};
