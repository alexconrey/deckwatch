import { describe, expect, it } from "vitest";
import { formatMemory } from "@/utils/format";

describe("formatMemory", () => {
  describe("IEC binary suffixes", () => {
    it("formats Ki as KiB for small values", () => {
      expect(formatMemory("512Ki")).toBe("512 KiB");
    });

    it("formats Mi as MiB", () => {
      expect(formatMemory("256Mi")).toBe("256 MiB");
    });

    it("formats Gi as GiB", () => {
      expect(formatMemory("2Gi")).toBe("2.0 GiB");
    });

    it("formats Ti as TiB", () => {
      expect(formatMemory("1Ti")).toBe("1.0 TiB");
    });

    it("auto-promotes large Ki values to MiB", () => {
      // 2048 Ki = 2 MiB
      expect(formatMemory("2048Ki")).toBe("2 MiB");
    });

    it("auto-promotes large Ki values to GiB", () => {
      // 8123456 Ki = ~7.7 GiB
      expect(formatMemory("8123456Ki")).toBe("7.7 GiB");
    });

    it("auto-promotes large Mi values to GiB", () => {
      // 2048 Mi = 2 GiB
      expect(formatMemory("2048Mi")).toBe("2.0 GiB");
    });
  });

  describe("SI decimal suffixes", () => {
    it("formats K as KB", () => {
      expect(formatMemory("500K")).toBe("500 KB");
    });

    it("formats M as MB", () => {
      expect(formatMemory("500M")).toBe("500 MB");
    });

    it("formats G as GB", () => {
      expect(formatMemory("4G")).toBe("4.0 GB");
    });

    it("formats T as TB", () => {
      expect(formatMemory("2T")).toBe("2.0 TB");
    });
  });

  describe("plain byte counts", () => {
    it("auto-converts bytes to GiB for large values", () => {
      // 1073741824 bytes = 1 GiB
      expect(formatMemory("1073741824")).toBe("1.0 GiB");
    });

    it("auto-converts bytes to MiB", () => {
      // 128974848 bytes = ~123 MiB
      expect(formatMemory("128974848")).toBe("123 MiB");
    });

    it("auto-converts bytes to KiB for medium values", () => {
      // 2048 bytes = 2 KiB
      expect(formatMemory("2048")).toBe("2 KiB");
    });

    it("keeps small byte counts as bytes", () => {
      expect(formatMemory("512")).toBe("512 B");
    });

    it("renders very small byte counts as B", () => {
      expect(formatMemory("42")).toBe("42 B");
    });
  });

  describe("null / undefined / empty", () => {
    it("returns dash for null", () => {
      expect(formatMemory(null)).toBe("-");
    });

    it("returns dash for undefined", () => {
      expect(formatMemory(undefined)).toBe("-");
    });

    it("returns dash for empty string", () => {
      expect(formatMemory("")).toBe("-");
    });

    it("returns dash for whitespace-only string", () => {
      expect(formatMemory("   ")).toBe("-");
    });
  });

  describe("pass-through for unrecognized formats", () => {
    it("returns already-formatted strings unchanged", () => {
      expect(formatMemory("1.5 GiB")).toBe("1.5 GiB");
    });
  });
});
