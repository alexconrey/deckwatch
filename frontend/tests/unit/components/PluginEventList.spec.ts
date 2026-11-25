import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";
import PluginEventList from "@/components/common/PluginEventList.vue";
import type { PluginEventReport } from "@/types/api";

const vuetify = createVuetify({ components, directives });

function event(overrides: Partial<PluginEventReport> = {}): PluginEventReport {
  return {
    plugin: "aws",
    export: "provision",
    severity: "warning",
    code: "aws.slow",
    message: "throttled by the API",
    retryable: false,
    ...overrides,
  };
}

function mountList(props: { events: PluginEventReport[]; hideInfo?: boolean; hidePlugin?: boolean }) {
  return mount(PluginEventList, { props, global: { plugins: [vuetify] } });
}

describe("PluginEventList", () => {
  it("renders plugin, export, code, message and the optional details", () => {
    const w = mountList({
      events: [
        event({
          severity: "error",
          code: "aws.assume_role_failed",
          message: "AccessDenied",
          hint: "Grant sts:AssumeRole",
          retryable: true,
          resource_id: "rds",
          object: { api_version: "v1", kind: "ServiceAccount", name: "shop-sa" },
          trace_id: "4bf92f3577b34da6a3ce929d0e0e4736",
        }),
      ],
    });
    const text = w.text();
    for (const expected of [
      "aws",
      "provision",
      "aws.assume_role_failed",
      "AccessDenied",
      "Hint: Grant sts:AssumeRole",
      "retryable",
      "Resource: rds",
      "ServiceAccount shop-sa",
      "4bf92f3577b34da6a3ce929d0e0e4736",
    ]) {
      expect(text).toContain(expected);
    }
  });

  it("lists the most severe events first", () => {
    const w = mountList({
      events: [
        event({ severity: "info", message: "a notice" }),
        event({ severity: "warning", message: "a warning" }),
        event({ severity: "error", message: "an error" }),
      ],
    });
    const severities = w.findAll(".plugin-event").map((e) => e.attributes("data-severity"));
    expect(severities).toEqual(["error", "warning", "info"]);
  });

  // Plugin text is untrusted: markup must stay text.
  it("never renders plugin text as HTML", () => {
    const w = mountList({
      events: [
        event({
          message: '<img src=x onerror="alert(1)"><b>bold</b>',
          hint: "<script>steal()</script>",
          code: "aws.<i>x</i>",
        }),
      ],
    });
    expect(w.find("img").exists()).toBe(false);
    expect(w.find("b").exists()).toBe(false);
    expect(w.find("script").exists()).toBe(false);
    // v-icon renders an <i>; the injected one would contain "x".
    expect(w.findAll("i").filter((n) => n.text() === "x")).toHaveLength(0);
    expect(w.text()).toContain("aws.<i>x</i>");
    expect(w.text()).toContain('<img src=x onerror="alert(1)"><b>bold</b>');
    expect(w.text()).toContain("<script>steal()</script>");
  });

  it("can hide info events and the plugin name", () => {
    const w = mountList({
      events: [event({ severity: "info", message: "only info" }), event({ message: "kept" })],
      hideInfo: true,
      hidePlugin: true,
    });
    expect(w.text()).not.toContain("only info");
    expect(w.text()).toContain("kept");
    expect(w.text()).not.toMatch(/^aws/);
  });

  it("renders nothing for no events", () => {
    const w = mountList({ events: [] });
    expect(w.find("[data-testid='plugin-event-list']").exists()).toBe(false);
  });
});
