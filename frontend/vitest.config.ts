import { defineConfig } from "vitest/config";
import vue from "@vitejs/plugin-vue";
import vuetify from "vite-plugin-vuetify";
import { fileURLToPath, URL } from "node:url";

export default defineConfig({
  plugins: [vue(), vuetify({ autoImport: true })],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  test: {
    environment: "happy-dom",
    // Node 25+ puts its own localStorage/sessionStorage on globalThis, and
    // they are undefined unless --localstorage-file is set. Vitest 4 then
    // keeps them instead of happy-dom's Storage (vitest-dev/vitest#10867,
    // fixed in Vitest 5), so turn Node's off in test workers. Node 24 also
    // accepts this spelling (it would reject --no-webstorage). Remove this
    // after upgrading to Vitest 5.
    execArgv: ["--no-experimental-webstorage"],
    server: {
      deps: {
        inline: ["vuetify"],
      },
    },
    globals: true,
    include: ["tests/unit/**/*.spec.ts"],
    coverage: {
      provider: "v8",
      reporter: ["text", "html", "lcov"],
      include: [
        "src/api/**/*.ts",
        "src/stores/**/*.ts",
        "src/composables/**/*.ts",
      ],
      exclude: ["src/**/*.d.ts", "src/main.ts", "src/env.d.ts"],
      thresholds: {
        lines: 70,
        functions: 70,
        branches: 60,
        statements: 70,
      },
    },
    setupFiles: ["./tests/unit/setup.ts"],
  },
});
