// Helpers for the application's Observability section: the OpenTelemetry
// injection form and the workloads table. Validation mirrors
// src/otel_inject.rs so operators get feedback while typing; the server
// validates again on save and is the authority.
import type {
  InjectedWorkloadKind,
  InjectionState,
  OtelInjectionSettings,
  ResourceAttribute,
  WorkloadInjection,
} from "@/types/api";

export const MAX_SERVICE_NAME_CHARS = 255;
export const MAX_RESOURCE_ATTRIBUTES = 32;
const MAX_KEY_LENGTH = 255;
const MAX_VALUE_LENGTH = 1024;

/** Resource attributes deckwatch sets itself; extra attributes can't override them. */
export const MANAGED_ATTRIBUTES = [
  "k8s.namespace.name",
  "k8s.deployment.name",
  "k8s.statefulset.name",
  "k8s.cronjob.name",
  "deckwatch.application.name",
];

/** An error for the service name override, or null when it's fine (empty = unset). */
export function validateServiceName(raw: string): string | null {
  const name = raw.trim();
  if (name === "") return null;
  if ([...name].length > MAX_SERVICE_NAME_CHARS) {
    return `At most ${MAX_SERVICE_NAME_CHARS} characters.`;
  }
  // Rust's char::is_control: the C0 and C1 control characters.
  if (/[\u0000-\u001f\u007f-\u009f]/.test(name)) return "No control characters.";
  return null;
}

/** A resource attribute key: letters, digits, ".", "_" and "-". */
export function isAttributeKey(key: string): boolean {
  return key.length > 0 && key.length <= MAX_KEY_LENGTH && /^[A-Za-z0-9._-]+$/.test(key);
}

/**
 * A percent-encoded attribute value: W3C baggage-octets only (printable ASCII
 * except space, `"`, `,`, `;` and `\`), and every `%` starts an escape.
 */
export function isEncodedAttributeValue(value: string): boolean {
  if (value.length === 0 || value.length > MAX_VALUE_LENGTH) return false;
  if (!/^[\x21\x23-\x2b\x2d-\x3a\x3c-\x5b\x5d-\x7e]+$/.test(value)) return false;
  return !/%(?![0-9A-Fa-f]{2})/.test(value);
}

export interface ParsedAttributes {
  attributes: ResourceAttribute[];
  /** The first problem, or null. */
  error: string | null;
}

/** Parse the attributes field: `key=value` pairs separated by commas. */
export function parseResourceAttributes(text: string): ParsedAttributes {
  const attributes: ResourceAttribute[] = [];
  const fail = (error: string): ParsedAttributes => ({ attributes, error });
  const pairs = text
    .split(",")
    .map((p) => p.trim())
    .filter((p) => p !== "");
  for (const pair of pairs) {
    const eq = pair.indexOf("=");
    if (eq < 0) return fail(`"${pair}" isn't key=value.`);
    const key = pair.slice(0, eq).trim();
    const value = pair.slice(eq + 1).trim();
    if (!isAttributeKey(key)) {
      return fail(`The key "${key}" can only use letters, digits, ".", "_" and "-".`);
    }
    if (MANAGED_ATTRIBUTES.includes(key)) return fail(`${key} is set by deckwatch.`);
    if (key === "service.name") return fail("Set service.name with the service name above.");
    if (!isEncodedAttributeValue(value)) {
      return fail(
        `Percent-encode the value of ${key}: no spaces, quotes, commas, semicolons, ` +
          "backslashes or non-ASCII characters, and every % starts an escape such as %20.",
      );
    }
    if (attributes.some((a) => a.key === key)) return fail(`${key} is repeated.`);
    attributes.push({ key, value });
  }
  if (attributes.length > MAX_RESOURCE_ATTRIBUTES) {
    return fail(`At most ${MAX_RESOURCE_ATTRIBUTES} attributes.`);
  }
  return { attributes, error: null };
}

/** The attributes as the field shows them. */
export function formatResourceAttributes(attributes: ResourceAttribute[]): string {
  return attributes.map((a) => `${a.key}=${a.value}`).join(", ");
}

export type InjectionChange = "enable" | "disable" | "update";

/** What saving `after` over `before` does, or null if nothing changes. */
export function injectionChange(
  before: OtelInjectionSettings,
  after: OtelInjectionSettings,
): InjectionChange | null {
  if (!before.enabled && after.enabled) return "enable";
  if (before.enabled && !after.enabled) return "disable";
  const same =
    (before.service_name ?? null) === (after.service_name ?? null) &&
    formatResourceAttributes(before.resource_attributes) ===
      formatResourceAttributes(after.resource_attributes);
  return same ? null : "update";
}

function counted(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** "2 Deployments, 1 StatefulSet and 1 CronJob", or "" when there are none. */
export function describeWorkloads(workloads: { kind: InjectedWorkloadKind }[]): string {
  const count = (kind: InjectedWorkloadKind) => workloads.filter((w) => w.kind === kind).length;
  const parts = [
    [count("Deployment"), "Deployment", "Deployments"],
    [count("StatefulSet"), "StatefulSet", "StatefulSets"],
    [count("CronJob"), "CronJob", "CronJobs"],
  ]
    .filter(([n]) => (n as number) > 0)
    .map(([n, one, many]) => counted(n as number, one as string, many as string));
  if (parts.length <= 1) return parts.join("");
  return `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}`;
}

export interface ConfirmCopy {
  title: string;
  message: string;
  confirmText: string;
}

/**
 * The confirmation shown before a save that restarts pods, or null when the
 * save restarts nothing (settings changed while injection stays off).
 */
export function confirmCopy(
  change: InjectionChange,
  enabledAfter: boolean,
  workloads: { kind: InjectedWorkloadKind }[],
  hasEndpoint: boolean,
): ConfirmCopy | null {
  const list = describeWorkloads(workloads);
  const rolling = workloads.some((w) => w.kind !== "CronJob");
  const cronjobs = workloads.some((w) => w.kind === "CronJob");
  // Deployments and StatefulSets roll their pods; a CronJob's next Job just
  // starts with the new environment.
  const restart = rolling
    ? "Their pods restart once, as a rolling update" +
      (cronjobs ? "; CronJobs pick up the change on their next run." : ".")
    : cronjobs
      ? "CronJobs pick up the change on their next run."
      : "";
  switch (change) {
    case "enable": {
      let message: string;
      if (!hasEndpoint) {
        message =
          "No OTLP endpoint is set in Settings → Observability, so nothing is injected yet. " +
          "Once one is, the application's pods restart once to pick up the variables.";
      } else if (list === "") {
        message =
          "The application has no Deployments, StatefulSets or CronJobs yet. They get the " +
          "OTEL_* variables when they're added.";
      } else {
        message = `deckwatch adds the OTEL_* variables to the primary container of ${list}. ${restart}`;
      }
      return { title: "Turn on OpenTelemetry injection?", message, confirmText: "Turn on" };
    }
    case "disable":
      return {
        title: "Turn off OpenTelemetry injection?",
        message:
          "deckwatch removes the variables it added; values your workloads set themselves " +
          `stay. ${restart}`.trim(),
        confirmText: "Turn off",
      };
    case "update":
      if (!enabledAfter) return null;
      return {
        title: "Save the OpenTelemetry settings?",
        message:
          "Workloads whose variables change get the new values, and their pods restart once, " +
          "as a rolling update.",
        confirmText: "Save",
      };
  }
}

/** The label of a workload's injection state. */
export function injectionStateLabel(w: WorkloadInjection, enabled: boolean): string {
  switch (w.state) {
    case "injected":
      return `Injected (${w.injected.length})`;
    case "pending":
      return enabled ? "Applying" : "Removing";
    case "foreign":
      return "Another deckwatch";
    default:
      return "Off";
  }
}

export function injectionStateColor(state: InjectionState): string | undefined {
  switch (state) {
    case "injected":
      return "success";
    case "pending":
      return "info";
    case "foreign":
      return "warning";
    default:
      return undefined;
  }
}
