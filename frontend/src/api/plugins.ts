import { apiFetch } from "./client";
import type { ConfigField, PluginSummary } from "@/types/api";

export const pluginsApi = {
  /** GET /api/plugins — list all loaded plugins with full metadata. */
  list: () => apiFetch<PluginSummary[]>("/plugins"),

  /** GET /api/plugins/{name}/schema — config field schema for a single plugin. */
  getSchema: (name: string) =>
    apiFetch<ConfigField[]>(`/plugins/${name}/schema`),

  /**
   * POST /api/plugins/{name}/config — persist operator-supplied config values.
   * Secret-typed fields are encrypted on the backend before storage.
   */
  saveConfig: (name: string, config: Record<string, string>) =>
    apiFetch<void>(`/plugins/${name}/config`, {
      method: "POST",
      body: JSON.stringify(config),
    }),
};
