import { describe, expect, it } from "vitest";
import { applicationResourcesApi } from "@/api/application_resources";
import { ApiError, pluginEventsOf } from "@/api/client";
import { mockFetchOnce } from "../helpers/mockFetch";

const report = {
  plugin: "aws",
  export: "deprovision",
  severity: "info",
  code: "plugin.unspecified",
  message: "Bucket kept because it isn't empty",
  retryable: false,
};

describe("applicationResourcesApi", () => {
  // deckwatch#250: the plugin's message comes back from DELETE.
  it("returns the plugin's message and events from deprovision", async () => {
    const fetchMock = mockFetchOnce({
      body: { message: "Bucket kept because it isn't empty", plugin_events: [report] },
    });
    const result = await applicationResourcesApi.deprovision("shop", "web", "aws", "s3");
    expect(result.message).toBe("Bucket kept because it isn't empty");
    expect(result.plugin_events).toHaveLength(1);
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/namespaces/shop/applications/web/resources/aws/s3",
      expect.objectContaining({ method: "DELETE" }),
    );
  });

  it("carries the plugin events of a failed provision on the error", async () => {
    mockFetchOnce({
      status: 502,
      body: {
        error: "plugin_failed",
        message: "Plugin call failed: plugin.trap: provision() trapped",
        plugin_events: [{ ...report, severity: "error", code: "plugin.trap" }],
      },
    });
    const err = await applicationResourcesApi
      .provision("shop", "web", "aws", "rds", { fields: {} })
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(pluginEventsOf(err).map((e) => e.code)).toEqual(["plugin.trap"]);
    expect(pluginEventsOf(new Error("network"))).toEqual([]);
  });
});
