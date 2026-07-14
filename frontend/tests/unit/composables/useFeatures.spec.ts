import { describe, expect, it, vi, beforeEach } from "vitest";
import { defineComponent, h, nextTick } from "vue";
import { mount, flushPromises } from "@vue/test-utils";
import { mockFetchOnce } from "../helpers/mockFetch";

/**
 * useFeatures relies on onMounted, so it must run inside a live component.
 * We mount a tiny wrapper and inspect the composable's returned refs after
 * the fetch resolves.
 *
 * Because useFeatures stores `features` in a module-level ref (singleton),
 * we dynamically re-import the module each test to get a fresh ref.
 */
async function mountWithFeatures() {
  // Reset the module so the singleton `features` ref starts fresh.
  vi.resetModules();
  const { useFeatures } = await import("@/composables/useFeatures");

  let composable!: ReturnType<typeof useFeatures>;
  const Wrapper = defineComponent({
    setup() {
      composable = useFeatures();
      return () => h("div");
    },
  });
  const wrapper = mount(Wrapper);
  return { wrapper, composable };
}

describe("useFeatures", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  it("fetches features on mount and exposes them", async () => {
    mockFetchOnce({ body: { prometheus: true, registry: false } });
    const { wrapper, composable } = await mountWithFeatures();

    await flushPromises();

    expect(composable.features.value).toEqual({
      prometheus: true,
      registry: false,
    });
    wrapper.unmount();
  });

  it("defaults to all-false when fetch fails", async () => {
    // Simulate a network error by making fetch reject.
    vi.stubGlobal(
      "fetch",
      vi.fn().mockRejectedValue(new Error("network down")),
    );
    const { wrapper, composable } = await mountWithFeatures();

    await flushPromises();

    expect(composable.features.value).toEqual({
      prometheus: false,
      registry: false,
    });
    wrapper.unmount();
  });

  it("defaults to all-false on non-ok HTTP response", async () => {
    mockFetchOnce({
      status: 500,
      body: { error: "internal", message: "boom" },
    });
    const { wrapper, composable } = await mountWithFeatures();

    await flushPromises();

    expect(composable.features.value).toEqual({
      prometheus: false,
      registry: false,
    });
    wrapper.unmount();
  });

  it("refresh() re-fetches features", async () => {
    // First mount returns prometheus=false.
    mockFetchOnce({ body: { prometheus: false, registry: false } });
    const { wrapper, composable } = await mountWithFeatures();
    await flushPromises();
    expect(composable.features.value?.prometheus).toBe(false);

    // Now mock a second fetch that flips prometheus to true.
    mockFetchOnce({ body: { prometheus: true, registry: true } });
    await composable.refresh();
    await flushPromises();

    expect(composable.features.value).toEqual({
      prometheus: true,
      registry: true,
    });
    wrapper.unmount();
  });
});
