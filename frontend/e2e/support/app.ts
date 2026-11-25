import { expect, type Locator, type Page, type PageAssertionsToHaveScreenshotOptions } from "@playwright/test";
import { FIXED_NOW } from "./data";
import type { MockApi } from "./mock-api";

export type Theme = "dark" | "light";

export const THEMES: readonly Theme[] = ["dark", "light"];
export const THEME_NAMES: Record<Theme, string> = { dark: "deckwatchDark", light: "deckwatchLight" };

/** Viewport every test uses unless it overrides `viewport`. */
export const DEFAULT_VIEWPORT = { width: 1440, height: 900 };

export interface SnapOptions {
  /** Screenshot only this element instead of the viewport. */
  locator?: Locator;
  /** Capture the whole page (see growViewportToContent) instead of the viewport. */
  fullPage?: boolean;
  /** Extra elements to paint over (inherently variable content). */
  mask?: Locator[];
  /** Keep an open tooltip in the shot instead of waiting for it to close. */
  allowTooltip?: boolean;
}

/**
 * Runs in the page. "network": no fetch in flight (counted in fixtures.ts)
 * and the web fonts loaded. "rendering": no running animation and no Vue
 * transition in progress. "all": both at once.
 */
const pageIdle = (what: "network" | "rendering" | "all"): boolean => {
  const network =
    !(window as unknown as { __e2eFetchesInFlight?: number }).__e2eFetchesInFlight &&
    document.fonts.status === "loaded";
  if (what === "network") return network;
  const rendering =
    !document.getAnimations().some((a) => a.playState === "running" || a.pending) &&
    !document.querySelector('[class*="-enter-active"], [class*="-leave-active"]');
  return what === "rendering" ? rendering : network && rendering;
};

/**
 * Helpers shared by the functional and visual specs: navigation that keeps
 * the frozen clock, waiting for the UI to settle, and screenshots.
 */
export class App {
  /** Uncaught exceptions and unhandled rejections seen on the page. */
  readonly pageErrors: Error[] = [];
  /** Set in a test that expects the page to throw. */
  allowPageErrors = false;
  /** The page's timers are paused (freezeTimers) until the next snap(). */
  private frozen = false;

  constructor(
    readonly page: Page,
    readonly api: MockApi,
    readonly theme: Theme,
  ) {
    page.on("pageerror", (error) => this.pageErrors.push(error));
  }

  /**
   * Navigate and wait until the app has rendered and gone quiet.
   *
   * The clock is re-frozen before every navigation: Playwright replays its
   * clock log (including the pause/resume around each screenshot) in every
   * new document, and that replay would otherwise unfreeze `Date`.
   */
  async goto(path: string): Promise<void> {
    await this.page.clock.setFixedTime(FIXED_NOW);
    await this.page.goto(path);
    await expect(this.page.locator("#app > *").first()).toBeAttached();
    await this.settle();
  }

  async reload(): Promise<void> {
    await this.page.clock.setFixedTime(FIXED_NOW);
    await this.page.reload();
    await expect(this.page.locator("#app > *").first()).toBeAttached();
    await this.settle();
  }

  /**
   * Wait until nothing is still changing: no fetch in flight (counted in the
   * page, see fixtures.ts) and the mocked API quiet, web fonts loaded, no
   * running animation or Vue transition, then two frames for Vue to flush.
   */
  async settle(): Promise<void> {
    await this.api.waitForQuiet();
    if (this.frozen) {
      // requestAnimationFrame is paused with the other timers, and Vue ends
      // its transitions on animation frames: step the clock a frame at a
      // time instead of waiting.
      await this.page.waitForFunction(pageIdle, "network" as const);
      for (let frames = 0; !(await this.page.evaluate(pageIdle, "rendering" as const)); frames++) {
        if (frames === 60) throw new Error("the page kept changing for 60 frames while its timers were frozen");
        await this.page.clock.runFor(17);
      }
      await this.page.clock.runFor(34);
    } else {
      await this.page.waitForFunction(pageIdle, "all" as const);
      await this.page.evaluate(
        () => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))),
      );
    }
    await this.api.waitForQuiet();
  }

  /**
   * Pause the page's timers now, before a screenshot of something that
   * closes itself: a snackbar (3s) or a cluster-alert toast (5s). Timers
   * otherwise run in real time, and on a loaded machine the element could
   * close between the assertion that it is visible and the capture.
   * snap() resumes them after the screenshot.
   */
  async freezeTimers(): Promise<void> {
    await this.page.clock.pauseAt(FIXED_NOW);
    this.frozen = true;
    await this.settle();
  }

  /**
   * Let a dialog with an autofocus field finish opening, then make sure that
   * field has the focus.
   *
   * Vuetify 3 focuses an autofocus field as soon as it is visible. Vuetify 4
   * defers it by 50ms (to wait out the dialog's enter transition) and, when
   * the page is busy, can lose it because the dialog is still hidden. The
   * pending timers are run first, so the deferred focus cannot land after
   * the test has moved on; then the field is focused if it isn't already.
   */
  async autofocused(field: Locator): Promise<void> {
    await this.page.clock.runFor(100);
    await this.settle();
    if (!(await field.evaluate((el) => el === document.activeElement))) {
      await field.focus();
      await this.settle();
    }
  }

  /**
   * Advance the frozen clock's timers by `ms` (polling intervals fire, `Date`
   * stays at FIXED_NOW), then settle. Use it to get a deterministic number of
   * metric samples instead of waiting in real time.
   */
  async runTimers(ms: number): Promise<void> {
    await this.page.clock.runFor(ms);
    await this.settle();
  }

  /**
   * Compare a screenshot with its baseline in e2e/__screenshots__.
   *
   * The file name gets the theme and, for non-default widths, the viewport
   * width. Playwright turns the "/" into "-", so `snap("resources/deployments")`
   * in the dark theme at 1440px is `resources-deployments-dark.png` in the
   * spec's folder under e2e/__screenshots__.
   *
   * Before the shot the mouse is parked in the top-left corner (the app bar
   * background, or the scrim when an overlay is open). The pointer would
   * otherwise rest on the last clicked control, and whether Chromium has
   * applied its hover state by then varies from run to run. Timers are
   * paused while the screenshot is taken, so a polling refresh cannot flash
   * a progress bar into the capture.
   */
  async snap(name: string, options: SnapOptions = {}): Promise<void> {
    await this.page.mouse.move(0, 0);
    if (!options.allowTooltip) {
      await expect(this.page.locator(".v-tooltip.v-overlay--active")).toHaveCount(0);
    }
    await this.settle();
    await this.normalizeVariableText();
    const width = this.page.viewportSize()?.width ?? DEFAULT_VIEWPORT.width;
    const suffix = width === DEFAULT_VIEWPORT.width ? `-${this.theme}` : `-w${width}-${this.theme}`;
    const screenshotOptions: PageAssertionsToHaveScreenshotOptions = { mask: options.mask };
    const restore = options.fullPage ? await this.growViewportToContent() : null;
    await this.page.clock.pauseAt(FIXED_NOW);
    try {
      if (options.locator) {
        await expect(options.locator).toHaveScreenshot(`${name}${suffix}.png`, screenshotOptions);
      } else {
        await expect(this.page).toHaveScreenshot(`${name}${suffix}.png`, screenshotOptions);
      }
    } finally {
      await this.page.clock.resume();
      this.frozen = false;
      if (restore) {
        await this.page.setViewportSize(restore);
        await this.settle();
      }
    }
  }

  /**
   * The app scrolls `<body>`, not the document (index.css gives html and body
   * `height: 100%; overflow: auto`), so Playwright's `fullPage` capture would
   * be cut off at the viewport height. Instead make the viewport as tall as
   * the content, and return the original size to restore afterwards.
   */
  private async growViewportToContent(): Promise<{ width: number; height: number } | null> {
    const original = this.page.viewportSize() ?? DEFAULT_VIEWPORT;
    let height = original.height;
    // Content can grow with the viewport (min-height: 100vh), so re-measure.
    for (let i = 0; i < 3; i++) {
      const content = await this.page.evaluate(() =>
        Math.ceil(Math.max(document.body.scrollHeight, document.documentElement.scrollHeight)),
      );
      if (content <= height) break;
      height = content;
      await this.page.setViewportSize({ width: original.width, height });
      await this.settle();
    }
    return height === original.height ? null : original;
  }

  /**
   * The footer shows the package.json version, which changes every release.
   * Replace it with a fixed string so a version bump (or a longer version
   * number) never shows up as a visual diff. Styling is untouched.
   */
  private async normalizeVariableText(): Promise<void> {
    await this.page.evaluate(() => {
      for (const el of document.querySelectorAll("footer span")) {
        if (/^Deckwatch v/.test(el.textContent ?? "")) el.textContent = "Deckwatch v0.0.0";
      }
    });
  }

  /**
   * The namespace switcher's text input in the app bar. Found by role, not
   * as "the input": Vuetify 4 also renders a hidden input inside it.
   */
  namespaceSwitcher(): Locator {
    return this.page.getByTestId("namespace-switcher").getByRole("combobox", { name: "Namespace", exact: true });
  }

  /** The open overlay content (dialog, menu, drawer...) that is on top. */
  topOverlay(): Locator {
    return this.page.locator(".v-overlay--active .v-overlay__content").last();
  }

  /** The open dialog with this title. */
  dialog(title: string | RegExp): Locator {
    return this.page.getByRole("dialog").filter({ has: this.page.locator(".v-card-title", { hasText: title }) });
  }
}
