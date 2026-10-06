/**
 * Playwright's `globalSetup`: compile the dev server's whole module graph
 * before the first test.
 *
 * Vite compiles a module the first time something asks for it and keeps the
 * result. Left alone, the first spec to need a part of the app pays for it:
 * the first load of the app, the first time Settings or the file editor opens
 * (each is a lazy chunk of dozens of modules), the first visit to
 * `/dev/gallery`. When other suites share the machine that cold compile takes
 * longer than a wait's default of five seconds, and the spec that happened to
 * go first fails while the same spec passes on its own. Asking for every module
 * here, once, moves that cost out of the specs.
 *
 * The crawl starts at the app's entry module and follows every import it finds,
 * dynamic ones included, which is what reaches the lazy chunks and the gallery
 * without a list to keep up to date. Playwright starts the web servers before
 * this runs. Only the dev server needs it: the preview server hands out files
 * built ahead of time. Each worker has its own dev server with its own module
 * cache, so every one of them is warmed, at the same time.
 */
import { workerServers } from "./servers";

const ENTRY = "/src/main.ts";

/** An import's specifier as Vite writes it into a compiled module: an absolute path from the server's root. */
const SPECIFIER = /(?:\bfrom\s*|\bimport\s*\(?\s*)["'](\/[^"'\s]+)["']/g;

/** How many modules are asked for at once, few enough that a loaded machine answers every request. */
const CONCURRENCY = 8;

interface Compiled {
  path: string;
  /** The modules it imports. */
  imports: string[];
  /** Why it did not compile, in one line, or `null`. */
  failure: string | null;
}

/**
 * How many times a request that never got an answer is made. The server
 * closes the idle connections its clients keep, so a request that reuses one
 * a moment too late is reset; asking again opens a new connection. An answer
 * with an error status is a module that does not compile, and is final.
 */
const ATTEMPTS = 3;

async function compile(origin: string, path: string): Promise<Compiled> {
  let failure = "";
  for (let attempt = 1; attempt <= ATTEMPTS; attempt += 1) {
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
      failure = error instanceof Error ? error.message : String(error);
    }
  }
  return { path, imports: [], failure: `no answer after ${ATTEMPTS} attempts: ${failure}` };
}

/** Compile `paths` on the server at `origin`, `CONCURRENCY` at a time. */
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
  const started = Date.now();
  const counts = await Promise.all(workerServers.map(({ dev }) => warmDevServer(dev.url)));
  const seconds = ((Date.now() - started) / 1000).toFixed(1);
  const servers =
    counts.length === 1 ? "the dev server's" : `${String(counts.length)} dev servers'`;
  process.stderr.write(`Warmed ${servers} ${String(counts[0] ?? 0)} modules in ${seconds}s.\n`);
}

/** Compile every module the dev server at `origin` can reach from the entry, and answer how many there were. */
async function warmDevServer(origin: string): Promise<number> {
  const seen = new Set<string>([ENTRY]);
  const failures: string[] = [];

  // One level of the import graph at a time: the modules the last level named.
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
