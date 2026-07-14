import { describe, expect, it, beforeEach } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useNamespaceStore } from "@/stores/namespace";
import { mockFetchOnce } from "../helpers/mockFetch";

describe("useNamespaceStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("initializes with empty state", () => {
    const s = useNamespaceStore();
    expect(s.namespaces).toEqual([]);
    expect(s.selected).toBe("");
    expect(s.loading).toBe(false);
    expect(s.error).toBeNull();
  });

  it("fetchNamespaces populates the list and auto-selects the first entry", async () => {
    mockFetchOnce({ body: { namespaces: ["default", "team-a", "team-b"] } });
    const s = useNamespaceStore();
    await s.fetchNamespaces();
    expect(s.namespaces).toEqual(["default", "team-a", "team-b"]);
    expect(s.selected).toBe("default");
    expect(s.loading).toBe(false);
    expect(s.error).toBeNull();
  });

  it("preserves an already-selected namespace even after re-fetch", async () => {
    mockFetchOnce({ body: { namespaces: ["default", "team-a"] } });
    const s = useNamespaceStore();
    s.selected = "team-a";
    await s.fetchNamespaces();
    expect(s.selected).toBe("team-a");
  });

  it("does not auto-select when the list is empty", async () => {
    mockFetchOnce({ body: { namespaces: [] } });
    const s = useNamespaceStore();
    await s.fetchNamespaces();
    expect(s.selected).toBe("");
  });

  it("sets error and clears loading on API failure", async () => {
    mockFetchOnce({
      status: 500,
      body: { error: "kube_error", message: "boom" },
    });
    const s = useNamespaceStore();
    await s.fetchNamespaces();
    expect(s.error).toBe("boom");
    expect(s.loading).toBe(false);
    expect(s.namespaces).toEqual([]);
  });
});
