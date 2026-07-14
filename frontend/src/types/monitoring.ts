// Types for Prometheus PodMonitor toggle (docs/PROMETHEUS_INTEGRATION.md).
// Kept in a separate file (not folded into api.ts) so the monitoring feature
// stays a self-contained slice while the design is still evolving.

export interface MonitorSettings {
  enabled: boolean;
  name: string;
  namespace: string;
  port: string;
  path: string;
  interval: string;
  matching_pods: number;
  /** Populated when the prometheus-operator CRD is not installed. When set,
   *  the other fields describe the *requested* config, not what was applied. */
  unavailable_reason?: string | null;
}

export interface MonitorConfigRequest {
  enabled: boolean;
  port?: string;
  path?: string;
  interval?: string;
}
