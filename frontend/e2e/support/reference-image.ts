import { existsSync, readFileSync } from "node:fs";

/**
 * The visual baselines are rendered in the official Playwright Docker image
 * (mcr.microsoft.com/playwright:v<version>-noble, linux/amd64). Fonts and
 * rasterization differ on every other platform, so screenshots taken
 * anywhere else cannot be compared with them.
 */
export function inReferenceImage(): boolean {
  if (process.platform !== "linux" || process.arch !== "x64") return false;
  if (process.env.PLAYWRIGHT_BROWSERS_PATH !== "/ms-playwright" || !existsSync("/ms-playwright")) return false;
  try {
    return /VERSION_CODENAME=noble/.test(readFileSync("/etc/os-release", "utf8"));
  } catch {
    return false;
  }
}

/**
 * `E2E_VISUAL_FORCE=1` runs the visual specs anywhere (useful to look at the
 * pages while writing a spec). Their screenshots then go to test-results/,
 * never to the committed baselines.
 */
export const forceVisual = process.env.E2E_VISUAL_FORCE === "1";
