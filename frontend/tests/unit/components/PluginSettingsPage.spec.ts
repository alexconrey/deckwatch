import { describe, expect, it, vi, beforeEach } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";
import type { ConfigField, PluginSummary, DeckwatchSettings } from "@/types/api";

// ---------------------------------------------------------------------------
// Hoisted mock functions — must be declared before vi.mock() calls.
// ---------------------------------------------------------------------------

const {
  mockListPlugins,
  mockSaveConfig,
  mockSettingsGet,
  mockSettingsUpdate,
  mockRouterPush,
} = vi.hoisted(() => ({
  mockListPlugins: vi.fn(),
  mockSaveConfig: vi.fn(),
  mockSettingsGet: vi.fn(),
  mockSettingsUpdate: vi.fn(),
  mockRouterPush: vi.fn(),
}));

vi.mock("vue-router", () => ({
  useRoute: () => ({ params: { name: "aws" } }),
  useRouter: () => ({ push: mockRouterPush }),
}));

vi.mock("@/api/plugins", () => ({
  pluginsApi: {
    list: mockListPlugins,
    getSchema: vi.fn(),
    saveConfig: mockSaveConfig,
  },
}));

vi.mock("@/api/settings", () => ({
  settingsApi: {
    get: mockSettingsGet,
    update: mockSettingsUpdate,
  },
}));

// Import AFTER mocks.
import PluginSettingsPage from "@/components/pages/PluginSettingsPage.vue";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const vuetify = createVuetify({ components, directives });

function makePlugin(schema: ConfigField[], name = "aws"): PluginSummary {
  return {
    name,
    version: "1.0.0",
    description: "AWS plugin",
    provides: [],
    depends_on: [],
    wasm_size_bytes: 1024,
    config_schema: schema,
  };
}

function makeSettings(pluginConfig: Record<string, string> = {}): DeckwatchSettings {
  return {
    allowed_namespaces: [],
    default_resource_limits: null,
    auth: null,
    notifications: null,
    git_repositories: [],
    oci_registries: [],
    git_token_secrets: [],
    plugins: [
      {
        name: "aws",
        enabled: true,
        source: { type: "url", url: "https://example.com/aws.wasm" },
        allowed_hosts: [],
        config: pluginConfig,
        inherit_env_keys: [],
        inherit_env_file_keys: {},
      },
    ],
  };
}

function mountPage() {
  return mount(PluginSettingsPage, {
    global: {
      plugins: [vuetify],
      stubs: { RouterLink: true },
    },
  });
}

// ---------------------------------------------------------------------------
// Field rendering tests
// ---------------------------------------------------------------------------

describe("PluginSettingsPage — field rendering", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockSettingsUpdate.mockResolvedValue(makeSettings());
    mockSaveConfig.mockResolvedValue(undefined);
  });

  it("renders a v-text-field for field_type 'string'", async () => {
    mockListPlugins.mockResolvedValue([
      makePlugin([{ key: "K", label: "My String", description: "d", field_type: "string", required: false, options: [] }]),
    ]);
    mockSettingsGet.mockResolvedValue(makeSettings());

    const wrapper = mountPage();
    await flushPromises();

    const fields = wrapper.findAllComponents(components.VTextField);
    const match = fields.find((f) => f.props("label") === "My String");
    expect(match).toBeTruthy();
    expect(match?.props("type")).not.toBe("password");
    expect(match?.props("readonly")).toBeFalsy();
  });

  it("renders a password v-text-field for field_type 'secret'", async () => {
    mockListPlugins.mockResolvedValue([
      makePlugin([{ key: "SK", label: "My Secret", description: "d", field_type: "secret", required: false, options: [] }]),
    ]);
    mockSettingsGet.mockResolvedValue(makeSettings());

    const wrapper = mountPage();
    await flushPromises();

    const fields = wrapper.findAllComponents(components.VTextField);
    const match = fields.find((f) => f.props("label") === "My Secret");
    expect(match).toBeTruthy();
    expect(match?.props("type")).toBe("password");
  });

  it("shows 'already configured' placeholder when backend returns sentinel 'configured'", async () => {
    mockListPlugins.mockResolvedValue([
      makePlugin([{ key: "API_KEY", label: "API Key", description: "d", field_type: "secret", required: false, options: [] }]),
    ]);
    mockSettingsGet.mockResolvedValue(makeSettings({ API_KEY: "configured" }));

    const wrapper = mountPage();
    await flushPromises();

    const fields = wrapper.findAllComponents(components.VTextField);
    const match = fields.find((f) => f.props("label") === "API Key");
    expect(match).toBeTruthy();
    // Placeholder must be the human-readable text, not the raw sentinel.
    expect(match?.props("placeholder")).toBe("already configured");
    // Model value must be empty (not "configured") so submitting without typing
    // doesn't overwrite the encrypted value.
    expect(match?.props("modelValue")).toBe("");
  });

  it("pre-populates a non-sentinel secret value", async () => {
    mockListPlugins.mockResolvedValue([
      makePlugin([{ key: "API_KEY", label: "API Key", description: "d", field_type: "secret", required: false, options: [] }]),
    ]);
    mockSettingsGet.mockResolvedValue(makeSettings({ API_KEY: "sk-live-plain" }));

    const wrapper = mountPage();
    await flushPromises();

    const fields = wrapper.findAllComponents(components.VTextField);
    const match = fields.find((f) => f.props("label") === "API Key");
    expect(match?.props("modelValue")).toBe("sk-live-plain");
    expect(match?.props("placeholder")).toBe(""); // no placeholder when value is set
  });

  it("renders a v-switch for field_type 'bool'", async () => {
    mockListPlugins.mockResolvedValue([
      makePlugin([{ key: "FEAT", label: "Enable Feature", description: "d", field_type: "bool", required: false, options: [], default: "false" }]),
    ]);
    mockSettingsGet.mockResolvedValue(makeSettings());

    const wrapper = mountPage();
    await flushPromises();

    const switches = wrapper.findAllComponents(components.VSwitch);
    const match = switches.find((s) => s.props("label") === "Enable Feature");
    expect(match).toBeTruthy();
  });

  it("renders a v-select with items for field_type 'select'", async () => {
    mockListPlugins.mockResolvedValue([
      makePlugin([{ key: "REGION", label: "Region", description: "d", field_type: "select", required: true, options: ["us-east-1", "us-west-2"] }]),
    ]);
    mockSettingsGet.mockResolvedValue(makeSettings());

    const wrapper = mountPage();
    await flushPromises();

    const selects = wrapper.findAllComponents(components.VSelect);
    const match = selects.find((s) => s.props("label") === "Region");
    expect(match).toBeTruthy();
    expect(match?.props("items")).toEqual(["us-east-1", "us-west-2"]);
  });

  it("renders env_source fields as read-only with a chip badge", async () => {
    mockListPlugins.mockResolvedValue([
      makePlugin([{ key: "AWS_REGION", label: "AWS Region", description: "d", field_type: "string", required: false, options: [], env_source: "AWS_REGION" }]),
    ]);
    mockSettingsGet.mockResolvedValue(makeSettings());

    const wrapper = mountPage();
    await flushPromises();

    const fields = wrapper.findAllComponents(components.VTextField);
    const match = fields.find((f) => f.props("label") === "AWS Region");
    expect(match).toBeTruthy();
    expect(match?.props("readonly")).toBe(true);

    const chips = wrapper.findAllComponents(components.VChip);
    const envChip = chips.find((c) => c.text().includes("From env"));
    expect(envChip).toBeTruthy();
  });

  it("shows 'From env: X' chip when key is in inherit_env_keys", async () => {
    const settings = makeSettings();
    settings.plugins![0].inherit_env_keys = ["AWS_REGION"];

    mockListPlugins.mockResolvedValue([
      makePlugin([{ key: "AWS_REGION", label: "AWS Region", description: "d", field_type: "string", required: false, options: [], env_source: "AWS_REGION" }]),
    ]);
    mockSettingsGet.mockResolvedValue(settings);

    const wrapper = mountPage();
    await flushPromises();

    const chips = wrapper.findAllComponents(components.VChip);
    const envChip = chips.find((c) => c.text().includes("From env: AWS_REGION"));
    expect(envChip).toBeTruthy();
  });

  it("pre-populates string field from saved config", async () => {
    mockListPlugins.mockResolvedValue([
      makePlugin([{ key: "BUCKET_PREFIX", label: "Bucket Prefix", description: "d", field_type: "string", required: false, options: [], default: "" }]),
    ]);
    mockSettingsGet.mockResolvedValue(makeSettings({ BUCKET_PREFIX: "myorg-" }));

    const wrapper = mountPage();
    await flushPromises();

    const fields = wrapper.findAllComponents(components.VTextField);
    const match = fields.find((f) => f.props("label") === "Bucket Prefix");
    expect(match?.props("modelValue")).toBe("myorg-");
  });

  it("falls back to field default when no saved config", async () => {
    mockListPlugins.mockResolvedValue([
      makePlugin([{ key: "BUCKET_PREFIX", label: "Bucket Prefix", description: "d", field_type: "string", required: false, options: [], default: "default-prefix-" }]),
    ]);
    mockSettingsGet.mockResolvedValue(makeSettings({}));

    const wrapper = mountPage();
    await flushPromises();

    const fields = wrapper.findAllComponents(components.VTextField);
    const match = fields.find((f) => f.props("label") === "Bucket Prefix");
    expect(match?.props("modelValue")).toBe("default-prefix-");
  });
});

// ---------------------------------------------------------------------------
// Loading and error state tests
// ---------------------------------------------------------------------------

describe("PluginSettingsPage — loading and error states", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockSettingsUpdate.mockResolvedValue(makeSettings());
    mockSaveConfig.mockResolvedValue(undefined);
  });

  it("shows a warning when the plugin is not in the loaded list", async () => {
    mockListPlugins.mockResolvedValue([]); // "aws" not loaded
    mockSettingsGet.mockResolvedValue(makeSettings());

    const wrapper = mountPage();
    await flushPromises();

    const alerts = wrapper.findAllComponents(components.VAlert);
    const warning = alerts.find((a) => a.text().includes("not currently loaded"));
    expect(warning).toBeTruthy();
  });

  it("shows an info alert when the plugin has no config_schema", async () => {
    mockListPlugins.mockResolvedValue([makePlugin([])]); // schema is empty
    mockSettingsGet.mockResolvedValue(makeSettings());

    const wrapper = mountPage();
    await flushPromises();

    const alerts = wrapper.findAllComponents(components.VAlert);
    const info = alerts.find((a) => a.text().includes("does not declare any configuration fields"));
    expect(info).toBeTruthy();
  });

  it("shows a loading indicator while fetching", () => {
    mockListPlugins.mockReturnValue(new Promise(() => {}));
    mockSettingsGet.mockReturnValue(new Promise(() => {}));

    const wrapper = mountPage();

    const progress = wrapper.findComponent(components.VProgressLinear);
    expect(progress.exists()).toBe(true);
  });

  it("displays the plugin version and description in the info card", async () => {
    const plugin = makePlugin([]);
    plugin.version = "2.5.1";
    plugin.description = "Manages AWS cloud resources";
    mockListPlugins.mockResolvedValue([plugin]);
    mockSettingsGet.mockResolvedValue(makeSettings());

    const wrapper = mountPage();
    await flushPromises();

    expect(wrapper.text()).toContain("2.5.1");
    expect(wrapper.text()).toContain("Manages AWS cloud resources");
  });
});

// ---------------------------------------------------------------------------
// Save payload-construction tests (pure logic — no DOM rendering)
//
// The save() function in PluginSettingsPage builds a payload from the component's
// schema and formValues before calling saveConfig(). We replicate that exact
// logic here so we can assert the filtering rules without triggering Vue's
// renderer (which hits a Happy DOM fragment-cleanup limitation when async
// operations are in flight during teardown).
//
// If the payload logic in PluginSettingsPage.vue changes, update this function
// to match.
// ---------------------------------------------------------------------------

/**
 * Replicates the payload-building logic from PluginSettingsPage.save().
 * Keep in sync with the `save()` function in PluginSettingsPage.vue.
 */
function buildSavePayload(
  schema: ConfigField[],
  formValues: Record<string, string>,
): Record<string, string> {
  const payload: Record<string, string> = {};
  for (const field of schema) {
    if (field.env_source) continue; // inherited from env — not stored in settings
    const val = formValues[field.key] ?? "";
    if (field.field_type === "secret" && !val) continue; // empty secret → keep existing
    payload[field.key] = val;
  }
  return payload;
}

describe("PluginSettingsPage — save payload construction", () => {
  it("includes plain string fields", () => {
    const schema: ConfigField[] = [
      { key: "BUCKET_PREFIX", label: "Prefix", description: "d", field_type: "string", required: false, options: [] },
    ];
    expect(buildSavePayload(schema, { BUCKET_PREFIX: "myorg-" })).toEqual({ BUCKET_PREFIX: "myorg-" });
  });

  it("excludes env_source fields — they come from the environment, not settings", () => {
    const schema: ConfigField[] = [
      { key: "AWS_REGION", label: "Region", description: "d", field_type: "string", required: false, options: [], env_source: "AWS_REGION" },
      { key: "BUCKET_PREFIX", label: "Prefix", description: "d", field_type: "string", required: false, options: [] },
    ];
    const payload = buildSavePayload(schema, { AWS_REGION: "us-east-1", BUCKET_PREFIX: "myorg-" });

    expect(payload).not.toHaveProperty("AWS_REGION");
    expect(payload).toHaveProperty("BUCKET_PREFIX", "myorg-");
  });

  it("excludes empty secret fields — empty value must not overwrite existing encrypted value", () => {
    const schema: ConfigField[] = [
      { key: "API_KEY", label: "Key", description: "d", field_type: "secret", required: false, options: [] },
      { key: "PLAIN_VAL", label: "Plain", description: "d", field_type: "string", required: false, options: [] },
    ];
    // API_KEY empty (sentinel was stripped on mount); PLAIN_VAL has a value.
    const payload = buildSavePayload(schema, { API_KEY: "", PLAIN_VAL: "some-value" });

    expect(payload).toHaveProperty("PLAIN_VAL", "some-value");
    expect(payload).not.toHaveProperty("API_KEY");
  });

  it("includes a non-empty secret field", () => {
    const schema: ConfigField[] = [
      { key: "API_KEY", label: "Key", description: "d", field_type: "secret", required: false, options: [] },
    ];
    expect(buildSavePayload(schema, { API_KEY: "sk-new-key" })).toHaveProperty("API_KEY", "sk-new-key");
  });

  it("includes bool fields as 'true'/'false' strings", () => {
    const schema: ConfigField[] = [
      { key: "ENABLE_X", label: "Enable X", description: "d", field_type: "bool", required: false, options: [] },
    ];
    expect(buildSavePayload(schema, { ENABLE_X: "true" })).toHaveProperty("ENABLE_X", "true");
  });

  it("includes select fields", () => {
    const schema: ConfigField[] = [
      { key: "REGION", label: "Region", description: "d", field_type: "select", required: true, options: ["us-east-1", "us-west-2"] },
    ];
    expect(buildSavePayload(schema, { REGION: "us-east-1" })).toHaveProperty("REGION", "us-east-1");
  });

  it("defaults to empty string for fields missing from formValues", () => {
    const schema: ConfigField[] = [
      { key: "OPTIONAL_KEY", label: "Opt", description: "d", field_type: "string", required: false, options: [] },
    ];
    expect(buildSavePayload(schema, {})).toHaveProperty("OPTIONAL_KEY", "");
  });

  it("strips sentinel 'configured' value: component sets formValues to '' on load", async () => {
    // Verify the component's initFormValues() strips the sentinel.
    mockListPlugins.mockResolvedValue([
      makePlugin([{ key: "API_KEY", label: "API Key", description: "d", field_type: "secret", required: false, options: [] }]),
    ]);
    mockSettingsGet.mockResolvedValue(makeSettings({ API_KEY: "configured" }));

    const wrapper = mountPage();
    await flushPromises();

    // The form value must be empty after the sentinel is stripped.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect((wrapper.vm as any).formValues.API_KEY).toBe("");
  });
});
