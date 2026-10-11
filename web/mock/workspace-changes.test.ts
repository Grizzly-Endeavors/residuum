import { once } from "node:events";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { startArtifactsListener } from "./artifacts-listener";
import {
  NO_FORWARDING,
  fetchJson,
  startMockServer,
  type Frame,
  type MockServerHarness,
  type TestSocket,
} from "./test-support";

const PAGE = "team/workbench/tip-splitter.html";

describe("changing a team file from outside", () => {
  let mock: MockServerHarness;

  beforeEach(async () => {
    mock = await startMockServer({ deterministic: true });
  });

  afterEach(async () => {
    await mock.close();
  });

  async function change(body: unknown): Promise<{ status: number; body: unknown }> {
    return fetchJson(`${mock.baseUrl}/api/mock/team-file`, {
      method: "POST",
      body: JSON.stringify(body),
    });
  }

  /** An agent's socket, watching `prefixes` once the agent has taken them. */
  async function agentSocket(name: string, prefixes: string[]): Promise<TestSocket> {
    const socket = await mock.openSocket(`/api/agents/${name}/ws`);
    socket.send({ type: "watch_workspace", prefixes });
    await socket.settled();
    return socket;
  }

  /** What an agent's socket was sent since it last settled, without the ping's answer. */
  async function sentTo(socket: TestSocket): Promise<Frame[]> {
    return (await socket.settled()).filter((frame) => frame.type !== "pong");
  }

  const changed = (...changes: { path: string; kind: string }[]): Frame => ({
    type: "workspace_changed",
    changes,
  });

  describe("an edit", () => {
    it("changes the file the workbench lists and the listener serves, and says what it sent", async () => {
      const listener = startArtifactsListener(mock.hub.hubState, () => undefined, NO_FORWARDING);
      await once(listener, "listening");
      try {
        const res = await change({ path: PAGE, content: "<title>Edited</title><p>new</p>" });
        expect(res).toEqual({
          status: 200,
          body: {
            changes: [{ path: PAGE, kind: "modified" }],
            artifacts: { updated: ["tip-splitter"], removed: [] },
          },
        });

        const listed = await fetchJson(`${mock.baseUrl}/api/team/workbench/artifacts`);
        expect(listed.body).toMatchObject([{ name: "tip-splitter", title: "Edited" }]);
        const served = await fetch(
          `http://127.0.0.1:${mock.hub.hubState.workbenchPort}/tip-splitter/`,
        );
        expect(await served.text()).toContain("<p>new</p>");
      } finally {
        listener.closeAllConnections();
        listener.close();
      }
    });

    it("sends workspace_changed to a page watching the path, then artifact_updated", async () => {
      const scout = await agentSocket("scout", ["team/workbench"]);
      await change({ path: PAGE, content: "<title>Edited</title>" });
      expect(await sentTo(scout)).toEqual([
        changed({ path: PAGE, kind: "modified" }),
        { type: "artifact_updated", name: "tip-splitter" },
      ]);
    });

    it("sends artifact_updated to every page on every agent, whatever it watches, and workspace_changed only where it matches", async () => {
      const watchingOther = await agentSocket("atlas", ["team/wiki"]);
      const watchingNothing = await agentSocket("scout", []);
      const watchingEverything = await agentSocket("scout", [""]);
      await change({ path: PAGE, content: "<title>Edited</title>" });
      const updated = { type: "artifact_updated", name: "tip-splitter" };
      expect(await sentTo(watchingOther)).toEqual([updated]);
      expect(await sentTo(watchingNothing)).toEqual([updated]);
      expect(await sentTo(watchingEverything)).toEqual([
        changed({ path: PAGE, kind: "modified" }),
        updated,
      ]);
    });

    it("sends a page that watches the file itself, or the folder above it, the change", async () => {
      const file = await agentSocket("scout", [PAGE]);
      const above = await agentSocket("atlas", ["team"]);
      await change({ path: PAGE, content: "<title>Edited</title>" });
      const both = [
        changed({ path: PAGE, kind: "modified" }),
        { type: "artifact_updated", name: "tip-splitter" },
      ];
      expect(await sentTo(file)).toEqual(both);
      expect(await sentTo(above)).toEqual(both);
    });

    it("sends no artifact frame for an artifact's saved data, only the change", async () => {
      const scout = await agentSocket("scout", ["team/workbench"]);
      const data = "team/workbench/tip-splitter.state.json";
      const res = await change({ path: data, content: '{"bill":90}' });
      expect(res.body).toEqual({
        changes: [{ path: data, kind: "created" }],
        artifacts: { updated: [], removed: [] },
      });
      expect(await sentTo(scout)).toEqual([changed({ path: data, kind: "created" })]);
      await change({ path: data, content: '{"bill":91}' });
      expect(await sentTo(scout)).toEqual([changed({ path: data, kind: "modified" })]);
    });

    it("sends no artifact frame for a rewrite that leaves the artifact's files as they were", async () => {
      const scout = await agentSocket("scout", ["team"]);
      const same = mock.hub.hubState.workspaceFileContents[PAGE] ?? "";
      const res = await change({ path: PAGE, content: same });
      expect(res.body).toMatchObject({ artifacts: { updated: [], removed: [] } });
      expect(await sentTo(scout)).toEqual([changed({ path: PAGE, kind: "modified" })]);
    });
  });

  describe("a new folder artifact", () => {
    it("is reported as its folder, which stands for what it holds, and then as updated", async () => {
      const scout = await agentSocket("scout", ["team/workbench"]);
      const res = await change({
        path: "team/workbench/graph/index.html",
        content: "<title>G</title>",
      });
      expect(res.body).toEqual({
        changes: [{ path: "team/workbench/graph", kind: "created" }],
        artifacts: { updated: ["graph"], removed: [] },
      });
      expect(await sentTo(scout)).toEqual([
        changed({ path: "team/workbench/graph", kind: "created" }),
        { type: "artifact_updated", name: "graph" },
      ]);
    });

    it("updates when a file in its folder changes, and reports a file added to an existing folder as itself", async () => {
      await change({ path: "team/workbench/graph/index.html", content: "<title>G</title>" });
      const scout = await agentSocket("scout", ["team/workbench/graph"]);
      const asset = "team/workbench/graph/app.js";
      await change({ path: asset, content: "let x;" });
      expect(await sentTo(scout)).toEqual([
        changed({ path: asset, kind: "created" }),
        { type: "artifact_updated", name: "graph" },
      ]);
    });
  });

  describe("a delete", () => {
    it("sends workspace_changed as removed, then artifact_removed, and the artifact is gone", async () => {
      const scout = await agentSocket("scout", ["team/workbench"]);
      const res = await change({ path: PAGE, content: null });
      expect(res).toEqual({
        status: 200,
        body: {
          changes: [{ path: PAGE, kind: "removed" }],
          artifacts: { updated: [], removed: ["tip-splitter"] },
        },
      });
      expect(await sentTo(scout)).toEqual([
        changed({ path: PAGE, kind: "removed" }),
        { type: "artifact_removed", name: "tip-splitter" },
      ]);
      const listed = await fetchJson(`${mock.baseUrl}/api/team/workbench/artifacts`);
      expect(listed.body).toEqual([]);
    });

    it("reports a removed folder alone, to a page watching a file inside it", async () => {
      await change({ path: "team/workbench/graph/index.html", content: "<title>G</title>" });
      const scout = await agentSocket("scout", ["team/workbench/graph/index.html"]);
      await change({ path: "team/workbench/graph", content: null });
      expect(await sentTo(scout)).toEqual([
        changed({ path: "team/workbench/graph", kind: "removed" }),
        { type: "artifact_removed", name: "graph" },
      ]);
    });
  });

  describe("the pages that watch", () => {
    it("are sent a change by the prefixes they last asked for, and keep them when a request is refused", async () => {
      const scout = await agentSocket("scout", ["team/workbench"]);
      scout.send({ type: "watch_workspace", prefixes: ["team/wiki", "team//wiki/./notes/"] });
      await sentTo(scout);

      await change({ path: PAGE, content: "<title>A</title>" });
      expect(await sentTo(scout)).toEqual([{ type: "artifact_updated", name: "tip-splitter" }]);

      scout.send({ type: "watch_workspace", prefixes: ["team/workbench", "../secrets"] });
      const refused = await sentTo(scout);
      expect(refused).toMatchObject([
        { type: "error", message: expect.stringContaining("..") as unknown },
      ]);

      await change({ path: "team/wiki/notes/a.md", content: "hi" });
      expect(await sentTo(scout)).toEqual([changed({ path: "team/wiki/notes", kind: "created" })]);
    });

    it("match by whole path segments", async () => {
      const scout = await agentSocket("scout", ["team/wiki"]);
      await change({ path: "team/wikipedia/a.md", content: "x" });
      await change({ path: "team/wiki.md", content: "x" });
      expect(await sentTo(scout)).toEqual([]);
      await change({ path: "team/wiki/a.md", content: "x" });
      expect(await sentTo(scout)).toEqual([changed({ path: "team/wiki/a.md", kind: "created" })]);
    });

    /**
     * A hub socket that has taken its `watch_team` frames. The hub answers a
     * frame it can't read with a notice, so that answer shows the earlier
     * frames were handled.
     */
    async function hubSocket(...watches: string[][]): Promise<TestSocket> {
      const socket = await mock.openSocket("/api/hub/ws");
      for (const prefixes of watches) socket.send({ type: "watch_team", prefixes });
      socket.sendRaw("not a frame");
      await socket.next((frame) => String(frame.message).includes("couldn't read"));
      return socket;
    }

    const feedFrames = (socket: TestSocket): string[] =>
      socket.frames
        .map((frame) => String(frame.type))
        .filter((type) => type === "workspace_changed" || type.startsWith("artifact_"));

    it("include the hub's team watch, and every hub socket gets the artifact frame whatever it watches", async () => {
      const watching = await hubSocket(["team/workbench"]);
      const other = await hubSocket(["team/wiki"]);
      const idle = await hubSocket();

      await change({ path: PAGE, content: "<title>Edited</title>" });

      expect(await watching.nextOfType("workspace_changed")).toEqual(
        changed({ path: PAGE, kind: "modified" }),
      );
      const updated = { type: "artifact_updated", name: "tip-splitter" };
      for (const socket of [watching, other, idle]) {
        expect(await socket.nextOfType("artifact_updated")).toEqual(updated);
      }
      expect(feedFrames(watching)).toEqual(["workspace_changed", "artifact_updated"]);
      expect(feedFrames(other)).toEqual(["artifact_updated"]);
      expect(feedFrames(idle)).toEqual(["artifact_updated"]);
    });

    it("send artifact frames to the hub socket with no agent running", async () => {
      for (const agent of mock.hub.agents.values()) mock.hub.transition(agent, "stopped");
      const hub = await hubSocket();

      await change({ path: PAGE, content: "<title>Edited</title>" });
      expect(await hub.nextOfType("artifact_updated")).toEqual({
        type: "artifact_updated",
        name: "tip-splitter",
      });
      await change({ path: PAGE, content: null });
      expect(await hub.nextOfType("artifact_removed")).toEqual({
        type: "artifact_removed",
        name: "tip-splitter",
      });
    });

    it("are told when the hub refuses a team watch, and keep the old one", async () => {
      const hub = await hubSocket(["team/workbench"], ["wiki"]);
      expect(hub.frames.filter((frame) => frame.type === "notice")).toMatchObject([
        { level: "warn", message: expect.stringContaining("start with team/") as unknown },
        { level: "warn", message: expect.stringContaining("couldn't read") as unknown },
      ]);
      await change({ path: PAGE, content: "<title>Edited</title>" });
      expect(await hub.nextOfType("workspace_changed")).toEqual(
        changed({ path: PAGE, kind: "modified" }),
      );
    });
  });

  describe("a change it refuses", () => {
    it.each([
      [{ path: "workbench/x.html", content: "x" }, 422],
      [{ path: "team", content: "x" }, 422],
      [{ path: "team/../x", content: "x" }, 422],
      [{ path: "/team/x", content: "x" }, 422],
      [{ path: "team/workbench", content: "x" }, 422],
      [{ path: PAGE }, 422],
      [{ path: PAGE, content: 1 }, 422],
      [{ content: "x" }, 422],
      [{ path: "team/workbench/nothing.html", content: null }, 404],
    ])("answers %j with %i, and sends nothing", async (body, status) => {
      const scout = await agentSocket("scout", [""]);
      expect((await change(body)).status).toBe(status);
      expect(await sentTo(scout)).toEqual([]);
    });
  });
});

describe("changing a file in an agent's own workspace from outside", () => {
  let mock: MockServerHarness;

  beforeEach(async () => {
    mock = await startMockServer({ deterministic: true });
  });

  afterEach(async () => {
    await mock.close();
  });

  async function change(agent: string, body: unknown): Promise<{ status: number; body: unknown }> {
    return fetchJson(`${mock.baseUrl}/api/mock/agent-file?agent=${agent}`, {
      method: "POST",
      body: JSON.stringify(body),
    });
  }

  async function watching(agent: string, prefixes: string[]): Promise<TestSocket> {
    const socket = await mock.openSocket(`/api/agents/${agent}/ws`);
    socket.send({ type: "watch_workspace", prefixes });
    await socket.settled();
    return socket;
  }

  /** What an agent's socket was sent since it last settled, without the ping's answer. */
  async function sentTo(socket: TestSocket): Promise<Frame[]> {
    return (await socket.settled()).filter((frame) => frame.type !== "pong");
  }

  it("writes the file and tells that agent's pages watching it, and no one else", async () => {
    const notes = await watching("atlas", ["notes"]);
    const memory = await watching("atlas", ["memory"]);
    const scout = await watching("scout", [""]);
    const hub = await mock.openSocket("/api/hub/ws");
    await hub.nextOfType("agents_snapshot");

    const res = await change("atlas", { path: "notes/today.md", content: "- water the plants" });
    expect(res).toEqual({
      status: 200,
      body: { changes: [{ path: "notes", kind: "created" }] },
    });
    const sent = { type: "workspace_changed", changes: [{ path: "notes", kind: "created" }] };
    expect(await sentTo(notes)).toEqual([sent]);
    expect(await sentTo(memory)).toEqual([]);
    expect(await sentTo(scout)).toEqual([]);
    expect(hub.frames.filter((frame) => frame.type === "workspace_changed")).toEqual([]);

    const read = await fetch(
      `${mock.baseUrl}/api/agents/atlas/workspace/file?path=notes%2Ftoday.md`,
    );
    expect(await read.text()).toBe("- water the plants");

    await change("atlas", { path: "notes/today.md", content: null });
    expect(await sentTo(notes)).toEqual([
      { type: "workspace_changed", changes: [{ path: "notes/today.md", kind: "removed" }] },
    ]);
  });

  it.each([
    ["atlas", { path: "team/wiki/a.md", content: "x" }, 422],
    ["atlas", { path: "", content: "x" }, 422],
    ["atlas", { path: "../x", content: "x" }, 422],
    ["atlas", { path: "nothing/here.md", content: null }, 404],
    ["nobody", { path: "notes/a.md", content: "x" }, 404],
  ])("answers %s %j with %i", async (agent, body, status) => {
    expect((await change(agent, body)).status).toBe(status);
  });
});
