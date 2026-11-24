// Tests for the shared `formatAge` helper in `src/utils/format.ts`.
//
// The signature is `formatAge(iso, options, now)`. Every call here passes the
// fixed `now` as the third argument so results never depend on the wall
// clock. If `now` is omitted, or passed in the `options` slot, the helper
// falls back to `Date.now()` and these expectations become time-dependent.
import { describe, expect, it } from "vitest";
import { formatAge } from "@/utils/format";

describe("formatAge", () => {
  const now = new Date("2026-07-10T12:00:00Z").getTime();

  it("returns - for null input", () => {
    expect(formatAge(null, {}, now)).toBe("-");
  });

  it("formats minutes when under 1 hour", () => {
    const t = new Date(now - 5 * 60 * 1000).toISOString();
    expect(formatAge(t, {}, now)).toBe("5m");
  });

  it("formats hours when under 24 hours", () => {
    const t = new Date(now - 3 * 60 * 60 * 1000).toISOString();
    expect(formatAge(t, {}, now)).toBe("3h");
  });

  it("formats days when over 24 hours", () => {
    const t = new Date(now - 5 * 24 * 60 * 60 * 1000).toISOString();
    expect(formatAge(t, {}, now)).toBe("5d");
  });

  it("rounds down to the nearest whole unit", () => {
    // 59 seconds ago → 0m
    const t = new Date(now - 59_000).toISOString();
    expect(formatAge(t, {}, now)).toBe("0m");
  });

  it("handles the 1-hour boundary", () => {
    const t = new Date(now - 60 * 60 * 1000).toISOString();
    expect(formatAge(t, {}, now)).toBe("1h");
  });

  it("handles the 24-hour boundary", () => {
    const t = new Date(now - 24 * 60 * 60 * 1000).toISOString();
    expect(formatAge(t, {}, now)).toBe("1d");
  });
});
