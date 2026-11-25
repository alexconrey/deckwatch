import { defineConfig, type Plugin } from "vite";
import vue from "@vitejs/plugin-vue";
import vuetify from "vite-plugin-vuetify";
import { fileURLToPath, URL } from "node:url";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";

const pkg = JSON.parse(readFileSync("./package.json", "utf-8"));

/**
 * Declare Vuetify 4's cascade-layer order before any stylesheet loads.
 *
 * Layer order is fixed by where each layer first appears in the document.
 * Vite splits the CSS of shared chunks (VBtn, VTextField, ...) into their
 * own files and links them in index.html *before* the entry CSS, which is
 * where main.ts's `import "vuetify/styles/layers"` ends up. Those chunk files
 * mention vuetify-components and vuetify-final first, so vuetify-core (the
 * reset) would rank last and win: `button { font: inherit }` beats .v-btn,
 * and the reset's `body { overflow: auto }` beats the trumps layer's
 * scroll lock while a dialog is open. Inlining Vuetify's own layers.css at
 * the top of <head> puts the declaration first in every build.
 */
function vuetifyLayerOrder(): Plugin {
  const layers = readFileSync(createRequire(import.meta.url).resolve("vuetify/styles/layers"), "utf-8");
  return {
    name: "deckwatch:vuetify-layer-order",
    transformIndexHtml: () => [{ tag: "style", children: layers, injectTo: "head-prepend" }],
  };
}

export default defineConfig({
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
  plugins: [vuetifyLayerOrder(), vue(), vuetify({ autoImport: true })],
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
