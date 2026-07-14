// Global test setup for Vitest.
//
// - Resets fetch mocks between tests so cross-test bleed can't hide bugs.
// - Silences Vue Router warnings that don't apply in isolated unit tests.
import { afterEach, beforeEach, vi } from "vitest";

beforeEach(() => {
  // Every test starts from a clean fetch stub. Individual tests can
  // override with `vi.spyOn(globalThis, "fetch").mockImplementation(...)`.
  vi.stubGlobal("fetch", vi.fn());
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});
