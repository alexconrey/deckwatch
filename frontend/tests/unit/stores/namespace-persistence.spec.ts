import { describe, expect, it, beforeEach, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useNamespaceStore } from "@/stores/namespace";
import { mockFetchOnce } from "../helpers/mockFetch";
import { nextTick } from "vue";

const STORAGE_KEY = "deckwatch-namespace";

describe("useNamespaceStore – localStorage persistence", () => {
  let storage: Record<string, string>;

  beforeEach(() => {
    setActivePinia(createPinia());

    // Provide a minimal in-memory localStorage stub so tests don't
    // depend on happy-dom's implementation details.
    storage = {};
    vi.stubGlobal("localStorage", {
      getItem: vi.fn((key: string) => storage[key] ?? null),
      setItem: vi.fn((key: string, val: string) => {
        storage[key] = val;
      }),
      removeItem: vi.fn((key: string) => {
        delete storage[key];
      }),
    });
  });

  it("saves the selected namespace to localStorage", async () => {
    mockFetchOnce({ body: { namespaces: ["default", "team-a"] } });
    const s = useNamespaceStore();
    await s.fetchNamespaces();

    // fetchNamespaces auto-selects "default" (nothing stored).
    // The Vue watcher should fire and persist.
    await nextTick();
    expect(localStorage.setItem).toHaveBeenCalledWith(
      STORAGE_KEY,
      "default",
    );
  });

  it("manually selecting a namespace persists it", async () => {
    mockFetchOnce({ body: { namespaces: ["default", "team-a", "team-b"] } });
    const s = useNamespaceStore();
    await s.fetchNamespaces();
    await nextTick();

    s.selected = "team-b";
    await nextTick();

    expect(localStorage.setItem).toHaveBeenCalledWith(
      STORAGE_KEY,
      "team-b",
    );
  });

  it("restores the stored namespace on fetch when it is in the list", async () => {
    storage[STORAGE_KEY] = "team-a";
    mockFetchOnce({ body: { namespaces: ["default", "team-a", "team-b"] } });
    const s = useNamespaceStore();
    await s.fetchNamespaces();

    expect(s.selected).toBe("team-a");
  });

  it("falls back to the first namespace when the stored one is not in the list", async () => {
    storage[STORAGE_KEY] = "deleted-ns";
    mockFetchOnce({ body: { namespaces: ["default", "team-a"] } });
    const s = useNamespaceStore();
    await s.fetchNamespaces();

    expect(s.selected).toBe("default");
  });

  it("ignores localStorage when the list is empty", async () => {
    storage[STORAGE_KEY] = "stale-ns";
    mockFetchOnce({ body: { namespaces: [] } });
    const s = useNamespaceStore();
    await s.fetchNamespaces();

    expect(s.selected).toBe("");
  });

  it("does not read localStorage when a namespace is already selected", async () => {
    storage[STORAGE_KEY] = "team-a";
    mockFetchOnce({ body: { namespaces: ["default", "team-a"] } });
    const s = useNamespaceStore();
    s.selected = "default";
    await s.fetchNamespaces();

    // The store should keep "default" because selected was already set,
    // not switch to "team-a" from localStorage.
    expect(s.selected).toBe("default");
  });
});
