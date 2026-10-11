// Builds the service worker (`src/sw/worker.ts`) into `sw.js` beside the app.
//
// The worker precaches the build's own files, so it can only be written once
// the build exists: this plugin waits for the bundle and the public directory
// to be in the output directory, reads which files the shell needs, bundles the
// worker with that list and a version derived from it, and writes `sw.js`. The
// worker is a script of its own, with no imports from the app's chunks.

import { readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join, resolve, sep } from "node:path";
import { build, type Plugin, type ResolvedConfig } from "vite";
import { isPrecached, planPrecache, type BuildFile } from "./precache";

const WORKER_ENTRY = "src/sw/worker.ts";

/** The files the worker precaches, read from the build's output directory. */
function readShellFiles(outDir: string): BuildFile[] {
  return readdirSync(outDir, { recursive: true, encoding: "utf8" })
    .map((relative) => relative.split(sep).join("/"))
    .filter((path) => isPrecached(path) && statSync(join(outDir, path)).isFile())
    .map((path) => ({ path, bytes: readFileSync(join(outDir, path)) }));
}

/** The worker's bundled code with `urls` and `version` filled in. */
async function bundleWorker(root: string, urls: string[], version: string): Promise<string> {
  const result = await build({
    // The worker is plain TypeScript: none of the app's plugins apply to it.
    configFile: false,
    root,
    logLevel: "warn",
    publicDir: false,
    define: {
      __SW_PRECACHE__: JSON.stringify(urls),
      __SW_VERSION__: JSON.stringify(version),
    },
    build: {
      write: false,
      emptyOutDir: false,
      lib: {
        entry: resolve(root, WORKER_ENTRY),
        formats: ["iife"],
        name: "residuumWorker",
        fileName: () => "sw.js",
      },
    },
  });
  const outputs = Array.isArray(result) ? result : [result];
  for (const output of outputs) {
    if (!("output" in output)) continue;
    const chunk = output.output.find((item) => item.type === "chunk");
    if (chunk !== undefined) return chunk.code;
  }
  throw new Error(`building ${WORKER_ENTRY} produced no script`);
}

export function serviceWorkerPlugin(): Plugin {
  let config: ResolvedConfig;
  return {
    name: "residuum-service-worker",
    apply: "build",
    configResolved(resolved) {
      config = resolved;
    },
    async writeBundle() {
      const outDir = resolve(config.root, config.build.outDir);
      const { urls, version } = planPrecache(readShellFiles(outDir));
      const code = await bundleWorker(config.root, urls, version);
      // The first line names the version, which a test of an update reads to stand in for a rebuild.
      writeFileSync(join(outDir, "sw.js"), `/* residuum-sw ${version} */\n${code}`);
      config.logger.info(
        `service worker: precaching ${String(urls.length)} files, version ${version}`,
      );
    },
  };
}
