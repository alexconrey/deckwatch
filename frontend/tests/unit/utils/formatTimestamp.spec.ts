import { describe, expect, it } from "vitest";
import { formatTimestamp } from "@/utils/format";

describe("formatTimestamp", () => {
  it("returns dash for null", () => {
    expect(formatTimestamp(null)).toBe("-");
  });

  it("formats an ISO-8601 string via toLocaleString", () => {
    const iso = "2026-07-10T15:30:00Z";
    const result = formatTimestamp(iso);
    // toLocaleString output varies by locale/TZ, so verify it is non-empty
    // and not the dash sentinel.
    expect(result).not.toBe("-");
    expect(result.length).toBeGreaterThan(0);
  });

  it("returns a string containing date components for a known timestamp", () => {
    const iso = "2025-01-15T08:00:00Z";
    const result = formatTimestamp(iso);
    // The formatted string should include the year somewhere.
    expect(result).toContain("2025");
  });

  it("handles an ISO string with milliseconds", () => {
    const iso = "2026-03-05T12:34:56.789Z";
    const result = formatTimestamp(iso);
    expect(result).not.toBe("-");
    expect(result).toContain("2026");
  });

  it("handles an ISO string with timezone offset", () => {
    const iso = "2026-06-01T20:00:00+05:30";
    const result = formatTimestamp(iso);
    expect(result).not.toBe("-");
  });
});
