import { describe, expect, it, vi, beforeEach } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";
import LogViewer from "@/components/common/LogViewer.vue";

const vuetify = createVuetify({ components, directives });

// Minimal EventSource stub so the component doesn't error during mount.
class MockEventSource {
  static instances: MockEventSource[] = [];
  url: string;
  listeners: Record<string, Function[]> = {};
  onerror: Function | null = null;

  constructor(url: string) {
    this.url = url;
    MockEventSource.instances.push(this);
  }

  addEventListener(event: string, cb: Function) {
    if (!this.listeners[event]) this.listeners[event] = [];
    this.listeners[event].push(cb);
  }

  close() {}
}

function mockFetchHistory(lines: string[]) {
  (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
    ok: true,
    json: () => Promise.resolve({ lines }),
  });
}

function mountLogViewer(
  props: Partial<InstanceType<typeof LogViewer>["$props"]> = {},
) {
  return mount(LogViewer, {
    props: {
      namespace: "default",
      podName: "my-pod-abc123",
      ...props,
    },
    global: {
      plugins: [vuetify],
      stubs: {
        DiagnoseButton: { template: "<div />" },
      },
    },
  });
}

describe("LogViewer", () => {
  beforeEach(() => {
    MockEventSource.instances = [];
    vi.stubGlobal("EventSource", MockEventSource);
  });

  it("renders fetched log lines after mount", async () => {
    mockFetchHistory(["INFO Starting server", "INFO Listening on :8080"]);

    const wrapper = mountLogViewer();
    await flushPromises();

    const html = wrapper.html();
    expect(html).toContain("Starting server");
    expect(html).toContain("Listening on :8080");
    wrapper.unmount();
  });

  it("filters displayed lines based on search input", async () => {
    mockFetchHistory([
      "INFO request handled",
      "ERROR connection refused",
      "INFO health check ok",
    ]);

    const wrapper = mountLogViewer();
    await flushPromises();

    // All three lines visible initially
    expect(wrapper.html()).toContain("request handled");
    expect(wrapper.html()).toContain("connection refused");
    expect(wrapper.html()).toContain("health check ok");

    // Type a search term
    const textField = wrapper.findComponent(components.VTextField);
    await textField.setValue("ERROR");
    await flushPromises();

    // Only the matching line should remain visible in the log output area.
    // We scope to .log-output to avoid matching text in the DiagnoseButton
    // stub's `logs` prop attribute.
    const logOutput = wrapper.find(".log-output");
    expect(logOutput.html()).toContain("connection refused");
    expect(logOutput.html()).not.toContain("request handled");
    expect(logOutput.html()).not.toContain("health check ok");
    wrapper.unmount();
  });

  it("has a download button", async () => {
    mockFetchHistory(["line1"]);

    const wrapper = mountLogViewer();
    await flushPromises();

    const downloadBtn = wrapper
      .findAllComponents(components.VBtn)
      .find((b) => b.attributes("title") === "Download logs");
    expect(downloadBtn).toBeTruthy();
    wrapper.unmount();
  });

  it("shows empty state when no logs are returned", async () => {
    mockFetchHistory([]);

    const wrapper = mountLogViewer();
    await flushPromises();

    expect(wrapper.html()).toContain("No logs available");
    wrapper.unmount();
  });

  it("shows error message when fetch fails", async () => {
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
      ok: false,
      json: () => Promise.resolve({ message: "pod not found" }),
    });

    const wrapper = mountLogViewer();
    await flushPromises();

    expect(wrapper.html()).toContain("pod not found");
    wrapper.unmount();
  });

  it("creates an EventSource to stream logs after history loads", async () => {
    mockFetchHistory(["line1"]);

    const wrapper = mountLogViewer();
    await flushPromises();

    expect(MockEventSource.instances.length).toBeGreaterThanOrEqual(1);
    const lastInstance = MockEventSource.instances[MockEventSource.instances.length - 1];
    expect(lastInstance.url).toContain("/logs?");
    expect(lastInstance.url).toContain("follow=true");
    wrapper.unmount();
  });
});
