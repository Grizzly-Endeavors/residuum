import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { mockServerPlugin } from "./mock-server";

const isMock = process.env.VITE_MOCK === "1";

export default defineConfig({
  plugins: [svelte(), ...(isMock ? [mockServerPlugin()] : [])],
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
});
