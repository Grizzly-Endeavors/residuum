import { readFileSync } from "node:fs";
import { createServer, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { APP_SHELL_CSP, appShellPolicy } from "./app-shell-policy";
import { WEB_ROOT } from "./assets";

/** A server that runs the middleware, then answers as `respond` does. */
async function serve(respond: (res: ServerResponse) => void): Promise<Server> {
  const server = createServer((req, res) => {
    appShellPolicy(req, res, () => {
      respond(res);
    });
  });
  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
  return server;
}

function urlOf(server: Server): string {
  return `http://127.0.0.1:${String((server.address() as AddressInfo).port)}/`;
}

describe("the app shell's policy in the mock", () => {
  let server: Server | undefined;

  afterEach(async () => {
    const running = server;
    server = undefined;
    if (running !== undefined) await new Promise((done) => running.close(done));
  });

  it("is the hub's, from the file the hub reads", () => {
    const file = readFileSync(resolve(WEB_ROOT, "app-shell-csp.txt"), "utf-8");
    expect(APP_SHELL_CSP).toBe(file.trim());
    expect(APP_SHELL_CSP).toContain("img-src 'self' data: blob:");
    expect(APP_SHELL_CSP).not.toContain("\n");
  });

  it.each([
    [
      "set before the body, as the dev server serves index.html",
      (res: ServerResponse) => {
        res.setHeader("Content-Type", "text/html");
        res.end("<!doctype html>");
      },
    ],
    [
      "handed to writeHead, as the preview serves a file",
      (res: ServerResponse) => {
        res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
        res.end("<!doctype html>");
      },
    ],
  ])("goes on an HTML document whose type is %s", async (_how, respond) => {
    server = await serve(respond);
    const res = await fetch(urlOf(server));
    expect(res.headers.get("content-security-policy")).toBe(APP_SHELL_CSP);
  });

  it.each([
    ["a script", "text/javascript"],
    ["JSON", "application/json"],
  ])("stays off %s", async (_what, type) => {
    server = await serve((res) => {
      res.writeHead(200, { "Content-Type": type });
      res.end("{}");
    });
    const res = await fetch(urlOf(server));
    expect(res.headers.get("content-security-policy")).toBeNull();
  });
});
