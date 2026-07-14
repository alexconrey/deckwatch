import "vuetify/styles";
import { createVuetify, type ThemeDefinition } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";

const deckwatchTheme: ThemeDefinition = {
  dark: true,
  colors: {
    background: "#0d1117",
    surface: "#161b22",
    "surface-variant": "#21262d",
    "on-surface-variant": "#c9d1d9",
    primary: "#58a6ff",
    secondary: "#8b949e",
    error: "#f85149",
    warning: "#d29922",
    success: "#3fb950",
    info: "#58a6ff",
  },
};

export const vuetify = createVuetify({
  components,
  directives,
  theme: {
    defaultTheme: "deckwatchTheme",
    themes: {
      deckwatchTheme,
    },
  },
});
