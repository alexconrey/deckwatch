import { describe, expect, it, vi, beforeEach } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { defineComponent, h } from "vue";
import { createMemoryHistory, createRouter } from "vue-router";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";

// The namespace switcher's #item slot changed shape in Vuetify 4: the slot's
// `item` is now the raw item (it was the internal item, with the raw one under
// `item.raw`). Reading `item.raw.divider` there throws, and the menu renders no
// options at all. These tests open the real menu and check every row.

const { mockList } = vi.hoisted(() => ({ mockList: vi.fn() }));

vi.mock("@/api/namespaces", () => ({
  namespacesApi: { list: mockList, create: vi.fn() },
}));

// The app bar polls plugin problems for its Problems button.
vi.mock("@/api/pluginStatus", () => ({
  pluginStatusApi: { problems: vi.fn().mockResolvedValue([]) },
}));

vi.mock("@/composables/useFeatures", async () => {
  const { ref } = await import("vue");
  return {
    useFeatures: () => ({ features: ref({ prometheus: false, registry: false }), refresh: vi.fn() }),
  };
});

import AppLayout from "@/layouts/AppLayout.vue";

const vuetify = createVuetify({ components, directives });

// Vuetify's overlays read visualViewport, which happy-dom doesn't provide.
beforeEach(() => {
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
  // Replaced at build time by vite.config.ts `define`; vitest.config.ts has no define.
  vi.stubGlobal("__APP_VERSION__", "0.0.0-test");
  localStorage.clear();
  mockList.mockResolvedValue({ namespaces: ["default", "team-a"] });
});

const Blank = defineComponent({ render: () => h("div") });

async function mountLayout() {
  const router = createRouter({
    history: createMemoryHistory(),
    // Every route the app bar links to.
    routes: [
      "Applications",
      "Deployments",
      "Registry",
      "ClusterOverview",
      "McpActivity",
      "PluginProblems",
      "Settings",
    ].map((name) => ({
      path: `/${name.toLowerCase()}`,
      name,
      component: Blank,
    })),
  });
  await router.push("/applications");
  const wrapper = mount(
    defineComponent({ render: () => h(components.VApp, null, () => h(AppLayout)) }),
    {
      global: {
        plugins: [vuetify, createPinia(), router],
        stubs: { CreateNamespaceDialog: true },
      },
      attachTo: document.body,
    },
  );
  await flushPromises();
  return wrapper;
}

async function openNamespaceMenu(wrapper: Awaited<ReturnType<typeof mountLayout>>) {
  await wrapper.get('[data-testid="namespace-switcher"] .v-field').trigger("mousedown");
  await flushPromises();
  const menu = document.body.querySelector(".v-overlay--active .v-list");
  expect(menu, "namespace menu did not open").not.toBeNull();
  return menu as HTMLElement;
}

describe("AppLayout namespace switcher", () => {
  it("lists every namespace, then a divider, then Create Namespace…", async () => {
    const wrapper = await mountLayout();
    const menu = await openNamespaceMenu(wrapper);

    const rows = [...menu.querySelectorAll(".v-list-item, .v-divider")].map((el) =>
      el.classList.contains("v-divider") ? "<divider>" : el.querySelector(".v-list-item-title")?.textContent?.trim(),
    );
    expect(rows).toEqual(["default", "team-a", "<divider>", "Create Namespace…"]);
    wrapper.unmount();
  });

  it("renders the Create Namespace… row with its icon and primary colour", async () => {
    const wrapper = await mountLayout();
    const menu = await openNamespaceMenu(wrapper);

    const create = [...menu.querySelectorAll(".v-list-item")].find((el) =>
      el.textContent?.includes("Create Namespace…"),
    );
    expect(create).toBeDefined();
    expect(create!.querySelector(".mdi-plus")).not.toBeNull();
    expect(create!.classList.contains("text-primary")).toBe(true);
    wrapper.unmount();
  });

  it("keeps the namespaces as selectable options", async () => {
    const wrapper = await mountLayout();
    const menu = await openNamespaceMenu(wrapper);

    const options = [...menu.querySelectorAll('[role="option"]')].map((el) => el.textContent?.trim());
    expect(options).toEqual(["default", "team-a", "Create Namespace…"]);
    wrapper.unmount();
  });
});
