import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { jsonResponse, mockFetch, render, screen, settle } from "../test/component";
import { FakeWebSocket } from "../test/fake-websocket";
import Workspace from "./Workspace.svelte";
import { hub } from "../lib/hub.svelte";
import { setViewedAgent } from "../lib/viewed-agent";
import type { WorkspaceEntry } from "../lib/types";

function file(name: string): WorkspaceEntry {
  return { name, entry_type: "file", size: 1, modified: 0, version: "v" };
}

function folder(name: string): WorkspaceEntry {
  return { name, entry_type: "directory", size: null, modified: 0, version: "v" };
}

/** The root listing the fake server answers, and how often it was asked for. */
let rootListing: WorkspaceEntry[] = [];
let rootFetches = 0;

function serve(rootUrl: string): void {
  rootFetches = 0;
  mockFetch((url) => {
    if (url === rootUrl) {
      rootFetches += 1;
      return jsonResponse(rootListing);
    }
    // Connecting an agent also loads its history and usage; the tree doesn't wait for them.
    return new Promise<Response>(() => {});
  });
}

function frames(socket: FakeWebSocket, type: string): unknown[] {
  return socket.sentFrames().filter((f) => (f as { type: string }).type === type);
}

/** The socket opened for `agent`. */
function agentSocket(agent: string): FakeWebSocket {
  const socket = FakeWebSocket.sockets.find((s) => s.url.endsWith(`/api/agents/${agent}/ws`));
  if (!socket) throw new Error(`no socket was opened for ${agent}`);
  return socket;
}

/** The hub's socket, opened. */
function openHubSocket(): FakeWebSocket {
  hub.connect();
  const socket = FakeWebSocket.sockets.find((s) => s.url.endsWith("/api/hub/ws"));
  if (!socket) throw new Error("the hub socket was not opened");
  socket.simulateOpen();
  return socket;
}

beforeEach(() => {
  FakeWebSocket.install();
  vi.stubGlobal("location", { protocol: "http:", host: "localhost:7700" });
  rootListing = [];
});

afterEach(() => {
  hub.disconnect();
  setViewedAgent(null);
  vi.unstubAllGlobals();
});

describe("Workspace follows the disk", () => {
  it("shows a team file that appears, from the hub's team watch", async () => {
    serve("/api/team/workspace/files");
    rootListing = [folder("wiki"), file("AGENTS.md")];
    const hubSocket = openHubSocket();

    render(Workspace, { agent: null, scope: "team" });
    await settle();
    expect(await screen.findByText("AGENTS.md")).toBeTruthy();
    expect(screen.queryByText("rules.md")).toBeNull();
    expect(frames(hubSocket, "watch_team")).toEqual([{ type: "watch_team", prefixes: ["team"] }]);

    rootListing = [folder("wiki"), file("AGENTS.md"), file("rules.md")];
    hubSocket.simulateMessage({
      type: "workspace_changed",
      changes: [{ path: "team/rules.md", kind: "created" }],
    });
    await settle();
    expect(await screen.findByText("rules.md")).toBeTruthy();
    expect(rootFetches).toBe(2);
  });

  it("does not list a folder again for a change in what a file contains", async () => {
    serve("/api/team/workspace/files");
    rootListing = [file("AGENTS.md")];
    const hubSocket = openHubSocket();

    render(Workspace, { agent: null, scope: "team" });
    await screen.findByText("AGENTS.md");
    hubSocket.simulateMessage({
      type: "workspace_changed",
      changes: [{ path: "team/AGENTS.md", kind: "modified" }],
    });
    await settle();
    expect(rootFetches).toBe(1);
  });

  it("lists every listed folder again after a resync", async () => {
    serve("/api/team/workspace/files");
    rootListing = [file("AGENTS.md")];
    const hubSocket = openHubSocket();

    render(Workspace, { agent: null, scope: "team" });
    await screen.findByText("AGENTS.md");
    rootListing = [file("AGENTS.md"), file("USER.md")];
    hubSocket.simulateMessage({ type: "workspace_resync", reason: "overflow" });
    await settle();
    expect(await screen.findByText("USER.md")).toBeTruthy();
  });

  it("shows an agent's file that appears, from the agent's own socket", async () => {
    serve("/api/agents/scout/workspace/files");
    rootListing = [file("SOUL.md")];
    setViewedAgent("scout");
    const socket = agentSocket("scout");
    socket.simulateOpen();

    render(Workspace, { agent: "scout" });
    await settle();
    expect(await screen.findByText("SOUL.md")).toBeTruthy();
    expect(frames(socket, "watch_workspace")).toEqual([
      { type: "watch_workspace", prefixes: [""] },
    ]);

    rootListing = [file("SOUL.md"), file("notes.md")];
    socket.simulateMessage({
      type: "workspace_changed",
      changes: [{ path: "notes.md", kind: "created" }],
    });
    await settle();
    expect(await screen.findByText("notes.md")).toBeTruthy();
  });

  it("gives its watch up when it goes away, and leaves another owner's in place", async () => {
    serve("/api/team/workspace/files");
    const hubSocket = openHubSocket();
    const wiki = hub.teamWatches.register({ changed: () => {} });
    wiki.set(["team/wiki"]);

    const { unmount } = render(Workspace, { agent: null, scope: "team" });
    await settle();
    expect(frames(hubSocket, "watch_team").at(-1)).toEqual({
      type: "watch_team",
      prefixes: ["team", "team/wiki"],
    });

    unmount();
    expect(frames(hubSocket, "watch_team").at(-1)).toEqual({
      type: "watch_team",
      prefixes: ["team/wiki"],
    });
    wiki.release();
    expect(frames(hubSocket, "watch_team").at(-1)).toEqual({ type: "watch_team", prefixes: [] });
  });
});
