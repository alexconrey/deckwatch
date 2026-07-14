import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { defineComponent, h } from "vue";
import { mount } from "@vue/test-utils";
import { usePolling } from "@/composables/usePolling";

/**
 * usePolling relies on onMounted / onUnmounted hooks, so it must be
 * exercised inside a live Vue component instance. We wrap it in a tiny
 * test harness that mounts a component and exposes the polling controls
 * through refs so the test can drive them.
 */
function makeHarness(fn: () => Promise<void>, intervalMs: number) {
  const controls: { stop?: () => void; start?: () => void } = {};
  const Harness = defineComponent({
    setup() {
      const p = usePolling(fn, intervalMs);
      controls.stop = p.stop;
      controls.start = p.start;
      return () => h("div");
    },
  });
  const wrapper = mount(Harness);
  return { wrapper, controls };
}

describe("usePolling", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("runs the callback once immediately on mount", async () => {
    const spy = vi.fn().mockResolvedValue(undefined);
    const { wrapper } = makeHarness(spy, 5000);
    // The immediate call happens synchronously inside start(), but the
    // returned promise is not awaited — flushing microtasks is enough.
    await Promise.resolve();
    expect(spy).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("re-invokes the callback on every interval tick", async () => {
    const spy = vi.fn().mockResolvedValue(undefined);
    const { wrapper } = makeHarness(spy, 1000);
    await Promise.resolve();
    vi.advanceTimersByTime(3000);
    // 1 immediate + 3 timer ticks = 4 calls
    expect(spy).toHaveBeenCalledTimes(4);
    wrapper.unmount();
  });

  it("stops polling when the component unmounts", async () => {
    const spy = vi.fn().mockResolvedValue(undefined);
    const { wrapper } = makeHarness(spy, 1000);
    await Promise.resolve();
    expect(spy).toHaveBeenCalledTimes(1);
    wrapper.unmount();
    vi.advanceTimersByTime(10_000);
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("manually calling stop() halts polling before unmount", async () => {
    const spy = vi.fn().mockResolvedValue(undefined);
    const { wrapper, controls } = makeHarness(spy, 1000);
    await Promise.resolve();
    controls.stop!();
    vi.advanceTimersByTime(10_000);
    expect(spy).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("start() resets an existing timer instead of stacking them", async () => {
    const spy = vi.fn().mockResolvedValue(undefined);
    const { wrapper, controls } = makeHarness(spy, 1000);
    await Promise.resolve();
    // 1 immediate call so far.
    controls.start!(); // triggers another immediate call + fresh interval
    await Promise.resolve();
    expect(spy).toHaveBeenCalledTimes(2);
    vi.advanceTimersByTime(1000);
    // Only one tick should fire per second even after restart (no doubling).
    expect(spy).toHaveBeenCalledTimes(3);
    wrapper.unmount();
  });
});
