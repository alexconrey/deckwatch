import { describe, expect, it, vi, beforeEach } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";
import type { ConfigField, PluginSummary, DeckwatchSettings } from "@/types/api";

// ---------------------------------------------------------------------------
// Mocks — set up before importing the component so the module cache is warm.
// ---------------------------------------------------------------------------

const mockRouteParams = { name: "aws" };

vi.mock("vue-router", () => ({
  useRoute: () => ({ params: mockRouteParams }),
  useRouter: () => ({ push: vi.fn() }),
}));

const mockListPlugins = vi.fn();
const mockSaveConfig = vi.fn();
vi.mock("@/api/plugins", () => ({
  pluginsApi: {
    list: mockListPlugins,
    getSchema: vi.fn(),
    saveConfig: mockSaveConfig,
  },
}));

const mockSettingsGet = vi.fn();
const mockSettingsUpdate = vi.fn();
vi.mock("@/api/settings", () => ({
  settingsApi: {
    get: mockSettingsGet,
    update: mockSettingsUpdate,
  },
}));

// Import the component after the mocks are in place.
import PluginSettingsPage from "@/components/pages/PluginSettingsPage.vue";

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

const vuetify = createVuetify({ components, directives });

/** Build a PluginSummary with a custom config_schema. */
function makePlugin(schema: ConfigField[]): PluginSummary {
  return {
    name: "aws",
    version: "1.0.0",
    description: "AWS plugin",
    provides: [],
    depends_on: [],
    wasm_size_bytes: 1024,
    config_schema: schema,
  };
}

/** Build a minimal DeckwatchSettings with optional plugin config values. */
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
      stubs: {
        // Stub router-link to avoid "Cannot find component" warnings.
        RouterLink: true,
      },
    },
  });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("PluginSettingsPage field rendering", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockSettingsUpdate.mockResolvedValue(makeSettings());
    mockSaveConfig.mockResolvedValue(undefined);
  });

  it("renders a v-text-field for field_type 'string'", async () => {
    const schema: ConfigField[] = [
      {
        key: "MY_KEY",
        label: "My String",
        description: "A string value",
        field_type: "string",
        required: false,
        options: [],
      },
    ];
    mockListPlugins.mockResolvedValue([makePlugin(schema)]);
    mockSettingsGet.mockResolvedValue(makeSettings());

    const wrapper = mountPage();
    await flushPromises();

    // Should find a VTextField with the matching label.
    const textFields = wrapper.findAllComponents(components.VTextField);
    const match = textFields.find((f) => f.props("label") === "My String");
    expect(match).toBeTruthy();
    // 'string' type should NOT be a password field.
    expect(match?.props("type")).not.toBe("password");
  });

  it("renders a password v-text-field for field_type 'secret'", async () => {
    const schema: ConfigField[] = [
      {
        key: "MY_SECRET",
        label: "My Secret",
        description: "An encrypted value",
        field_type: "secret",
        required: false,
        options: [],
      },
    ];
    mockListPlugins.mockResolvedValue([makePlugin(schema)]);
    mockSettingsGet.mockResolvedValue(makeSettings());

    const wrapper = mountPage();
    await flushPromises();

    const textFields = wrapper.findAllComponents(components.VTextField);
    const match = textFields.find((f) => f.props("label") === "My Secret");
    expect(match).toBeTruthy();
    // Default is password type (eye toggle off).
    expect(match?.props("type")).toBe("password");
  });

  it("shows 'already configured' placeholder for a secret with sentinel value", async () => {
    const schema: ConfigField[] = [
      {
        key: "API_KEY",
        label: "API Key",
        description: "Secret key",
        field_type: "secret",
        required: false,
        options: [],
      },
    ];
    mockListPlugins.mockResolvedValue([makePlugin(schema)]);
    // Backend returns sentinel "configured" for the secret field.
    mockSettingsGet.mockResolvedValue(makeSettings({ API_KEY: "configured" }));

    const wrapper = mountPage();
    await flushPromises();

    const textFields = wrapper.findAllComponents(components.VTextField);
    const match = textFields.find((f) => f.props("label") === "API Key");
    expect(match).toBeTruthy();
    expect(match?.props("placeholder")).toBe("already configured");
    // The model value must be empty (not the sentinel string).
    expect(match?.props("modelValue")).toBe("");
  });

  it("renders a v-switch for field_type 'bool'", async () => {
    const schema: ConfigField[] = [
      {
        key: "ENABLE_FEATURE",
        label: "Enable Feature",
        description: "Toggle",
        field_type: "bool",
        required: false,
        options: [],
        default: "false",
      },
    ];
    mockListPlugins.mockResolvedValue([makePlugin(schema)]);
    mockSettingsGet.mockResolvedValue(makeSettings());

    const wrapper = mountPage();
    await flushPromises();

    const switches = wrapper.findAllComponents(components.VSwitch);
    const match = switches.find((s) => s.props("label") === "Enable Feature");
    expect(match).toBeTruthy();
  });

  it("renders a v-select for field_type 'select'", async () => {
    const schema: ConfigField[] = [
      {
        key: "REGION",
        label: "Region",
        description: "AWS region",
        field_type: "select",
        required: true,
        options: ["us-east-1", "us-west-2"],
      },
    ];
    mockListPlugins.mockResolvedValue([makePlugin(schema)]);
    mockSettingsGet.mockResolvedValue(makeSettings());

    const wrapper = mountPage();
    await flushPromises();

    const selects = wrapper.findAllComponents(components.VSelect);
    const match = selects.find((s) => s.props("label") === "Region");
    expect(match).toBeTruthy();
    expect(match?.props("items")).toEqual(["us-east-1", "us-west-2"]);
  });

  it("renders env_source fields as read-only with a chip badge", async () => {
    const schema: ConfigField[] = [
      {
        key: "AWS_REGION",
        label: "AWS Region",
        description: "Region from env",
        field_type: "string",
        required: false,
        options: [],
        env_source: "AWS_REGION",
      },
    ];
    mockListPlugins.mockResolvedValue([makePlugin(schema)]);
    mockSettingsGet.mockResolvedValue(makeSettings());

    const wrapper = mountPage();
    await flushPromises();

    // The field must be read-only.
    const textFields = wrapper.findAllComponents(components.VTextField);
    const match = textFields.find((f) => f.props("label") === "AWS Region");
    expect(match).toBeTruthy();
    expect(match?.props("readonly")).toBe(true);

    // A VChip containing "From env" should exist somewhere in the component.
    const chips = wrapper.findAllComponents(components.VChip);
    const envChip = chips.find((c) => c.text().includes("From env"));
    expect(envChip).toBeTruthy();
  });

  it("shows a warning when the plugin is not loaded", async () => {
    // listPlugins returns an empty array — plugin "aws" is not loaded.
    mockListPlugins.mockResolvedValue([]);
    mockSettingsGet.mockResolvedValue(makeSettings());

    const wrapper = mountPage();
    await flushPromises();

    const alerts = wrapper.findAllComponents(components.VAlert);
    const notLoadedAlert = alerts.find((a) =>
      a.text().includes("not currently loaded"),
    );
    expect(notLoadedAlert).toBeTruthy();
  });
});
