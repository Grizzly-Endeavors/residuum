import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createRebuiltWorkerHandler, workerAfterRebuilds } from "./rebuilt-worker";
import { createStubHub, fetchText } from "./test-support";
import type { MockHub } from "./state";

const WORKER =
  '/* residuum-sw 0b5cff776324 */\n(function(){const a="0b5cff776324";const b="0b5cff776324"})();';

describe("the worker after a rebuild", () => {
  it("is the build's script until the app is rebuilt", () => {
    expect(workerAfterRebuilds(WORKER, 0)).toBe(WORKER);
  });

  it("carries a new version everywhere the old one appeared", () => {
    const rebuilt = workerAfterRebuilds(WORKER, 2);
    expect(rebuilt).not.toContain('0b5cff776324"');
    expect(rebuilt).not.toBe(WORKER);
    expect(rebuilt.match(/0b5cff776324-rebuild-2/g)).toHaveLength(3);
    expect(workerAfterRebuilds(WORKER, 3)).not.toBe(rebuilt);
  });

  it("refuses a script that doesn't name its version, which the build always does", () => {
    expect(() => workerAfterRebuilds("(function(){})();", 1)).toThrow(
      /doesn't start with its version/,
    );
  });
});

describe("serving the rebuilt worker", () => {
  let dist: string;
  let hub: MockHub;
  let server: Server;
  let baseUrl: string;

  beforeEach(async () => {
    dist = mkdtempSync(join(tmpdir(), "rebuilt-worker-"));
    writeFileSync(join(dist, "sw.js"), WORKER);
    hub = createStubHub();
    const handle = createRebuiltWorkerHandler(hub, dist);
    server = createServer((req, res) => {
      void handle(req, res).then((handled) => {
        if (!handled) {
          res.writeHead(404);
          res.end("preview server");
        }
      });
    });
    await new Promise<void>((resolve) => {
      server.listen(0, "127.0.0.1", resolve);
    });
    baseUrl = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  });

  afterEach(async () => {
    server.closeAllConnections();
    await new Promise<void>((resolve) => {
      server.close(() => {
        resolve();
      });
    });
    rmSync(dist, { recursive: true });
  });

  it("leaves /sw.js to the preview server until a test rebuilds the app", async () => {
    expect(await fetchText(`${baseUrl}/sw.js`)).toEqual({ status: 404, body: "preview server" });
  });

  it("answers /sw.js as the rebuilt app's, as JavaScript that is checked again on every load", async () => {
    hub.appRebuilds = 1;
    const res = await fetch(`${baseUrl}/sw.js?cache-bust=1`);
    expect(res.status).toBe(200);
    expect(res.headers.get("content-type")).toBe("text/javascript");
    expect(res.headers.get("cache-control")).toBe("no-cache");
    expect(await res.text()).toBe(workerAfterRebuilds(WORKER, 1));
  });

  it("gives a HEAD request the headers and no body, and leaves other methods and paths alone", async () => {
    hub.appRebuilds = 1;
    const head = await fetch(`${baseUrl}/sw.js`, { method: "HEAD" });
    expect(head.status).toBe(200);
    expect(await head.text()).toBe("");
    expect((await fetch(`${baseUrl}/sw.js`, { method: "POST" })).status).toBe(404);
    expect((await fetch(`${baseUrl}/index.html`)).status).toBe(404);
  });
});
