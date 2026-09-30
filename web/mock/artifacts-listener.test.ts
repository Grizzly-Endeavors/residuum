import { once } from "node:events";
import { request, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { startArtifactsListener, workbenchPage } from "./artifacts-listener";
import { MOCK_FEATURES, MOCK_RESIDUUM_VERSION } from "./constants";
import { createState, type MockState } from "./state";
import { removePath, writeFile } from "./workspace-tree";

describe("the page an artifact is served as", () => {
  const scripted = (html: string): string => workbenchPage(html, "chart");

  it("has the context and the SDK injected at the top of its head", () => {
    const page = scripted("<!doctype html><html><head lang=x><title>T</title></head></html>");
    expect(
      page.startsWith(
        '<!doctype html><html><head lang=x><script>{const __RESIDUUM_ARTIFACT__="chart";',
      ),
    ).toBe(true);
    expect(page).toContain(`const __RESIDUUM_VERSION__=${JSON.stringify(MOCK_RESIDUUM_VERSION)};`);
    expect(page).toContain(`const __RESIDUUM_FEATURES__=${JSON.stringify(MOCK_FEATURES)};`);
    expect(page).toContain("residuum");
    expect(page.endsWith("</script><title>T</title></head></html>")).toBe(true);
  });

  it("keeps the context constants in a block of their own, out of the page's global scope", () => {
    const page = scripted("<head></head>");
    expect(page.replaceAll("\r\n", "\n")).toMatch(/\n?\}<\/script><\/head>$/);
    expect(page).toContain("<script>{const __RESIDUUM_ARTIFACT__");
  });

  it("finds the head tag whatever its case, and not a header element", () => {
    expect(scripted("<HEAD><title>T</title></HEAD>")).toMatch(/^<HEAD><script>\{const /);
    expect(scripted("<html><body><header>h</header></body></html>")).toMatch(
      /^<html><script>\{const /,
    );
  });

  it("goes after the html tag when there is no head, then after the doctype, then at the start", () => {
    expect(scripted("<!doctype html><html lang=en><body></body></html>")).toMatch(
      /^<!doctype html><html lang=en><script>\{const /,
    );
    expect(scripted("<!DOCTYPE html><p>hi</p>")).toMatch(/^<!DOCTYPE html><script>\{const /);
    expect(scripted("<div>fragment</div>")).toMatch(
      /^<script>\{const .*<\/script><div>fragment<\/div>$/s,
    );
  });

  it("skips a tag that never closes, and keeps the page's own text intact", () => {
    expect(scripted("<head")).toMatch(/^<script>\{const .*<\/script><head$/s);
    expect(scripted("<p>İ</p><head>x</head>")).toContain("<p>İ</p><head><script>");
  });

  it("escapes a closing script tag in the name, so it can't end the block early", () => {
    expect(workbenchPage("<head></head>", "a</script>b")).toContain(
      'const __RESIDUUM_ARTIFACT__="a<\\/script>b";',
    );
  });
});

describe("the artifacts listener", () => {
  let server: Server;
  let state: MockState;
  let logged: string[];

  beforeEach(async () => {
    state = createState("hub");
    logged = [];
    server = startArtifactsListener(state, (message) => {
      logged.push(message);
    });
    await once(server, "listening");
  });

  afterEach(async () => {
    server.closeAllConnections();
    await new Promise<void>((resolve) => {
      server.close(() => {
        resolve();
      });
    });
  });

  const port = (): number => (server.address() as AddressInfo).port;
  const origin = (): string => `http://127.0.0.1:${port()}`;

  /** A request that keeps its path as written and follows no redirect. */
  async function get(path: string, init?: RequestInit): Promise<Response> {
    return fetch(`${origin()}${path}`, { redirect: "manual", ...init });
  }

  /** A request whose path reaches the server as written, with none of a URL parser's clean-up. */
  function rawGet(path: string): Promise<number | undefined> {
    return new Promise((resolve, reject) => {
      request({ host: "127.0.0.1", port: port(), path }, (res) => {
        res.resume();
        resolve(res.statusCode);
      })
        .on("error", reject)
        .end();
    });
  }

  const team = "team/workbench";

  it("records its port and says where it is", () => {
    expect(state.workbenchPort).toBe(port());
    expect(logged).toEqual([
      `  [mock] Workbench artifacts on http://localhost:${state.workbenchPort}`,
    ]);
  });

  describe("a page artifact", () => {
    it("is served at /{artifact}/, with its name embedded, never cached or sniffed", async () => {
      for (const path of ["/tip-splitter/", "/tip-splitter/?from=ui", "/tip-splitter/index.html"]) {
        const res = await get(path);
        expect(res.status, path).toBe(200);
        expect(res.headers.get("content-type")).toBe("text/html; charset=utf-8");
        expect(res.headers.get("cache-control")).toBe("no-store");
        expect(res.headers.get("x-content-type-options")).toBe("nosniff");
        const page = await res.text();
        expect(page).toContain('const __RESIDUUM_ARTIFACT__="tip-splitter";');
        expect(page).toContain("<title>Tip Splitter</title>");
      }
    });

    it("has no files besides its page", async () => {
      const res = await get("/tip-splitter/app.js");
      expect(res.status).toBe(404);
      expect(await res.text()).toContain(
        '<p>The artifact "tip-splitter" has no file "app.js".</p>',
      );
    });

    it("follows edits, arrivals and deletions in the team files", async () => {
      writeFile(state, `${team}/fresh.html`, "<head></head><p>fresh</p>");
      expect((await get("/fresh/")).status).toBe(200);
      writeFile(state, `${team}/fresh.html`, "<head></head><p>edited</p>");
      expect(await (await get("/fresh/")).text()).toContain("<p>edited</p>");
      removePath(state, `${team}/fresh.html`);
      expect((await get("/fresh/")).status).toBe(404);
    });

    it("is not served from its data files", async () => {
      writeFile(state, `${team}/tip-splitter.state.json`, '{"bill":84}');
      expect((await get("/tip-splitter.state.json")).status).toBe(404);
      expect((await get("/tip-splitter/tip-splitter.state.json")).status).toBe(404);
    });
  });

  describe("a folder artifact", () => {
    beforeEach(() => {
      writeFile(state, `${team}/graph/index.html`, "<!doctype html><html><head></head></html>");
      writeFile(state, `${team}/graph/app.js`, "console.log(1)");
      writeFile(state, `${team}/graph/style.css`, "body{}");
      writeFile(state, `${team}/graph/data/points.json`, "[1,2]");
      writeFile(state, `${team}/graph/docs/index.html`, "<p>docs</p>");
      writeFile(state, `${team}/graph.state.json`, "{}");
    });

    it("serves its index at /{artifact}/ with the SDK injected", async () => {
      const res = await get("/graph/");
      expect(res.status).toBe(200);
      const page = await res.text();
      expect(page).toContain('const __RESIDUUM_ARTIFACT__="graph";');
      expect(page).toMatch(/^<!doctype html><html><head><script>/);
    });

    it("serves the files it loads by relative URL, each with its type", async () => {
      const served = await Promise.all(
        ["/graph/app.js", "/graph/style.css", "/graph/data/points.json"].map(async (path) => {
          const res = await get(path);
          return [path, res.status, res.headers.get("content-type"), await res.text()];
        }),
      );
      expect(served).toEqual([
        ["/graph/app.js", 200, "text/javascript; charset=utf-8", "console.log(1)"],
        ["/graph/style.css", 200, "text/css; charset=utf-8", "body{}"],
        ["/graph/data/points.json", 200, "application/json", "[1,2]"],
      ]);
    });

    it("sends the same headers for a file as for a page", async () => {
      const res = await get("/graph/app.js");
      expect(res.headers.get("cache-control")).toBe("no-store");
      expect(res.headers.get("x-content-type-options")).toBe("nosniff");
    });

    it("injects the SDK into any HTML file in it, and serves a trailing slash as that folder's index", async () => {
      const page = await (await get("/graph/docs/")).text();
      expect(page).toMatch(/^<script>\{const __RESIDUUM_ARTIFACT__="graph";.*<p>docs<\/p>$/s);
      expect((await get("/graph/docs/index.html")).status).toBe(200);
    });

    it("answers 404 for a file or folder it doesn't have, naming the file", async () => {
      for (const path of ["/graph/nope.js", "/graph/data", "/graph/data/"]) {
        const res = await get(path);
        expect(res.status, path).toBe(404);
        expect(res.headers.get("x-content-type-options"), path).toBeNull();
        expect(await res.text(), path).toContain("has no file");
      }
    });

    it("refuses a path that climbs out of the folder, however it is spelled", async () => {
      for (const path of [
        "/graph/..%2Fgraph.state.json",
        "/graph/%2e%2e/graph.state.json",
        "/graph/%2e%2e%2fgraph.state.json",
        "/graph/data/..%2F..%2Fgraph.state.json",
        "/graph/..%5Cgraph.state.json",
        "/graph/./app.js",
        "/graph/data/../app.js",
      ]) {
        expect(await rawGet(path), path).toBe(404);
      }
    });
  });

  describe("redirects and refusals", () => {
    it("sends /{artifact} on to /{artifact}/ with a 308, keeping the query", async () => {
      const bare = await get("/tip-splitter");
      expect(bare.status).toBe(308);
      expect(bare.headers.get("location")).toBe("/tip-splitter/");

      const queried = await get("/tip-splitter?full=1&x=a%20b");
      expect(queried.status).toBe(308);
      expect(queried.headers.get("location")).toBe("/tip-splitter/?full=1&x=a%20b");
    });

    it("redirects a folder artifact the same way, and follows to its page", async () => {
      writeFile(state, `${team}/graph/index.html`, "<head></head>");
      expect((await get("/graph")).headers.get("location")).toBe("/graph/");
      const followed = await fetch(`${origin()}/graph`);
      expect(followed.url).toBe(`${origin()}/graph/`);
      expect(followed.status).toBe(200);
    });

    it("answers 404 instead of redirecting to an artifact that isn't there", async () => {
      for (const path of ["/nothing", "/Bad.Name", "/tip--splitter", "/tip-splitter.html"]) {
        const res = await get(path);
        expect(res.status, path).toBe(404);
        expect(res.headers.get("location"), path).toBeNull();
      }
    });

    it("says where artifacts open at the root", async () => {
      const res = await get("/");
      expect(res.status).toBe(200);
      expect(await res.text()).toContain(
        "Workbench artifacts open from the Workbench page in Residuum.",
      );
    });

    it("answers a missing artifact with a small HTML page that names it", async () => {
      const res = await get("/nothing/");
      expect(res.status).toBe(404);
      expect(res.headers.get("content-type")).toBe("text/html; charset=utf-8");
      expect(await res.text()).toContain('<p>There\'s no workbench artifact named "nothing".</p>');
      expect(await (await get("/api/status")).text()).toContain(
        'There\'s no workbench artifact named "api".',
      );
    });

    it("escapes what it echoes, since the page lands in the artifact's frame", async () => {
      writeFile(state, `${team}/graph/index.html`, "<head></head>");
      const res = await get("/graph/%3Cb%3E%26.js");
      expect(res.status).toBe(404);
      expect(await res.text()).toContain('has no file "&lt;b&gt;&amp;.js".</p>');
    });

    it("is read-only: only GET and HEAD are served", async () => {
      for (const method of ["POST", "PUT", "DELETE"]) {
        const res = await get("/tip-splitter/", { method });
        expect(res.status, method).toBe(405);
        expect(res.headers.get("allow")).toBe("GET,HEAD");
      }
      const head = await get("/tip-splitter/", { method: "HEAD" });
      expect(head.status).toBe(200);
      expect(await head.text()).toBe("");
    });
  });
});
