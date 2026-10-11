import { readFile } from "node:fs/promises";
import type { IncomingMessage, ServerResponse } from "node:http";
import { join } from "node:path";
import type { MockHub } from "./state";

/** The first line of the built worker, which names its version (`build/service-worker.ts`). */
const VERSION_BANNER = /^\/\* residuum-sw ([0-9a-f]+) \*\//;

/**
 * The worker's script as it would be after `rebuilds` rebuilds of the app:
 * the same files under another version, so the script differs in its bytes and
 * its cache has a new name, which is all a browser needs to see an update.
 */
export function workerAfterRebuilds(source: string, rebuilds: number): string {
  const version = VERSION_BANNER.exec(source)?.[1];
  if (version === undefined) throw new Error("mock: sw.js doesn't start with its version");
  if (rebuilds === 0) return source;
  return source.replaceAll(version, `${version}-rebuild-${String(rebuilds)}`);
}

/**
 * Serves `/sw.js` from the build in `distDir` as a rebuilt app's worker once a
 * test has rebuilt the app (`POST /api/mock/rebuild`). Until then the preview
 * server serves the file as built, and this answers `false` for it.
 */
export function createRebuiltWorkerHandler(
  hub: MockHub,
  distDir: string,
): (req: IncomingMessage, res: ServerResponse) => Promise<boolean> {
  return async (req, res) => {
    const path = (req.url ?? "").split("?")[0];
    const reads = req.method === "GET" || req.method === "HEAD";
    if (path !== "/sw.js" || !reads || hub.appRebuilds === 0) return false;
    const source = await readFile(join(distDir, "sw.js"), "utf8");
    res.writeHead(200, { "content-type": "text/javascript", "cache-control": "no-cache" });
    res.end(req.method === "HEAD" ? undefined : workerAfterRebuilds(source, hub.appRebuilds));
    return true;
  };
}
