import { once } from "node:events";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { WebSocket } from "ws";
import { apiRoutes } from "./api-routes";
import { isValidArtifactName } from "./artifact-name";
import { startArtifactsListener } from "./artifacts-listener";
import { ARTIFACT_BLOCKED_ROUTES, ARTIFACT_REFUSAL_MESSAGE, isApiTarget } from "./artifacts-origin";
import { createMockEnv } from "./env";
import { createHub } from "./hub";
import { createApiHandler } from "./middleware";
import { seedAgents } from "./scenario";
import type { MockAgent, MockHub } from "./state";
import { TestSocket, fetchJson, fetchText } from "./test-support";
import { writeFile } from "./workspace-tree";

/** The mock's own server, where the web app is served, and the artifacts listener beside it, both on real ports. */
describe("the artifacts origin", () => {
  let appServer: Server;
  let listener: Server;
  let hub: MockHub;
  const sockets: TestSocket[] = [];

  const listen = async (server: Server): Promise<string> => {
    if (!server.listening) await once(server.listen(0, "127.0.0.1"), "listening");
    return `127.0.0.1:${(server.address() as AddressInfo).port}`;
  };

  /** The app's origin, where requests are not marked as arriving through the artifacts origin. */
  let app: string;
  /** The artifacts origin. */
  let artifacts: string;

  beforeEach(async () => {
    appServer = createServer();
    hub = createHub(appServer, { env: createMockEnv({ deterministic: true }), seed: seedAgents });
    const api = createApiHandler({ hub, routes: apiRoutes });
    appServer.on("request", (req, res) => {
      void api(req, res).then((handled) => {
        if (!handled) {
          res.writeHead(404);
          res.end();
        }
      });
    });
    listener = startArtifactsListener(hub.hubState, () => undefined, {
      api,
      sockets: appServer,
    });
    app = await listen(appServer);
    artifacts = await listen(listener);
  });

  afterEach(async () => {
    await Promise.all(sockets.splice(0).map((socket) => socket.close()));
    for (const server of [appServer, listener]) {
      server.closeAllConnections();
      await new Promise<void>((resolve) => {
        server.close(() => {
          resolve();
        });
      });
    }
  });

  const url = (origin: string, path: string): string => `http://${origin}${path}`;

  function openSocket(origin: string, path: string): Promise<TestSocket> {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(`ws://${origin}${path}`);
      const socket = new TestSocket(ws);
      ws.once("open", () => {
        sockets.push(socket);
        resolve(socket);
      });
      ws.once("error", reject);
      ws.once("unexpected-response", (_req, res) => {
        ws.terminate();
        reject(new Error(`refused with ${String(res.statusCode)}`));
      });
    });
  }

  const atlas = (): MockAgent => {
    const agent = hub.agents.get("atlas");
    if (agent === undefined) throw new Error("the scenario has atlas");
    return agent;
  };

  describe("API forwarding", () => {
    it("answers the same API as the app, hub routes and agent routes alike", async () => {
      for (const path of ["/api/hub/agents", "/api/agents/atlas/status", "/api/hub/status"]) {
        const fromApp = await fetchJson(url(app, path));
        const fromArtifacts = await fetchJson(url(artifacts, path));
        expect(fromArtifacts.status, path).toBe(200);
        expect(fromArtifacts.body, path).toEqual(fromApp.body);
      }
    });

    it("keeps the method, the body and the query of what it forwards", async () => {
      const put = await fetchJson(url(artifacts, "/api/team/workspace/file"), {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path: "notes/from-artifact.md", content: "hello" }),
      });
      expect(put.status).toBe(200);
      const read = await fetchText(
        url(app, "/api/team/workspace/file?path=notes/from-artifact.md"),
      );
      expect(read).toEqual({ status: 200, body: "hello" });
    });

    it("answers an unknown API path with the API's own 404, never a page", async () => {
      const res = await fetchText(url(artifacts, "/api/no-such-route"));
      expect(res.status).toBe(404);
      expect(JSON.parse(res.body)).toHaveProperty("error");
    });

    it("forwards only /api: every other path is the listener's own", async () => {
      expect(isApiTarget("/api")).toBe(true);
      expect(isApiTarget("/api/")).toBe(true);
      expect(isApiTarget("/api/hub/agents?x=1")).toBe(true);
      for (const path of ["/", "/apix", "/api-tools/", "/index.html", "/webhook/scout/x"]) {
        expect(isApiTarget(path), path).toBe(false);
      }
      const root = await fetchText(url(artifacts, "/"));
      expect(root.body).toContain("Workbench artifacts open from the Workbench page");
      const other = await fetchText(url(artifacts, "/index.html"));
      expect(other.status).toBe(404);
      expect(other.body).toContain("There's no workbench artifact named");
    });
  });

  describe("sockets", () => {
    it("opens the hub socket and an agent's socket", async () => {
      const hubSocket = await openSocket(artifacts, "/api/hub/ws");
      expect((await hubSocket.next()).type).toBe("hub_boot");

      const agentSocket = await openSocket(artifacts, "/api/agents/atlas/ws");
      agentSocket.send({ type: "ping" });
      expect(await agentSocket.nextOfType("pong")).toEqual({ type: "pong" });
    });

    it("refuses an upgrade no socket route takes, and any upgrade outside /api", async () => {
      await expect(openSocket(artifacts, "/api/agents/nobody/ws")).rejects.toThrow(
        "refused with 404",
      );
      await expect(openSocket(artifacts, "/api/agents/drifter/ws")).rejects.toThrow(
        "refused with 409",
      );
      await expect(openSocket(artifacts, "/")).rejects.toThrow();
    });

    it("doesn't count an agent socket as a client, and doesn't reset unread", async () => {
      hub.addUnread(atlas());
      expect(atlas().unread).toBe(1);

      const page = await openSocket(artifacts, "/api/agents/atlas/ws");
      await page.settled();
      expect(atlas().unread).toBe(1);
      expect(atlas().connectedClients()).toBe(0);

      const ui = await openSocket(app, "/api/agents/atlas/ws");
      await ui.settled();
      expect(atlas().unread).toBe(0);
      expect(atlas().connectedClients()).toBe(1);
    });
  });

  describe("the block list", () => {
    it.each(ARTIFACT_BLOCKED_ROUTES)("refuses %s with a 403 and says why", async (path) => {
      for (const method of ["POST", "GET"]) {
        const res = await fetchJson(url(artifacts, path), { method });
        expect(res, `${method} ${path}`).toEqual({
          status: 403,
          body: { error: ARTIFACT_REFUSAL_MESSAGE },
        });
      }
    });

    it("lists the routes the backend's block list does", () => {
      expect(ARTIFACT_BLOCKED_ROUTES).toEqual([
        "/api/hub/shutdown",
        "/api/hub/stop-all",
        "/api/hub/update/check",
        "/api/hub/update/apply",
        "/api/hub/update/restart",
        "/api/hub/config/complete-setup",
      ]);
    });

    it("does nothing when it refuses", async () => {
      await fetchJson(url(artifacts, "/api/hub/stop-all"), { method: "POST" });
      expect(atlas().runState).toBe("running");
    });

    it("leaves the same routes open on the app's own origin, where nothing is marked", async () => {
      for (const path of ARTIFACT_BLOCKED_ROUTES) {
        const res = await fetchJson(url(app, path), { method: "POST" });
        expect(res.status, path).not.toBe(403);
      }
    });

    it("can't be dodged or provoked by a header: nothing a client sends marks a request", async () => {
      const forged = await fetchJson(url(app, "/api/hub/stop-all"), {
        method: "POST",
        headers: {
          "X-Residuum-Artifacts-Origin": "1",
          "X-Residuum-Artifact": "chart",
          Origin: `http://${artifacts}`,
        },
      });
      expect(forged.status).toBe(200);
      expect(atlas().runState).toBe("stopped");
    });

    it("allows everything else, including stopping a single agent", async () => {
      expect((await fetchJson(url(artifacts, "/api/hub/update/status"))).status).toBe(200);
      const stop = await fetchJson(url(artifacts, "/api/hub/agents/atlas/stop"), {
        method: "POST",
      });
      expect(stop.status).toBe(200);
      expect(atlas().runState).toBe("stopped");
    });
  });

  describe("the reserved name", () => {
    it("is not a valid artifact name, though names that merely contain it are", () => {
      expect(isValidArtifactName("api")).toBe(false);
      for (const name of ["api-explorer", "my-api", "apis", "a"]) {
        expect(isValidArtifactName(name), name).toBe(true);
      }
    });

    it("leaves an artifact named api neither listed nor served", async () => {
      writeFile(hub.hubState, "team/workbench/api/index.html", "<head></head><p>folder</p>");
      writeFile(hub.hubState, "team/workbench/api.html", "<head></head><p>page</p>");
      writeFile(hub.hubState, "team/workbench/api-tools.html", "<head></head><p>tools</p>");

      const listed = (await fetchJson(url(app, "/api/team/workbench/artifacts"))).body as {
        name: string;
      }[];
      const names = listed.map((artifact) => artifact.name);
      expect(names).toContain("api-tools");
      expect(names).not.toContain("api");

      for (const path of ["/api/", "/api/index.html"]) {
        const res = await fetchText(url(artifacts, path));
        expect(res.status, path).toBe(404);
        expect(res.body, path).not.toContain("folder");
      }
      expect((await fetchText(url(artifacts, "/api-tools/"))).body).toContain("tools");
    });
  });
});
