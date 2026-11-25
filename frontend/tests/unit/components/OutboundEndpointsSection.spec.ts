import { describe, expect, it } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";
import OutboundEndpointsSection from "@/components/settings/OutboundEndpointsSection.vue";
import type { EndpointSettings, OutboundStatus } from "@/types/api";
import { ENDPOINT_FIELDS, emptyEndpoints } from "@/utils/endpoints";
import { mockFetchOnce } from "../helpers/mockFetch";

const vuetify = createVuetify({ components, directives });

function outboundStatus(overrides: Partial<OutboundStatus> = {}): OutboundStatus {
  return {
    allow_insecure_endpoints: false,
    extra_ca: null,
    endpoints: ENDPOINT_FIELDS.map((f) => ({
      id: f.id,
      setting: f.setting,
      label: f.label,
      default: f.defaultUrl,
      value: "",
      effective: f.defaultUrl,
      host: f.defaultUrl.replace("https://", ""),
      error: null,
    })),
    credentials: [],
    ...overrides,
  };
}

function mountSection(initial: EndpointSettings = emptyEndpoints()) {
  const wrapper = mount(OutboundEndpointsSection, {
    props: {
      modelValue: initial,
      "onUpdate:modelValue": (value: EndpointSettings) => wrapper.setProps({ modelValue: value }),
    },
    global: { plugins: [vuetify] },
  });
  return wrapper;
}

function field(wrapper: ReturnType<typeof mountSection>, setting: keyof EndpointSettings) {
  return wrapper.find(`[data-testid="endpoint-${setting}"]`);
}

describe("OutboundEndpointsSection", () => {
  it("loads the outbound status and shows the defaults as placeholders", async () => {
    const fetchMock = mockFetchOnce({ body: outboundStatus() });
    const wrapper = mountSection();
    await flushPromises();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0][0]).toBe("/api/settings/outbound");

    for (const f of ENDPOINT_FIELDS) {
      const input = field(wrapper, f.setting).find("input");
      expect(input.exists(), f.setting).toBe(true);
      expect(input.attributes("placeholder")).toBe(f.defaultUrl);
    }
    expect(wrapper.text()).toContain("Anthropic API base URL");
    expect(wrapper.text()).toContain("GitHub API base URL");
  });

  it("reports the deployment's TLS and endpoint policy", async () => {
    mockFetchOnce({
      body: outboundStatus({
        allow_insecure_endpoints: true,
        extra_ca: { file: "/etc/deckwatch/extra-ca/ca.crt", certificates: 2 },
      }),
    });
    const wrapper = mountSection();
    await flushPromises();

    expect(wrapper.find('[data-testid="outbound-policy"]').text()).toContain(
      "DECKWATCH_ALLOW_INSECURE_ENDPOINTS",
    );
    const ca = wrapper.find('[data-testid="outbound-extra-ca"]').text();
    expect(ca).toContain("2 extra CA certificates");
    expect(ca).toContain("/etc/deckwatch/extra-ca/ca.crt");
  });

  it("defaults to https-only and the system trust store", async () => {
    mockFetchOnce({ body: outboundStatus() });
    const wrapper = mountSection();
    await flushPromises();

    expect(wrapper.find('[data-testid="outbound-policy"]').text()).toContain("https:// only");
    expect(wrapper.find('[data-testid="outbound-extra-ca"]').text()).toContain(
      "System trust store",
    );
    expect(wrapper.find('[data-testid="credential-bindings-empty"]').exists()).toBe(true);
  });

  it("emits edits and shows validation messages while typing", async () => {
    mockFetchOnce({ body: outboundStatus() });
    const wrapper = mountSection();
    await flushPromises();

    await field(wrapper, "anthropic_api_base").find("input").setValue("http://gateway.example.com");
    await flushPromises();

    const emitted = wrapper.emitted("update:modelValue");
    expect(emitted?.at(-1)?.[0]).toMatchObject({ anthropic_api_base: "http://gateway.example.com" });
    expect(field(wrapper, "anthropic_api_base").text()).toContain(
      "http:// is only allowed for localhost",
    );

    await field(wrapper, "anthropic_api_base").find("input").setValue("https://gateway.example.com");
    await flushPromises();
    expect(field(wrapper, "anthropic_api_base").text()).not.toContain("only allowed");
  });

  it("stores an empty string, not null, when a field is cleared", async () => {
    mockFetchOnce({ body: outboundStatus() });
    const wrapper = mountSection({ ...emptyEndpoints(), github_api_base: "https://ghe.example.com/api/v3" });
    await flushPromises();

    const textField = wrapper
      .findAllComponents(components.VTextField)
      .find((c) => c.props("label") === "GitHub API base URL");
    expect(textField?.props("modelValue")).toBe("https://ghe.example.com/api/v3");
    textField?.vm.$emit("update:modelValue", null);
    await flushPromises();

    expect(wrapper.emitted("update:modelValue")?.at(-1)?.[0]).toMatchObject({ github_api_base: "" });
  });

  it("shows where each stored credential may be sent", async () => {
    mockFetchOnce({
      body: outboundStatus({
        credentials: [
          {
            credential: "anthropic_api_key",
            label: "Anthropic API key",
            usable: false,
            bindings: [
              {
                endpoint: "anthropic_api",
                bound_host: "api.anthropic.com",
                configured_host: "gateway.example.com",
                matches: false,
              },
            ],
          },
          {
            credential: "git_token:ghe",
            label: "git token 'ghe'",
            usable: true,
            bindings: [
              { endpoint: "github_api", bound_host: "ghe.example.com", configured_host: "ghe.example.com", matches: true },
              { endpoint: "github_web", bound_host: "ghe.example.com", configured_host: "ghe.example.com", matches: true },
            ],
          },
        ],
      }),
    });
    const wrapper = mountSection();
    await flushPromises();

    const key = wrapper.find('[data-testid="binding-anthropic_api_key"]').text();
    expect(key).toContain("Anthropic API key saved for api.anthropic.com");
    expect(key).toContain("now points to gateway.example.com");
    expect(key).toContain("re-enter");

    const token = wrapper.find('[data-testid="binding-git_token:ghe"]').text();
    expect(token).toContain("git token 'ghe' saved for ghe.example.com");
    expect(token).not.toContain("re-enter");
  });

  it("shows server-side endpoint errors", async () => {
    const status = outboundStatus();
    status.endpoints[0].error = "Anthropic API endpoint: http:// is only allowed for localhost";
    mockFetchOnce({ body: status });
    const wrapper = mountSection({ ...emptyEndpoints(), anthropic_api_base: "http://gateway.example.com" });
    await flushPromises();

    expect(field(wrapper, "anthropic_api_base").text()).toContain("only allowed for localhost");
  });

  it("keeps working when the status cannot be loaded", async () => {
    mockFetchOnce({ status: 500, body: { error: "internal", message: "boom" } });
    const wrapper = mountSection();
    await flushPromises();

    expect(wrapper.find('[data-testid="outbound-status-error"]').text()).toContain("boom");
    // Fields still render with the compiled-in defaults.
    expect(field(wrapper, "anthropic_api_base").find("input").attributes("placeholder")).toBe(
      "https://api.anthropic.com",
    );
  });

  it("refresh() reloads the status", async () => {
    const fetchMock = mockFetchOnce({ body: outboundStatus() });
    const wrapper = mountSection();
    await flushPromises();
    fetchMock.mockResolvedValueOnce(
      new Response(JSON.stringify(outboundStatus({ allow_insecure_endpoints: true })), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );

    await (wrapper.vm as unknown as { refresh: () => Promise<void> }).refresh();
    await flushPromises();

    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(wrapper.find('[data-testid="outbound-policy"]').text()).toContain(
      "DECKWATCH_ALLOW_INSECURE_ENDPOINTS",
    );
  });
});
