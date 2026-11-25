import { test as base, useMocks } from "../fixtures";
import { THEMES, type Theme } from "../support/app";
import { forceVisual, inReferenceImage } from "../support/reference-image";

export { expect } from "../fixtures";
export { THEMES, type Theme };

/**
 * `test` for the visual specs. Screenshots are only meaningful inside the
 * pinned Playwright image (see support/reference-image.ts): elsewhere the
 * specs skip, except in CI, where running outside the image is a setup error.
 */
export const test = base.extend<{ referenceImage: void }>({
  referenceImage: [
    async ({}, use, testInfo) => {
      if (!useMocks) testInfo.skip(true, "Visual specs need the API mocks (PLAYWRIGHT_MOCK_MODE=off is set).");
      if (!inReferenceImage() && !forceVisual) {
        if (process.env.CI) {
          throw new Error("Visual specs must run in mcr.microsoft.com/playwright:v<version>-noble (linux/amd64).");
        }
        testInfo.skip(true, "Visual specs run only in the Playwright Docker image: pnpm test:e2e:docker");
      }
      await use();
    },
    { auto: true },
  ],
});

/** Declare the tests in `body` once per theme (dark and light). */
export function eachTheme(body: (theme: Theme) => void): void {
  for (const theme of THEMES) {
    test.describe(`${theme} theme`, () => {
      test.use({ theme });
      body(theme);
    });
  }
}
