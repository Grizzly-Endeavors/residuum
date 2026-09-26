import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { svelteTesting } from "@testing-library/svelte/vite";
import { mockServerPlugin } from "./mock-server";

const isMock = process.env.VITE_MOCK === "1";
// Vitest sets this before loading the config. HMR stays on for `vite dev`.
const isTest = Boolean(process.env.VITEST);

// Lib tests stay in Node. Mounting a `.svelte` file needs a DOM, so those
// files opt into jsdom by where they live. jsdom is what
// `@testing-library/svelte`'s setup guide installs. jsdom 29 is the newest
// major whose engines still include Node ^22.13; jsdom 30 requires ^22.22.2
// or ^24.15.0.
const componentTests = ["src/components/**/*.test.ts", "src/**/*.component.test.ts"];

export default defineConfig({
  plugins: [
    svelte(isTest ? { compilerOptions: { hmr: false } } : {}),
    ...(isMock ? [mockServerPlugin()] : []),
  ],
  build: {
    outDir: "dist",
  },
  server: {
    ...(!isMock && {
      proxy: {
        "/api": "http://localhost:7700",
        "/ws": {
          target: "http://localhost:7700",
          ws: true,
        },
      },
    }),
  },
  test: {
    projects: [
      {
        extends: true,
        test: {
          name: "unit",
          environment: "node",
          include: ["src/**/*.test.ts"],
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
