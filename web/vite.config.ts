import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import type { Plugin } from "vite";
import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { svelteTesting } from "@testing-library/svelte/vite";
import { mockServerPlugin } from "./mock/plugin";

const isMock = process.env.VITE_MOCK === "1";
// Vitest sets this before loading the config. HMR stays on for `vite dev`.
const isTest = Boolean(process.env.VITEST);

// Lib tests stay in Node. Mounting a `.svelte` file needs a DOM, so those
// files opt into jsdom by where they live. jsdom is what
// `@testing-library/svelte`'s setup guide installs. jsdom 29 is the newest
// major whose engines still include Node ^22.13; jsdom 30 requires ^22.22.2
// or ^24.15.0.
const componentTests = ["src/components/**/*.test.ts", "src/**/*.component.test.ts"];

/** The bundled fonts' OFL licenses, which must accompany every copy of the font files. */
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

export default defineConfig({
  plugins: [
    svelte(isTest ? { compilerOptions: { hmr: false } } : {}),
    fontLicenses(),
    ...(isMock ? [mockServerPlugin()] : []),
  ],
  build: {
    outDir: "dist",
    // Font subsets stay separate files: inlined into the CSS, every subset would
    // download on load instead of when its unicode-range is first rendered.
    assetsInlineLimit: (file) => (file.endsWith(".woff2") ? false : undefined),
  },
  server: {
    ...(!isMock && {
      proxy: {
        // Carries the agent and hub WebSockets too (`/api/agents/{name}/ws`, `/api/hub/ws`).
        "/api": {
          target: "http://localhost:7700",
          ws: true,
        },
      },
    }),
  },
  test: {
    // Reported in CI's job summary, with no threshold. `npm run test:coverage` runs it locally.
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
          include: ["src/**/*.test.ts", "mock/**/*.test.ts"],
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
});
