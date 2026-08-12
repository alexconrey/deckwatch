import { describe, expect, it } from "vitest";
import { pluginsApi } from "@/api/plugins";
import { mockFetchOnce } from "../helpers/mockFetch";
import type { ConfigField, PluginSummary } from "@/types/api";

// ---------------------------------------------------------------------------
// Type shape tests — assert runtime behaviour matches the declared interface
// ---------------------------------------------------------------------------

describe("ConfigField type shape", () => {
  it("accepts all ConfigFieldType variants", () => {
    // This is a compile-time check exercised at runtime: constructing a valid
    // ConfigField for each field_type should not throw at the TS level.
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
});

// ---------------------------------------------------------------------------
// API client tests
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
});
