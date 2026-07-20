import { describe, expect, it, vi, beforeEach } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";
import GitOpsCard from "@/components/views/deployment/GitOpsCard.vue";
import type { GitOpsStatus, BuildSummary } from "@/types/api";

vi.mock("@/api/gitops", () => ({
  gitopsApi: {
    getConfig: vi.fn(),
    setConfig: vi.fn(),
    deleteConfig: vi.fn(),
    triggerBuild: vi.fn(),
    listBuilds: vi.fn(),
    listJobPods: vi.fn(),
    getPodLogsHistory: vi.fn(),
  },
}));

vi.mock("@/api/diagnostics", () => ({
  diagnosticsApi: {
    create: vi.fn(),
    status: vi.fn(),
    result: vi.fn(),
    streamUrl: vi.fn(() => "/mock-stream"),
  },
}));

import { gitopsApi } from "@/api/gitops";

const vuetify = createVuetify({ components, directives });

function makeStatus(overrides: Partial<GitOpsStatus> = {}): GitOpsStatus {
  return {
    enabled: true,
    config: {
      repo_url: "https://github.com/org/repo",
      branch: "main",
      token_secret: "git-token",
      dockerfile_path: "Dockerfile",
      docker_context: ".",
      ecr_repository: "123456.dkr.ecr.us-east-1.amazonaws.com/app",
      include_paths: [],
      exclude_paths: [],
      poll_interval_seconds: 60,
      webhook_enabled: false,
    },
    last_commit_sha: "abc1234def5678",
    last_build_status: "success",
    last_build_job: "build-job-1",
    last_build_time: new Date().toISOString(),
    last_build_error: null,
    ...overrides,
  };
}

function mountCard(props: Partial<InstanceType<typeof GitOpsCard>["$props"]> = {}) {
  return mount(GitOpsCard, {
    props: {
      namespace: "default",
      deploymentName: "my-app",
      ...props,
    },
    global: {
      plugins: [vuetify],
      stubs: {
        GitOpsConfigDialog: { template: "<div />" },
        ConfirmDialog: { template: "<div />" },
        LogViewer: { template: "<div />" },
        AiFixButton: { template: "<div />" },
      },
    },
  });
}

describe("GitOpsCard", () => {
  beforeEach(() => {
    vi.mocked(gitopsApi.getConfig).mockReset();
    vi.mocked(gitopsApi.listBuilds).mockReset();
    vi.stubGlobal("EventSource", class { addEventListener() {} close() {} onerror = null; });
  });

  it('shows "GitOps not configured" and Enable button when disabled', async () => {
    vi.mocked(gitopsApi.getConfig).mockResolvedValue(
      makeStatus({ enabled: false, config: null }),
    );

    const wrapper = mountCard();
    await flushPromises();

    expect(wrapper.html()).toContain("GitOps not configured");

    const enableBtn = wrapper
      .findAllComponents(components.VBtn)
      .find((b) => b.text().includes("Enable"));
    expect(enableBtn).toBeTruthy();
    wrapper.unmount();
  });

  it("shows repo URL and branch when enabled", async () => {
    vi.mocked(gitopsApi.getConfig).mockResolvedValue(makeStatus());

    const wrapper = mountCard();
    await flushPromises();

    expect(wrapper.html()).toContain("https://github.com/org/repo");
    expect(wrapper.html()).toContain("main");
    wrapper.unmount();
  });

  it("shows build status chip with success color", async () => {
    vi.mocked(gitopsApi.getConfig).mockResolvedValue(
      makeStatus({ last_build_status: "success" }),
    );

    const wrapper = mountCard();
    await flushPromises();

    const chips = wrapper.findAllComponents(components.VChip);
    const statusChip = chips.find((c) => c.text().includes("success"));
    expect(statusChip).toBeTruthy();
    expect(statusChip!.props("color")).toBe("success");
    wrapper.unmount();
  });

  it("shows build status chip with error color for failed", async () => {
    vi.mocked(gitopsApi.getConfig).mockResolvedValue(
      makeStatus({ last_build_status: "failed" }),
    );

    const wrapper = mountCard();
    await flushPromises();

    const chips = wrapper.findAllComponents(components.VChip);
    const statusChip = chips.find((c) => c.text().includes("failed"));
    expect(statusChip).toBeTruthy();
    expect(statusChip!.props("color")).toBe("error");
    wrapper.unmount();
  });

  it("shows build status chip with info color for building", async () => {
    vi.mocked(gitopsApi.getConfig).mockResolvedValue(
      makeStatus({ last_build_status: "building" }),
    );

    const wrapper = mountCard();
    await flushPromises();

    const chips = wrapper.findAllComponents(components.VChip);
    const statusChip = chips.find((c) => c.text().includes("building"));
    expect(statusChip).toBeTruthy();
    expect(statusChip!.props("color")).toBe("info");
    wrapper.unmount();
  });

  it("toggles build history table when Build History button is clicked", async () => {
    const builds: BuildSummary[] = [
      {
        job_name: "build-1",
        commit_sha: "abc123",
        status: "succeeded",
        started_at: new Date().toISOString(),
        completed_at: new Date().toISOString(),
        image_tag: "v1.0.0",
      },
    ];

    vi.mocked(gitopsApi.getConfig).mockResolvedValue(makeStatus());
    vi.mocked(gitopsApi.listBuilds).mockResolvedValue({ builds });

    const wrapper = mountCard();
    await flushPromises();

    // Table should not be visible initially
    expect(wrapper.findComponent(components.VTable).exists()).toBe(false);

    // Click the Build History button
    const historyBtn = wrapper
      .findAllComponents(components.VBtn)
      .find((b) => b.text().includes("Build History"));
    expect(historyBtn).toBeTruthy();

    await historyBtn!.trigger("click");
    await flushPromises();

    // Table should now be visible
    expect(wrapper.findComponent(components.VTable).exists()).toBe(true);
    expect(wrapper.html()).toContain("build-1");

    // Click again to collapse
    await historyBtn!.trigger("click");
    await flushPromises();

    expect(wrapper.findComponent(components.VTable).exists()).toBe(false);
    wrapper.unmount();
  });

  it("shows abbreviated commit SHA", async () => {
    vi.mocked(gitopsApi.getConfig).mockResolvedValue(
      makeStatus({ last_commit_sha: "abc1234def5678" }),
    );

    const wrapper = mountCard();
    await flushPromises();

    expect(wrapper.html()).toContain("abc1234");
    wrapper.unmount();
  });
});
