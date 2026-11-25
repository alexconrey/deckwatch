import { describe, expect, it, vi, beforeEach } from "vitest";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";
import type { DeckwatchSettings, GitOpsConfig } from "@/types/api";

// The Repository, Git Token and OCI Registry pickers render their options
// through an #item slot that shows each option's title and subtitle. In
// Vuetify 4 the slot's `item` is the raw item (in Vuetify 3 it was the
// internal item, with the raw one under `item.raw`), so `item.raw.title`
// throws and the menus render nothing. These tests open each real menu.

const { mockSettingsGet, mockListBranches } = vi.hoisted(() => ({
  mockSettingsGet: vi.fn(),
  mockListBranches: vi.fn(),
}));

vi.mock("@/api/settings", () => ({ settingsApi: { get: mockSettingsGet } }));
vi.mock("@/api/gitops", () => ({ gitopsApi: { listBranches: mockListBranches } }));

import GitOpsConfigDialog from "@/components/views/deployment/GitOpsConfigDialog.vue";

const vuetify = createVuetify({ components, directives });

const settings: Partial<DeckwatchSettings> = {
  git_repositories: [
    { name: "storefront", url: "https://github.com/acme/storefront.git", default_branch: "main" },
    { name: "platform-api", url: "https://github.com/acme/platform-api.git", default_branch: "develop" },
  ],
  git_token_secrets: [{ name: "github-bot", secret_name: "github-bot-token", namespace: "deckwatch" }],
  oci_registries: [
    { name: "deckwatch", url: "registry.deckwatch.local", registry_type: "deckwatch", builtin: true },
    { name: "ghcr", url: "ghcr.io/acme", registry_type: "ghcr" },
  ],
};

beforeEach(() => {
  // Vuetify's overlays read visualViewport, which happy-dom doesn't provide.
  if (!globalThis.visualViewport) {
    vi.stubGlobal("visualViewport", {
      width: 1440,
      height: 900,
      offsetLeft: 0,
      offsetTop: 0,
      scale: 1,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    });
  }
  mockSettingsGet.mockResolvedValue(settings);
  mockListBranches.mockReset();
  mockListBranches.mockResolvedValue({ branches: ["main", "develop"], default_branch: "main" });
});

const existingConfig: GitOpsConfig = {
  repo_url: "https://github.com/acme/storefront.git",
  branch: "main",
  token_secret: "github-bot-token",
  git_auth_user: "x-access-token",
  dockerfile_path: "Dockerfile",
  docker_context: ".",
  ecr_repository: "",
  oci_repository: "ghcr.io/acme",
  include_paths: [],
  exclude_paths: [],
  poll_interval_seconds: 60,
  webhook_enabled: false,
};

async function mountDialog() {
  const wrapper = mount(GitOpsConfigDialog, {
    props: { modelValue: true, namespace: "default", deploymentName: "web", initialConfig: null },
    global: { plugins: [vuetify], stubs: { RegistryTagPicker: true } },
    attachTo: document.body,
  });
  await flushPromises();
  return wrapper;
}

/** Open the select labelled `label` and return its rows as [title, subtitle]. */
async function menuRows(wrapper: VueWrapper, label: string): Promise<[string, string][]> {
  const select = wrapper
    .findAllComponents(components.VSelect)
    .find((s) => s.props("label") === label);
  expect(select, `no select labelled ${label}`).toBeDefined();
  await select!.find(".v-field").trigger("mousedown");
  await flushPromises();
  const menu = [...document.body.querySelectorAll(".v-overlay--active .v-list")].at(-1);
  expect(menu, `${label} menu did not open`).toBeDefined();
  const rows = [...menu!.querySelectorAll(".v-list-item")].map(
    (el) =>
      [
        el.querySelector(".v-list-item-title")?.textContent?.trim() ?? "",
        el.querySelector(".v-list-item-subtitle")?.textContent?.trim() ?? "",
      ] as [string, string],
  );
  // Close it again so the next menu is the active one.
  await select!.find(".v-field").trigger("mousedown");
  await flushPromises();
  return rows;
}

describe("GitOpsConfigDialog pickers", () => {
  it("shows each repository's name and URL, plus Custom URL…", async () => {
    const wrapper = await mountDialog();
    expect(await menuRows(wrapper, "Repository")).toEqual([
      ["storefront", "https://github.com/acme/storefront.git"],
      ["platform-api", "https://github.com/acme/platform-api.git"],
      ["Custom URL…", "Type your own"],
    ]);
    wrapper.unmount();
  });

  it("shows each Git token with its namespace/secret", async () => {
    const wrapper = await mountDialog();
    expect(await menuRows(wrapper, "Git Token")).toEqual([
      ["None (public repo)", ""],
      ["github-bot", "deckwatch/github-bot-token"],
    ]);
    wrapper.unmount();
  });

  it("shows each OCI registry with its type and URL, plus Custom URL…", async () => {
    const wrapper = await mountDialog();
    expect(await menuRows(wrapper, "OCI Registry")).toEqual([
      ["deckwatch", "deckwatch · registry.deckwatch.local"],
      ["ghcr", "ghcr · ghcr.io/acme"],
      ["Custom URL…", "Type your own"],
    ]);
    wrapper.unmount();
  });

  it("selecting Custom URL… reveals the free-text repository field", async () => {
    const wrapper = await mountDialog();
    const repo = wrapper.findAllComponents(components.VSelect).find((s) => s.props("label") === "Repository");
    expect(wrapper.findAllComponents(components.VTextField).some((t) => t.props("label") === "Custom repository URL")).toBe(false);
    await repo!.find(".v-field").trigger("mousedown");
    await flushPromises();
    const custom = [...document.body.querySelectorAll(".v-overlay--active .v-list-item")].find((el) =>
      el.textContent?.includes("Custom URL…"),
    ) as HTMLElement;
    custom.click();
    await flushPromises();
    expect(wrapper.findAllComponents(components.VTextField).some((t) => t.props("label") === "Custom repository URL")).toBe(true);
    wrapper.unmount();
  });

  it("opens an existing config without a branch error, and looks branches up when typing", async () => {
    // Open it the way GitOpsCard does: mounted closed, then opened. The form is
    // filled in when it opens, while the settings (and so the token
    // selection) are still loading.
    const wrapper = mount(GitOpsConfigDialog, {
      props: { modelValue: false, namespace: "default", deploymentName: "web", initialConfig: existingConfig },
      global: { plugins: [vuetify], stubs: { RegistryTagPicker: true } },
      attachTo: document.body,
    });
    await wrapper.setProps({ modelValue: true });
    await flushPromises();
    const branch = wrapper.findAllComponents(components.VAutocomplete).find((a) => a.props("label") === "Branch")!;

    // Filling in the form must not count as a search: it would run before the
    // token selection is restored and report a missing token.
    expect(wrapper.text()).not.toContain("Select a Git token");
    expect(mockListBranches).not.toHaveBeenCalled();
    expect(branch.props("errorMessages")).toEqual([]);

    await branch.find("input:not([type=hidden])").setValue("dev");
    await flushPromises();
    expect(mockListBranches).toHaveBeenCalledWith({
      repoUrl: "https://github.com/acme/storefront.git",
      tokenSecret: "github-bot",
      namespace: "default",
    });
    wrapper.unmount();
  });
});
