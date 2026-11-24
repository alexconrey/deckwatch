import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import vuetify from "vite-plugin-vuetify";
import { fileURLToPath, URL } from "node:url";
import { readFileSync } from "node:fs";

const pkg = JSON.parse(readFileSync("./package.json", "utf-8"));

export default defineConfig({
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
  plugins: [vue(), vuetify({ autoImport: true })],
  // Dev-server only. Vuetify 4 stacks overlays (a select menu opened inside a
  // dialog) through a shared singleton. If Vite serves some Vuetify modules
  // from its pre-bundle cache and others as raw ESM, two copies exist and
  // menus render behind dialogs. Pre-bundling these up front keeps one copy.
  // https://vuetifyjs.com/en/getting-started/upgrade-guide/#vite-overlay-z-index-in-dev-mode
  optimizeDeps: {
    include: [
      "vuetify/components/VOverlay",
      "vuetify/components/VDialog",
      "vuetify/components/VMenu",
      "vuetify/components/VSelect",
      "vuetify/components/VTooltip",
    ],
  },
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  server: {
    port: 3000,
    proxy: {
      "/api": {
        target: "http://localhost:8080",
        changeOrigin: true,
      },
    },
  },
});
