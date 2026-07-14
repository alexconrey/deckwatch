// Tests for a shared `formatAge` helper.
//
// NOTE: The `formatAge` function is currently duplicated across
// DeploymentsPage.vue, DeploymentDetailPage.vue, ClusterOverviewPage.vue,
// and PodDetailPage.vue. As a follow-up cleanup, extract it into
// `frontend/src/utils/format.ts` — this test file assumes that module
// exists.
//
// Suggested implementation (extract verbatim from any page component):
//
//   export function formatAge(iso: string | null, now = Date.now()): string {
//     if (!iso) return "-";
//     const diff = now - new Date(iso).getTime();
//     const minutes = Math.floor(diff / 60000);
//     if (minutes < 60) return `${minutes}m`;
//     const hours = Math.floor(minutes / 60);
//     if (hours < 24) return `${hours}h`;
//     const days = Math.floor(hours / 24);
//     return `${days}d`;
//   }
//
// Adding an optional `now` parameter (default Date.now()) makes tests
// deterministic without needing fake timers.
import { describe, expect, it } from "vitest";
import { formatAge } from "@/utils/format";

describe("formatAge", () => {
  const now = new Date("2026-07-10T12:00:00Z").getTime();

  it("returns - for null input", () => {
    expect(formatAge(null, now)).toBe("-");
  });

  it("formats minutes when under 1 hour", () => {
    const t = new Date(now - 5 * 60 * 1000).toISOString();
    expect(formatAge(t, now)).toBe("5m");
  });

  it("formats hours when under 24 hours", () => {
    const t = new Date(now - 3 * 60 * 60 * 1000).toISOString();
    expect(formatAge(t, now)).toBe("3h");
  });

  it("formats days when over 24 hours", () => {
    const t = new Date(now - 5 * 24 * 60 * 60 * 1000).toISOString();
    expect(formatAge(t, now)).toBe("5d");
  });

  it("rounds down to the nearest whole unit", () => {
    // 59 seconds ago → 0m
    const t = new Date(now - 59_000).toISOString();
    expect(formatAge(t, now)).toBe("0m");
  });

  it("handles the 1-hour boundary", () => {
    const t = new Date(now - 60 * 60 * 1000).toISOString();
    expect(formatAge(t, now)).toBe("1h");
  });

  it("handles the 24-hour boundary", () => {
    const t = new Date(now - 24 * 60 * 60 * 1000).toISOString();
    expect(formatAge(t, now)).toBe("1d");
  });
});
