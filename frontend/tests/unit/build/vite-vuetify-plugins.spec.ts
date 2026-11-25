// @vitest-environment node
// (vite.config.ts resolves files from import.meta.url, which needs Node.)
import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import type { HtmlTagDescriptor } from "vite";
import { vuetifyLayerOrder, vuetifyV3Breakpoints } from "../../../vite.config";

// The build-level half of the Vuetify 3 compatibility layer (vite.config.ts).
// The e2e visual suite shows the effect, every page against the Vuetify 3
// screenshots; these pin down the mechanics, so a Vuetify update that moves a
// breakpoint or the layers file fails here first.

const require = createRequire(import.meta.url);

describe("vuetifyV3Breakpoints", () => {
  const transform = vuetifyV3Breakpoints().transform as (code: string, id: string) => string | null;
  const gridCssPath = require.resolve("vuetify/lib/components/VGrid/VGrid.css");

  it("moves the md/lg/xl/xxl media queries back to Vuetify 3's widths", () => {
    const css =
      "@media (min-width: 840px){a{}}@media (max-width: 1144.98px){b{}}" +
      "@media (min-width: 1545px){c{}}@media (min-width:2138px){d{}}";
    expect(transform(css, gridCssPath)).toBe(
      "@media (min-width: 960px){a{}}@media (max-width: 1279.98px){b{}}" +
        "@media (min-width: 1920px){c{}}@media (min-width: 2560px){d{}}",
    );
  });

  it("keeps sm (600px), which Vuetify 4 did not change, and other widths", () => {
    const css = "@media (min-width: 600px){a{}}@media (max-width: 599.98px){b{}}@media (min-width: 841px){c{}}";
    expect(transform(css, gridCssPath)).toBe(css);
  });

  it("leaves CSS that isn't Vuetify's alone", () => {
    expect(transform("@media (min-width: 840px){a{}}", "/app/src/styles/index.css")).toBeNull();
    expect(transform("@media (min-width: 840px){a{}}", "/app/node_modules/other/x.css")).toBeNull();
  });

  it("leaves no Vuetify 4 breakpoint in Vuetify's own grid CSS", () => {
    const out = transform(readFileSync(gridCssPath, "utf8"), gridCssPath)!;
    expect(out).not.toMatch(/\((?:min|max)-width:\s*(?:840|1145|1545|2138|839\.98|1144\.98|1544\.98|2137\.98)px\)/);
    expect(out).toMatch(/\(min-width: 960px\)/);
    expect(out).toMatch(/\(min-width: 2560px\)/);
  });
});

describe("vuetifyLayerOrder", () => {
  it("inlines Vuetify's layer declaration at the very top of <head>", () => {
    const hook = vuetifyLayerOrder().transformIndexHtml as () => HtmlTagDescriptor[];
    const tags = hook();
    expect(tags).toHaveLength(1);
    expect(tags[0]).toMatchObject({ tag: "style", injectTo: "head-prepend" });
    expect(tags[0].children).toBe(readFileSync(require.resolve("vuetify/styles/layers"), "utf8"));
    // The reset layer must be declared before the component layers.
    const declared = String(tags[0].children);
    expect(declared.indexOf("vuetify-core")).toBeGreaterThan(-1);
    expect(declared.indexOf("vuetify-core")).toBeLessThan(declared.indexOf("vuetify-components"));
  });
});
