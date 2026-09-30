import { once } from "node:events";
import type { Server } from "node:http";
import type { AddressInfo } from "node:net";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { startArtifactsListener, workbenchPage } from "./artifacts-listener";
import { MOCK_FEATURES, MOCK_RESIDUUM_VERSION } from "./constants";
import { createState, type MockState } from "./state";

describe("the page an artifact is served as", () => {
  it("has the context and the SDK injected at the top of its head", () => {
    const page = workbenchPage(
      "<!doctype html><html><head><title>T</title></head></html>",
      "chart",
    );
    expect(page).toContain('<head><script>const __RESIDUUM_ARTIFACT__="chart";');
    expect(page).toContain(`const __RESIDUUM_VERSION__=${JSON.stringify(MOCK_RESIDUUM_VERSION)};`);
    expect(page).toContain(`const __RESIDUUM_FEATURES__=${JSON.stringify(MOCK_FEATURES)};`);
    expect(page).toContain("residuum");
    expect(page.endsWith("<title>T</title></head></html>")).toBe(true);
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

  const origin = (): string => `http://127.0.0.1:${(server.address() as AddressInfo).port}`;

  it("records its port and says where it is", () => {
    expect(state.workbenchPort).toBe((server.address() as AddressInfo).port);
    expect(logged).toEqual([
      `  [mock] Workbench artifacts on http://localhost:${state.workbenchPort}`,
    ]);
  });

  it("serves an artifact's page at /{artifact}/, with its name embedded and no caching", async () => {
    for (const path of ["/tip-splitter/", "/tip-splitter/?from=ui"]) {
      const res = await fetch(`${origin()}${path}`);
      expect(res.status).toBe(200);
      expect(res.headers.get("content-type")).toBe("text/html; charset=utf-8");
      expect(res.headers.get("cache-control")).toBe("no-store");
      const page = await res.text();
      expect(page).toContain('const __RESIDUUM_ARTIFACT__="tip-splitter";');
      expect(page).toContain("<title>Tip Splitter</title>");
    }
  });

  it("serves an artifact added after it started, and stops serving one that was deleted", async () => {
    state.workbenchArtifacts.set("fresh", {
      html: "<head></head><p>fresh</p>",
      modifiedAt: new Date().toISOString(),
    });
    expect((await fetch(`${origin()}/fresh/`)).status).toBe(200);
    state.workbenchArtifacts.delete("fresh");
    expect((await fetch(`${origin()}/fresh/`)).status).toBe(404);
  });

  it("answers 404 for anything that isn't an artifact's page", async () => {
    for (const path of [
      "/",
      "/nothing/",
      "/tip-splitter",
      "/tip-splitter/index.html",
      "/api/status",
    ]) {
      const res = await fetch(`${origin()}${path}`);
      expect(res.status, path).toBe(404);
      expect(await res.text()).toBe("There's no workbench artifact here.");
    }
  });
});
