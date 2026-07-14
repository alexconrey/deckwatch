import "vuetify/styles";
import { createVuetify, type ThemeDefinition } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";

const deckwatchDark: ThemeDefinition = {
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

const deckwatchLight: ThemeDefinition = {
  dark: false,
  colors: {
    background: "#ffffff",
    surface: "#f5f5f5",
    "surface-variant": "#e0e0e0",
    "on-surface-variant": "#424242",
    primary: "#1976d2",
    secondary: "#757575",
    error: "#d32f2f",
    warning: "#f9a825",
    success: "#388e3c",
    info: "#1976d2",
  },
};

const savedTheme = localStorage.getItem("deckwatch-theme");
const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
const defaultTheme = savedTheme || (prefersDark ? "deckwatchDark" : "deckwatchLight");

export const vuetify = createVuetify({
  components,
  directives,
  theme: {
    defaultTheme,
    themes: {
      deckwatchDark,
      deckwatchLight,
    },
  },
});
