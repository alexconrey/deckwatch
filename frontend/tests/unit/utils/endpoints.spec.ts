import { describe, expect, it } from "vitest";
import {
  ENDPOINT_FIELDS,
  bindingState,
  displayOrigin,
  effectiveOrigin,
  emptyEndpoints,
  fieldFor,
  validateEndpoint,
} from "@/utils/endpoints";

const anthropic = fieldFor("anthropic_api");
const vertexApi = fieldFor("vertex_api");

describe("ENDPOINT_FIELDS", () => {
  it("covers every endpoint setting exactly once", () => {
    const settings = Object.keys(emptyEndpoints()).sort();
    expect(ENDPOINT_FIELDS.map((f) => f.setting).sort()).toEqual(settings);
  });

  it("has defaults that pass validation", () => {
    for (const field of ENDPOINT_FIELDS) {
      expect(validateEndpoint(field, field.defaultUrl, false)).toBeNull();
    }
  });
});

describe("validateEndpoint", () => {
  it("accepts empty (default) and https URLs with a path", () => {
    expect(validateEndpoint(anthropic, "", false)).toBeNull();
    expect(validateEndpoint(anthropic, "   ", false)).toBeNull();
    expect(validateEndpoint(anthropic, "https://gateway.example.com/anthropic/", false)).toBeNull();
    expect(validateEndpoint(anthropic, "https://gateway.example.com:8443", false)).toBeNull();
  });

  it("rejects http for remote hosts unless insecure endpoints are allowed", () => {
    const msg = validateEndpoint(anthropic, "http://gateway.example.com", false);
    expect(msg).toContain("http:// is only allowed for localhost");
    expect(msg).toContain("DECKWATCH_ALLOW_INSECURE_ENDPOINTS");
    expect(validateEndpoint(anthropic, "http://gateway.example.com", true)).toBeNull();
  });

  it("accepts http for loopback hosts", () => {
    for (const url of ["http://localhost:8080", "http://127.0.0.1:9000", "http://[::1]:8443"]) {
      expect(validateEndpoint(anthropic, url, false)).toBeNull();
    }
    expect(validateEndpoint(anthropic, "http://localhost.example.com", false)).not.toBeNull();
    expect(validateEndpoint(anthropic, "http://127.0.0.1.nip.io", false)).not.toBeNull();
  });

  it("rejects userinfo, query strings and fragments", () => {
    expect(validateEndpoint(anthropic, "https://user:pass@gateway.example.com", false)).toContain(
      "username or password",
    );
    expect(validateEndpoint(anthropic, "https://gateway.example.com/?key=1", false)).toContain(
      "query string",
    );
    expect(validateEndpoint(anthropic, "https://gateway.example.com/#x", false)).toContain(
      "query string",
    );
  });

  it("rejects values that are not absolute http(s) URLs", () => {
    expect(validateEndpoint(anthropic, "gateway.example.com", false)).toContain("Not a valid URL");
    expect(validateEndpoint(anthropic, "ftp://gateway.example.com", false)).toContain("https://");
  });

  it("allows {region} once, in the host, for Vertex AI and Bedrock only", () => {
    expect(validateEndpoint(vertexApi, "https://{region}-aiplatform.example.com", false)).toBeNull();
    expect(validateEndpoint(anthropic, "https://{region}.example.com", false)).toContain(
      "only supported",
    );
    expect(validateEndpoint(vertexApi, "https://example.com/{region}", false)).toContain(
      "host name",
    );
    expect(validateEndpoint(vertexApi, "https://{region}.{region}.example.com", false)).toContain(
      "only once",
    );
  });
});

describe("origins and bindings", () => {
  it("computes the effective origin from the override or the default", () => {
    const settings = emptyEndpoints();
    expect(effectiveOrigin(anthropic, settings)).toBe("https://api.anthropic.com");
    settings.anthropic_api_base = "https://Gateway.example.com:443/anthropic";
    expect(effectiveOrigin(anthropic, settings)).toBe("https://gateway.example.com");
    expect(effectiveOrigin(vertexApi, emptyEndpoints())).toBe(
      "https://{region}-aiplatform.googleapis.com",
    );
    settings.anthropic_api_base = "not a url";
    expect(effectiveOrigin(anthropic, settings)).toBeNull();
  });

  it("shows https origins as bare hosts", () => {
    expect(displayOrigin("https://api.anthropic.com")).toBe("api.anthropic.com");
    expect(displayOrigin("http://localhost:8080")).toBe("http://localhost:8080");
  });

  it("treats a missing binding as bound to the default", () => {
    const settings = emptyEndpoints();
    expect(bindingState(anthropic, null, settings)).toEqual({
      boundHost: "api.anthropic.com",
      configuredHost: "api.anthropic.com",
      matches: true,
    });
    settings.anthropic_api_base = "https://gateway.example.com";
    expect(bindingState(anthropic, undefined, settings)).toEqual({
      boundHost: "api.anthropic.com",
      configuredHost: "gateway.example.com",
      matches: false,
    });
  });

  it("compares a recorded binding with the configured endpoint", () => {
    const settings = { ...emptyEndpoints(), anthropic_api_base: "https://gateway.example.com/v1-proxy" };
    const binding = { anthropic_api: "https://gateway.example.com" };
    expect(bindingState(anthropic, binding, settings).matches).toBe(true);
    settings.anthropic_api_base = "https://gateway.example.com:8443";
    expect(bindingState(anthropic, binding, settings)).toEqual({
      boundHost: "gateway.example.com",
      configuredHost: "gateway.example.com:8443",
      matches: false,
    });
  });
});
