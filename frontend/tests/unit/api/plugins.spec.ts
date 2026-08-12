import { describe, expect, it } from "vitest";
import { pluginsApi } from "@/api/plugins";
import { applicationsApi } from "@/api/applications";
import { mockFetchOnce, mockFetchSequence } from "../helpers/mockFetch";
import type { ApplicationPluginEntry, ConfigField, PluginSummary } from "@/types/api";

// ---------------------------------------------------------------------------
// Type shape tests — assert the declared interfaces match their spec at runtime
// ---------------------------------------------------------------------------

describe("ConfigField type shape", () => {
  it("accepts all ConfigFieldType variants", () => {
    const fields: ConfigField[] = [
      {
        key: "MY_STRING",
        label: "String field",
        description: "A plain string",
        field_type: "string",
        required: false,
        options: [],
      },
      {
        key: "MY_SECRET",
        label: "Secret field",
        description: "An encrypted field",
        field_type: "secret",
        required: true,
        options: [],
        default: null,
        env_source: null,
      },
      {
        key: "MY_BOOL",
        label: "Bool field",
        description: "A boolean toggle",
        field_type: "bool",
        required: false,
        options: [],
        default: "false",
      },
      {
        key: "MY_SELECT",
        label: "Select field",
        description: "A dropdown",
        field_type: "select",
        required: true,
        options: ["a", "b", "c"],
      },
    ];

    expect(fields).toHaveLength(4);
    expect(fields[0].field_type).toBe("string");
    expect(fields[1].field_type).toBe("secret");
    expect(fields[2].field_type).toBe("bool");
    expect(fields[3].field_type).toBe("select");
  });

  it("ConfigField with env_source carries the env var name", () => {
    const field: ConfigField = {
      key: "AWS_REGION",
      label: "Region",
      description: "From env",
      field_type: "string",
      required: false,
      options: [],
      env_source: "AWS_REGION",
    };
    expect(field.env_source).toBe("AWS_REGION");
  });

  it("ConfigField default is optional / nullable", () => {
    const withDefault: ConfigField = {
      key: "K",
      label: "L",
      description: "D",
      field_type: "string",
      required: false,
      options: [],
      default: "default-val",
    };
    const noDefault: ConfigField = {
      key: "K2",
      label: "L2",
      description: "D2",
      field_type: "secret",
      required: false,
      options: [],
      default: null,
    };
    expect(withDefault.default).toBe("default-val");
    expect(noDefault.default).toBeNull();
  });

  it("PluginSummary carries config_schema as an array", () => {
    const summary: PluginSummary = {
      name: "aws",
      version: "1.0.0",
      description: "AWS plugin",
      provides: ["s3", "rds"],
      depends_on: [],
      wasm_size_bytes: 1024,
      config_schema: [
        {
          key: "AWS_REGION",
          label: "AWS Region",
          description: "The AWS region",
          field_type: "string",
          required: true,
          options: [],
          env_source: "AWS_REGION",
        },
      ],
    };

    expect(summary.config_schema).toHaveLength(1);
    expect(summary.config_schema[0].key).toBe("AWS_REGION");
    expect(summary.config_schema[0].env_source).toBe("AWS_REGION");
  });

  it("PluginSummary with empty config_schema is valid (old plugin compat)", () => {
    const summary: PluginSummary = {
      name: "legacy",
      version: "",
      description: "",
      provides: [],
      depends_on: [],
      wasm_size_bytes: 512,
      config_schema: [],
    };
    expect(summary.config_schema).toHaveLength(0);
  });

  it("ApplicationPluginEntry shape", () => {
    const entry: ApplicationPluginEntry = {
      plugin_name: "aws",
      created_at: "2026-08-12T00:00:00Z",
      is_loaded: true,
    };
    expect(entry.plugin_name).toBe("aws");
    expect(entry.is_loaded).toBe(true);

    const unloaded: ApplicationPluginEntry = {
      plugin_name: "broken",
      created_at: "2026-08-12T00:00:00Z",
      is_loaded: false,
    };
    expect(unloaded.is_loaded).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// pluginsApi client tests
// ---------------------------------------------------------------------------

const MOCK_PLUGINS: PluginSummary[] = [
  {
    name: "aws",
    version: "0.4.0",
    description: "AWS integrations",
    provides: [],
    depends_on: [],
    wasm_size_bytes: 2048,
    config_schema: [
      {
        key: "AWS_REGION",
        label: "AWS Region",
        description: "Region",
        field_type: "string",
        required: true,
        options: [],
        env_source: "AWS_REGION",
      },
    ],
  },
];

describe("pluginsApi.list", () => {
  it("GETs /api/plugins and returns PluginSummary[]", async () => {
    const fetchMock = mockFetchOnce({ body: MOCK_PLUGINS });
    const result = await pluginsApi.list();

    expect(fetchMock.mock.calls[0][0]).toBe("/api/plugins");
    expect(result).toHaveLength(1);
    expect(result[0].name).toBe("aws");
    expect(result[0].config_schema[0].field_type).toBe("string");
  });

  it("returns an empty array when no plugins are loaded", async () => {
    mockFetchOnce({ body: [] });
    const result = await pluginsApi.list();
    expect(result).toEqual([]);
  });
});

describe("pluginsApi.getSchema", () => {
  it("GETs /api/plugins/{name}/schema and returns ConfigField[]", async () => {
    const schema: ConfigField[] = MOCK_PLUGINS[0].config_schema;
    const fetchMock = mockFetchOnce({ body: schema });

    const result = await pluginsApi.getSchema("aws");

    expect(fetchMock.mock.calls[0][0]).toBe("/api/plugins/aws/schema");
    expect(result).toHaveLength(1);
    expect(result[0].key).toBe("AWS_REGION");
  });

  it("returns an empty array when the plugin has no schema", async () => {
    mockFetchOnce({ body: [] });
    const result = await pluginsApi.getSchema("legacy");
    expect(result).toEqual([]);
  });
});

describe("pluginsApi.saveConfig", () => {
  it("POSTs /api/plugins/{name}/config with the given config object", async () => {
    const fetchMock = mockFetchOnce({ status: 204 });

    await pluginsApi.saveConfig("aws", { AWS_REGION: "us-east-1" });

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/plugins/aws/config");
    expect(init?.method).toBe("POST");
    expect(JSON.parse(init?.body as string)).toEqual({ AWS_REGION: "us-east-1" });
  });

  it("POSTs multiple config key-value pairs", async () => {
    const fetchMock = mockFetchOnce({ status: 204 });

    await pluginsApi.saveConfig("aws", {
      AWS_REGION: "us-gov-west-1",
      BUCKET_PREFIX: "myorg-",
    });

    const body = JSON.parse(fetchMock.mock.calls[0][1]?.body as string);
    expect(body.AWS_REGION).toBe("us-gov-west-1");
    expect(body.BUCKET_PREFIX).toBe("myorg-");
  });
});

// ---------------------------------------------------------------------------
// applicationsApi plugin association tests
// ---------------------------------------------------------------------------

describe("applicationsApi.listPlugins", () => {
  it("GETs the correct URL and returns ApplicationPluginEntry[]", async () => {
    const entries: ApplicationPluginEntry[] = [
      { plugin_name: "aws", created_at: "2026-08-12T00:00:00Z", is_loaded: true },
    ];
    const fetchMock = mockFetchOnce({ body: entries });

    const result = await applicationsApi.listPlugins("production", "crm");

    expect(fetchMock.mock.calls[0][0]).toBe(
      "/api/namespaces/production/applications/crm/plugins",
    );
    expect(result).toHaveLength(1);
    expect(result[0].plugin_name).toBe("aws");
    expect(result[0].is_loaded).toBe(true);
  });

  it("reflects is_loaded: false for plugins not currently loaded", async () => {
    const entries: ApplicationPluginEntry[] = [
      { plugin_name: "broken-plugin", created_at: "2026-08-12T00:00:00Z", is_loaded: false },
    ];
    mockFetchOnce({ body: entries });
    const result = await applicationsApi.listPlugins("ns", "app");
    expect(result[0].is_loaded).toBe(false);
  });

  it("returns an empty array when no plugins are associated", async () => {
    mockFetchOnce({ body: [] });
    const result = await applicationsApi.listPlugins("ns", "app");
    expect(result).toEqual([]);
  });
});

describe("applicationsApi.addPlugin", () => {
  it("POSTs to the correct URL and returns void on 204", async () => {
    const fetchMock = mockFetchOnce({ status: 204 });

    await applicationsApi.addPlugin("production", "crm", "aws");

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe(
      "/api/namespaces/production/applications/crm/plugins/aws",
    );
    expect(init?.method).toBe("POST");
  });
});

describe("applicationsApi.removePlugin", () => {
  it("DELETEs from the correct URL and returns void on 204", async () => {
    const fetchMock = mockFetchOnce({ status: 204 });

    await applicationsApi.removePlugin("production", "crm", "aws");

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe(
      "/api/namespaces/production/applications/crm/plugins/aws",
    );
    expect(init?.method).toBe("DELETE");
  });
});

describe("applicationsApi plugin association — full workflow", () => {
  it("add then list reflects the new entry", async () => {
    const addedEntry: ApplicationPluginEntry = {
      plugin_name: "aws",
      created_at: "2026-08-12T00:00:00Z",
      is_loaded: true,
    };
    mockFetchSequence([
      { status: 204 },          // addPlugin → 204
      { body: [addedEntry] },   // listPlugins → [entry]
    ]);

    await applicationsApi.addPlugin("ns", "app", "aws");
    const list = await applicationsApi.listPlugins("ns", "app");
    expect(list).toHaveLength(1);
    expect(list[0].plugin_name).toBe("aws");
  });
});
