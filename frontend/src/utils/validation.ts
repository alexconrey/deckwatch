/**
 * Shared validation rules for Kubernetes resource names.
 *
 * Kubernetes names follow RFC 1123 DNS label rules:
 *   - lowercase letters, digits, and hyphens only
 *   - must start and end with an alphanumeric character
 *   - maximum 63 characters
 *
 * Underscores are explicitly not allowed and are a common source of confusing
 * API errors because Kubernetes rejects them without a clear explanation.
 */

const K8S_NAME_RE = /^[a-z0-9]([-a-z0-9]*[a-z0-9])?$/;

/** Vuetify-compatible rule functions for a required Kubernetes resource name. */
export const k8sNameRules: ((v: string) => true | string)[] = [
  (v) => !!v || "Required",
  (v) =>
    !v.includes("_")
      ? true
      : "Resource names cannot contain underscores — Kubernetes only allows lowercase letters, numbers, and hyphens",
  (v) => v.length <= 63 || "Name must be 63 characters or fewer",
  (v) =>
    !v || K8S_NAME_RE.test(v)
      ? true
      : "Use lowercase letters, digits, and hyphens only (no leading/trailing hyphen)",
];
