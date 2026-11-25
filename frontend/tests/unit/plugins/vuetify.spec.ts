import { describe, expect, it } from "vitest";
import { vuetify } from "@/plugins/vuetify";

// The runtime half of the Vuetify 3 compatibility settings
// (src/plugins/vuetify.ts). vite.config.ts rewrites the CSS breakpoints to
// the same values; tests/unit/build covers that side.

describe("Vuetify instance", () => {
  it("uses Vuetify 3's display breakpoints, matching the CSS rewrite", () => {
    expect(vuetify.display.thresholds.value).toEqual({ xs: 0, sm: 600, md: 960, lg: 1280, xl: 1920, xxl: 2560 });
  });

  it("gives the slider thumb its Vuetify 3 elevation", () => {
    expect(vuetify.defaults.value?.VSlider).toEqual({ elevation: 2 });
  });
});
