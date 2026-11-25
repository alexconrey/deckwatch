// Settings → MCP Tuning → Kubernetes tool guardrails (mcp_tuning.guardrails).
// The backend validates and normalizes the values (src/handlers/mcp_guardrails.rs);
// this file holds the defaults and the rules the restrictive policy can relax.
import type { McpGuardrails } from "@/types/api";

/** The backend's defaults: secure, but apply_manifest on. */
export function defaultGuardrails(): McpGuardrails {
  return {
    workload_policy: "restrictive",
    exceptions: [],
    allowed_custom_resource_groups: [],
    allow_privileged_csr: false,
    apply_manifest_enabled: true,
    secret_decode_enabled: false,
    secure_defaults: true,
  };
}

/** `stored` with the defaults for anything missing (an older backend sends none). */
export function withGuardrailDefaults(stored?: Partial<McpGuardrails> | null): McpGuardrails {
  const defaults = defaultGuardrails();
  return {
    ...defaults,
    ...(stored ?? {}),
    exceptions: [...(stored?.exceptions ?? defaults.exceptions)],
    allowed_custom_resource_groups: [
      ...(stored?.allowed_custom_resource_groups ?? defaults.allowed_custom_resource_groups),
    ],
  };
}

export interface GuardrailException {
  /** mcp-k8s's WorkloadPolicy field, the value stored in `exceptions`. */
  field: string;
  /** What letting it through allows. */
  label: string;
}

/**
 * The rules of the restrictive policy an admin can let through, in the
 * backend's order (`EXCEPTIONS` in src/handlers/mcp_guardrails.rs).
 */
export const GUARDRAIL_EXCEPTIONS: readonly GuardrailException[] = [
  { field: "allow_privileged", label: "Privileged containers" },
  { field: "allow_privilege_escalation", label: "Privilege escalation" },
  { field: "allow_host_namespaces", label: "Host network, PID and IPC namespaces" },
  { field: "allow_host_path", label: "hostPath volumes" },
  { field: "allow_host_ports", label: "Host ports" },
  { field: "allow_unconfined", label: "Unconfined profiles, unsafe sysctls" },
  { field: "allow_node_port", label: "NodePort Services" },
  { field: "allow_load_balancer", label: "LoadBalancer Services" },
  { field: "allow_external_services", label: "ExternalName Services and external IPs" },
  { field: "allow_ingress_snippets", label: "Ingress snippet annotations" },
  { field: "allow_rbac_escalation", label: "RBAC escalation (bind, escalate, wildcards)" },
];

/**
 * `exceptions` with `field` added or removed: the known rules in
 * GUARDRAIL_EXCEPTIONS order, then any this page doesn't know, kept as they were.
 */
export function toggleException(exceptions: readonly string[], field: string, on: boolean): string[] {
  const set = new Set(exceptions);
  if (on) set.add(field);
  else set.delete(field);
  const known = GUARDRAIL_EXCEPTIONS.map((e) => e.field);
  return [...known.filter((f) => set.has(f)), ...[...set].filter((f) => !known.includes(f))];
}

const DNS_LABEL = /^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$/;

/**
 * Why `group` can't be an allowed custom resource API group, or null if it
 * can: "*" or a DNS subdomain such as cert-manager.io. (The backend also
 * refuses built-in groups such as apps.)
 */
export function validateApiGroup(group: string): string | null {
  const g = group.trim().toLowerCase();
  if (g === "*") return null;
  if (g.length > 253 || !g.split(".").every((label) => DNS_LABEL.test(label))) {
    return `"${group}" is not an API group name (a DNS subdomain such as cert-manager.io), or * for any group`;
  }
  return null;
}

/** The first problem with `g` to show before saving, or null. */
export function validateGuardrails(g: McpGuardrails): string | null {
  for (const group of g.allowed_custom_resource_groups) {
    const error = validateApiGroup(group);
    if (error) return `Allowed custom resource API groups: ${error}`;
  }
  return null;
}
