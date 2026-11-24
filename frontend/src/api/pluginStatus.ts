import { apiFetch } from "./client";
import type { PluginHealth, PluginStatus } from "@/types/api";

export interface PluginStatusFilter {
  health?: PluginHealth[];
  plugin?: string;
  namespace?: string;
}

export const pluginStatusApi = {
  /** GET /api/plugin-status — every target, most severe first. */
  list: (filter: PluginStatusFilter = {}) => {
    const params = new URLSearchParams();
    if (filter.health?.length) params.set("health", filter.health.join(","));
    if (filter.plugin) params.set("plugin", filter.plugin);
    if (filter.namespace) params.set("namespace", filter.namespace);
    const query = params.toString();
    return apiFetch<PluginStatus[]>(`/plugin-status${query ? `?${query}` : ""}`);
  },

  /** Failing and degraded targets: the Problems view. */
  problems: () => pluginStatusApi.list({ health: ["failing", "degraded"] }),

  /** GET /api/namespaces/{ns}/applications/{name}/plugin-status */
  forApplication: (ns: string, name: string) =>
    apiFetch<PluginStatus[]>(`/namespaces/${ns}/applications/${name}/plugin-status`),

  /** GET /api/plugins/{name}/status — the plugin's targets, its load included. */
  forPlugin: (name: string) =>
    apiFetch<PluginStatus[]>(`/plugins/${encodeURIComponent(name)}/status`),
};
