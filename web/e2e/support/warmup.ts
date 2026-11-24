import { startedWorkerServers } from "./servers";

const ENTRY = "/src/main.ts";

const SPECIFIER = /(?:\bfrom\s*|\bimport\s*\(?\s*)["'](\/[^"'\s]+)["']/g;

const CONCURRENCY = 8;

interface Compiled {
  path: string;
  imports: string[];
  failure: string | null;
}

/**
 * Ask for one module. An answer with an error status is a module that does not
 * compile; no answer at all is a server that is down or broken. The mock leaves
 * idle connections open (`keepIdleConnectionsOpen` in `mock/http.ts`), so a
 * request never meets a connection the server is closing.
 */
async function compile(origin: string, path: string): Promise<Compiled> {
  try {
    const response = await fetch(`${origin}${path}`);
    const body = await response.text();
    if (!response.ok) {
      const firstLine = body.split("\n", 1)[0] ?? "";
      return { path, imports: [], failure: `${response.status} ${firstLine}`.trim() };
    }
    return {
      path,
      imports: Array.from(body.matchAll(SPECIFIER), (match) => match[1] ?? ""),
      failure: null,
    };
  } catch (error) {
    const reason = error instanceof Error ? error.message : String(error);
    return { path, imports: [], failure: `no answer: ${reason}` };
  }
}

async function compileAll(origin: string, paths: readonly string[]): Promise<Compiled[]> {
  const results: Compiled[] = [];
  const remaining = paths.values();
  const workers = Array.from({ length: Math.min(CONCURRENCY, paths.length) }, async () => {
    for (const path of remaining) results.push(await compile(origin, path));
  });
  await Promise.all(workers);
  return results;
}

export default async function warmDevServers(): Promise<void> {
  const servers = startedWorkerServers();
  const origins = (kind: "dev" | "preview"): string =>
    servers.map((worker) => worker[kind].url).join(", ");
  process.stderr.write(`Mock servers: dev ${origins("dev")}; preview ${origins("preview")}.\n`);
  const started = Date.now();
  const counts = await Promise.all(servers.map(({ dev }) => warmDevServer(dev.url)));
  const seconds = ((Date.now() - started) / 1000).toFixed(1);
  const warmed = counts.length === 1 ? "the dev server's" : `${String(counts.length)} dev servers'`;
  process.stderr.write(`Warmed ${warmed} ${String(counts[0] ?? 0)} modules in ${seconds}s.\n`);
}

async function warmDevServer(origin: string): Promise<number> {
  const seen = new Set<string>([ENTRY]);
  const failures: string[] = [];

  let level = [ENTRY];
  while (level.length > 0) {
    const next: string[] = [];
    for (const { path, imports, failure } of await compileAll(origin, level)) {
      if (failure !== null) failures.push(`  ${path}: ${failure}`);
      for (const found of imports) {
        if (seen.has(found)) continue;
        seen.add(found);
        next.push(found);
      }
    }
    level = next;
  }

  if (failures.length > 0) {
    throw new Error(
      `the dev server at ${origin} could not compile ${failures.length} of its ${seen.size} modules:\n${failures.join("\n")}`,
    );
  }
  return seen.size;
}
