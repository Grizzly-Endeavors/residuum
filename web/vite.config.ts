import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import type { Plugin } from "vite";
import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { svelteTesting } from "@testing-library/svelte/vite";
import { serviceWorkerPlugin } from "./build/service-worker";
import { mockServerPlugin } from "./mock/plugin";
import { HANG_GUARD_MS } from "./src/test/guard";

const isMock = process.env.VITE_MOCK === "1";
const isTest = Boolean(process.env.VITEST);

// Lib tests stay in Node. Mounting a `.svelte` file needs a DOM, so those
// files opt into jsdom by where they live. jsdom is what
// `@testing-library/svelte`'s setup guide installs. jsdom 29 is the newest
// major whose engines still include Node ^22.13; jsdom 30 requires ^22.22.2
// or ^24.15.0.
const componentTests = ["src/components/**/*.test.ts", "src/**/*.component.test.ts"];

function fontLicenses(): Plugin {
  const require = createRequire(import.meta.url);
  return {
    name: "font-licenses",
    apply: "build",
    generateBundle() {
      for (const font of ["onest", "jetbrains-mono", "cinzel"]) {
        this.emitFile({
          type: "asset",
          fileName: `licenses/${font}-OFL.txt`,
          source: readFileSync(require.resolve(`@fontsource/${font}/LICENSE`), "utf8"),
        });
      }
    },
  };
}

export default defineConfig(({ command }) => ({
  ...(process.env.VITE_CACHE_DIR && { cacheDir: process.env.VITE_CACHE_DIR }),
  define: {
    __UI_GALLERY__: JSON.stringify(command === "serve" || isMock),
    __SERVICE_WORKER__: JSON.stringify(command === "build"),
  },
  plugins: [
    svelte(isTest ? { compilerOptions: { hmr: false } } : {}),
    fontLicenses(),
    serviceWorkerPlugin(),
    ...(isMock ? [mockServerPlugin()] : []),
  ],
  build: {
    outDir: "dist",
    assetsInlineLimit: (file) => (file.endsWith(".woff2") ? false : undefined),
  },
  server: {
    ...(!isMock && {
      proxy: {
        "/api": {
          target: "http://localhost:7700",
          ws: true,
        },
      },
    }),
  },
  test: {
    testTimeout: HANG_GUARD_MS,
    hookTimeout: HANG_GUARD_MS,
    fsModuleCache: true,
    coverage: {
      provider: "v8",
      reportsDirectory: "coverage",
      include: ["src/**/*.{ts,svelte}"],
      exclude: ["src/**/*.test.ts", "src/test/**", "src/lib/generated/**"],
      reporter: ["text-summary", ["text-summary", { file: "summary.txt" }], "html"],
    },
    projects: [
      {
        extends: true,
        test: {
          name: "unit",
          environment: "node",
          include: ["src/**/*.test.ts", "build/**/*.test.ts", "mock/**/*.test.ts", "scripts/**/*.test.ts"],
          exclude: componentTests,
        },
      },
      {
        extends: true,
        plugins: [svelteTesting({ autoCleanup: false })],
        test: {
          name: "components",
          environment: "jsdom",
          include: componentTests,
          setupFiles: ["./src/test/setup.ts"],
        },
      },
    ],
  },
}));
