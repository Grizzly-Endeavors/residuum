import { readFile } from "node:fs/promises";
import type { IncomingMessage, ServerResponse } from "node:http";
import { join } from "node:path";
import type { MockHub } from "./state";

const VERSION_BANNER = /^\/\* residuum-sw ([0-9a-f]+) \*\//;

export function workerAfterRebuilds(source: string, rebuilds: number): string {
  const version = VERSION_BANNER.exec(source)?.[1];
  if (version === undefined) throw new Error("mock: sw.js doesn't start with its version");
  if (rebuilds === 0) return source;
  return source.replaceAll(version, `${version}-rebuild-${String(rebuilds)}`);
}

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
