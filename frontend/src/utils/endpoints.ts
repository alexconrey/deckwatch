// Endpoint settings helpers for the Settings page. Validation mirrors
// src/outbound/endpoints.rs so operators get feedback while typing; the
// server validates again on save and is the authority.
import type { EndpointBinding, EndpointId, EndpointSettings } from "@/types/api";

export interface EndpointField {
  id: EndpointId;
  setting: keyof EndpointSettings;
  label: string;
  hint: string;
  group: "ai" | "github";
  /** Compiled-in default; GET /api/settings/outbound reports the same. */
  defaultUrl: string;
  /** `{region}` may appear in the host. */
  allowsRegion: boolean;
}

export const REGION_PLACEHOLDER = "{region}";
const REGION_SENTINEL = "deckwatch-region-placeholder";
const MAX_LENGTH = 2048;

export const ENDPOINT_FIELDS: EndpointField[] = [
  {
    id: "anthropic_api",
    setting: "anthropic_api_base",
    label: "Anthropic API base URL",
    hint: "/v1/messages is appended. Use for an LLM gateway or proxy.",
    group: "ai",
    defaultUrl: "https://api.anthropic.com",
    allowsRegion: false,
  },
  {
    id: "vertex_token",
    setting: "vertex_token_url",
    label: "Vertex AI token URL",
    hint: "OAuth token endpoint for the service account key (full URL).",
    group: "ai",
    defaultUrl: "https://oauth2.googleapis.com/token",
    allowsRegion: false,
  },
  {
    id: "vertex_api",
    setting: "vertex_api_base",
    label: "Vertex AI API base URL",
    hint: "{region} in the host is replaced with the provider region.",
    group: "ai",
    defaultUrl: "https://{region}-aiplatform.googleapis.com",
    allowsRegion: true,
  },
  {
    id: "bedrock",
    setting: "bedrock_endpoint",
    label: "Bedrock runtime endpoint",
    hint: "For a VPC or FIPS endpoint. AWS credentials are only sent to AWS hosts.",
    group: "ai",
    defaultUrl: "https://bedrock-runtime.{region}.amazonaws.com",
    allowsRegion: true,
  },
  {
    id: "github_api",
    setting: "github_api_base",
    label: "GitHub API base URL",
    hint: "GitHub Enterprise Server: https://<host>/api/v3",
    group: "github",
    defaultUrl: "https://api.github.com",
    allowsRegion: false,
  },
  {
    id: "github_web",
    setting: "github_web_base",
    label: "GitHub web URL",
    hint: "Identifies GitHub repositories and serves plugin release downloads.",
    group: "github",
    defaultUrl: "https://github.com",
    allowsRegion: false,
  },
  {
    id: "github_raw",
    setting: "github_raw_base",
    label: "GitHub raw content URL",
    hint: "Plugin file downloads. GitHub Enterprise Server: https://<host>/raw",
    group: "github",
    defaultUrl: "https://raw.githubusercontent.com",
    allowsRegion: false,
  },
];

export function emptyEndpoints(): EndpointSettings {
  return {
    anthropic_api_base: "",
    vertex_token_url: "",
    vertex_api_base: "",
    bedrock_endpoint: "",
    github_api_base: "",
    github_web_base: "",
    github_raw_base: "",
  };
}

export function fieldFor(id: EndpointId): EndpointField {
  const field = ENDPOINT_FIELDS.find((f) => f.id === id);
  if (!field) throw new Error(`unknown endpoint ${id}`);
  return field;
}

function isLoopback(hostname: string): boolean {
  const host = hostname.toLowerCase();
  if (host === "localhost" || host === "[::1]") return true;
  const octets = host.split(".");
  return (
    octets.length === 4 &&
    octets[0] === "127" &&
    octets.every((o) => /^\d{1,3}$/.test(o) && Number(o) <= 255)
  );
}

/**
 * Validate an endpoint override. Returns an error message, or null when the
 * value is acceptable (empty means "use the default").
 */
export function validateEndpoint(
  field: EndpointField,
  raw: string,
  allowInsecure: boolean,
): string | null {
  const value = raw.trim();
  if (value === "") return null;
  if (value.length > MAX_LENGTH) return `Longer than ${MAX_LENGTH} characters.`;

  const placeholders = value.split(REGION_PLACEHOLDER).length - 1;
  if (placeholders > 0 && !field.allowsRegion) {
    return "{region} is only supported for the Vertex AI and Bedrock endpoints.";
  }
  if (placeholders > 1) return "{region} may appear only once.";

  const parseable = value.split(REGION_PLACEHOLDER).join(REGION_SENTINEL);
  let url: URL;
  try {
    url = new URL(parseable);
  } catch {
    return "Not a valid URL. Include the scheme, e.g. https://gateway.example.com";
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    return `Use https:// (got ${url.protocol}//).`;
  }
  if (url.username || url.password) {
    return "Must not contain a username or password.";
  }
  if (url.search || url.hash || /[?#]/.test(parseable)) {
    return "Must not contain a query string or fragment.";
  }
  if (!url.hostname) return "A host is required.";
  if (placeholders === 1 && !url.hostname.includes(REGION_SENTINEL)) {
    return "{region} is only allowed in the host name.";
  }
  if (url.protocol === "http:" && !isLoopback(url.hostname) && !allowInsecure) {
    return "http:// is only allowed for localhost. Use https://, or enable DECKWATCH_ALLOW_INSECURE_ENDPOINTS on the deployment.";
  }
  return null;
}

/**
 * Origin (`scheme://host[:port]`, `{region}` kept) of the effective endpoint:
 * the override when set, else the default. Null when the override is invalid.
 */
export function effectiveOrigin(field: EndpointField, settings: EndpointSettings): string | null {
  const value = settings[field.setting].trim() || field.defaultUrl;
  try {
    const url = new URL(value.split(REGION_PLACEHOLDER).join(REGION_SENTINEL));
    return `${url.protocol}//${url.host}`.split(REGION_SENTINEL).join(REGION_PLACEHOLDER);
  } catch {
    return null;
  }
}

/** `host[:port]` for https origins, the full origin otherwise. */
export function displayOrigin(origin: string): string {
  return origin.startsWith("https://") ? origin.slice("https://".length) : origin;
}

/**
 * Compare a stored credential's binding with the current endpoint. A missing
 * binding means the credential predates bindings and is bound to the default.
 */
export function bindingState(
  field: EndpointField,
  binding: EndpointBinding | null | undefined,
  settings: EndpointSettings,
): { boundHost: string; configuredHost: string; matches: boolean } {
  const bound =
    binding?.[field.id] ?? effectiveOrigin(field, emptyEndpoints()) ?? field.defaultUrl;
  const configured = effectiveOrigin(field, settings) ?? "";
  return {
    boundHost: displayOrigin(bound),
    configuredHost: displayOrigin(configured),
    matches: bound === configured,
  };
}
