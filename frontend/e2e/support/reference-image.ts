import { readFileSync } from "node:fs";
import { createRequire } from "node:module";

/**
 * The visual baselines are rendered in the official Playwright Docker image
 * for the installed @playwright/test version (mcr.microsoft.com/playwright:
 * v<version>-noble, linux/amd64). Fonts and rasterization differ on every
 * other platform, so screenshots taken anywhere else cannot be compared with
 * them.
 *
 * The image describes itself in /ms-playwright/.docker-info.
 */
export function inReferenceImage(): boolean {
  if (process.platform !== "linux" || process.arch !== "x64") return false;
  try {
    const info = JSON.parse(readFileSync("/ms-playwright/.docker-info", "utf8")) as {
      driverVersion?: string;
      dockerImageName?: string;
    };
    const installed = (createRequire(import.meta.url)("@playwright/test/package.json") as { version: string }).version;
    return info.driverVersion === installed && /-noble$/.test(info.dockerImageName ?? "");
  } catch {
    return false;
  }
}

/**
 * `E2E_VISUAL_FORCE=1` runs the visual specs anywhere (useful to look at the
 * pages while writing a spec). Their screenshots then go to .e2e-local/,
 * never to the committed baselines.
 */
export const forceVisual = process.env.E2E_VISUAL_FORCE === "1";
