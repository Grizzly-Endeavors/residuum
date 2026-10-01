import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { hub } from "../../lib/hub.svelte";
import { toast } from "../../lib/toast.svelte";
import type { WorkspaceEntry } from "../../lib/types";
import { setViewedAgent } from "../../lib/viewed-agent";
import { jsonResponse, mockFetch, render, screen, settle } from "../../test/component";
import { FakeWebSocket } from "../../test/fake-websocket";
import FilesPlace from "./FilesPlace.svelte";

function file(name: string): WorkspaceEntry {
  return { name, entry_type: "file", size: 1, modified: 0, version: "v" };
}

function folder(name: string): WorkspaceEntry {
  return { name, entry_type: "directory", size: null, modified: 0, version: "v" };
}

/** The listings the fake server answers, by folder, and how often each was asked for. */
let listings: Record<string, WorkspaceEntry[]> = {};
let listed: Record<string, number> = {};
let requests: { method: string; url: string; body?: string }[] = [];

function serve(filesUrl: string): void {
  listed = {};
  requests = [];
  mockFetch((url, init) => {
    const method = init?.method ?? "GET";
    requests.push({ method, url, body: typeof init?.body === "string" ? init.body : undefined });
    const parsed = new URL(url, "http://localhost");
    if (parsed.pathname === filesUrl) {
      const dir = parsed.searchParams.get("path") ?? "";
      listed[dir] = (listed[dir] ?? 0) + 1;
      return jsonResponse(listings[dir] ?? []);
    }
    if (method === "DELETE") {
      return jsonResponse({ deleted: true, checkpoint_id: "cp1", checkpoint_repo: "workspace" });
    }
    if (method === "POST" && parsed.pathname.endsWith("/move")) {
      return jsonResponse({ moved: true, version: null });
    }
    if (parsed.pathname.endsWith("/restore")) {
      return jsonResponse({ checkpoint_id: "cp2", restored_paths: ["SOUL.md"] });
    }
    // Connecting an agent also loads its history and usage; the tree doesn't wait for them.
    return new Promise<Response>(() => {});
  });
}

function frames(socket: FakeWebSocket, type: string): unknown[] {
  return socket.sent
    .map((data) => JSON.parse(data) as { type: string })
    .filter((frame) => frame.type === type);
}

function agentSocket(agent: string): FakeWebSocket {
  const socket = FakeWebSocket.sockets.find((s) => s.url.endsWith(`/api/agents/${agent}/ws`));
  if (!socket) throw new Error(`no socket was opened for ${agent}`);
  return socket;
}

function openHubSocket(): FakeWebSocket {
  hub.connect();
  const socket = FakeWebSocket.sockets.find((s) => s.url.endsWith("/api/hub/ws"));
  if (!socket) throw new Error("the hub socket was not opened");
  socket.simulateOpen();
  return socket;
}

const TEAM = { agent: null, scope: "team" } as const;
const SCOUT = { agent: "scout", scope: "agent" } as const;

beforeEach(() => {
  FakeWebSocket.install();
  vi.stubGlobal("location", { protocol: "http:", host: "localhost:7700" });
  listings = {};
});

afterEach(() => {
  hub.disconnect();
  setViewedAgent(null);
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("the tree follows the disk", () => {
  it("shows a team file that appears, from the hub's team watch", async () => {
    serve("/api/team/workspace/files");
    listings[""] = [folder("wiki"), file("AGENTS.md")];
    const hubSocket = openHubSocket();

    render(FilesPlace, { source: TEAM });
    expect(await screen.findByRole("button", { name: /^AGENTS\.md/ })).toBeTruthy();
    expect(frames(hubSocket, "watch_team")).toEqual([{ type: "watch_team", prefixes: ["team"] }]);

    listings[""] = [folder("wiki"), file("AGENTS.md"), file("rules.md")];
    hubSocket.simulateMessage({
      type: "workspace_changed",
      changes: [{ path: "team/rules.md", kind: "created" }],
    });
    expect(await screen.findByRole("button", { name: /^rules\.md/ })).toBeTruthy();
    expect(listed[""]).toBe(2);
  });

  it("does not list a folder again for a change in what a file contains", async () => {
    serve("/api/team/workspace/files");
    listings[""] = [file("AGENTS.md")];
    const hubSocket = openHubSocket();

    render(FilesPlace, { source: TEAM });
    await screen.findByRole("button", { name: /^AGENTS\.md/ });
    hubSocket.simulateMessage({
      type: "workspace_changed",
      changes: [{ path: "team/AGENTS.md", kind: "modified" }],
    });
    await settle();
    expect(listed[""]).toBe(1);
  });

  it("lists every open folder again after a resync, and after the socket reconnects", async () => {
    serve("/api/team/workspace/files");
    listings[""] = [folder("wiki")];
    listings.wiki = [file("index.md")];
    const hubSocket = openHubSocket();
    render(FilesPlace, { source: TEAM });
    await userEvent.click(await screen.findByRole("button", { name: "wiki" }));
    await screen.findByRole("button", { name: /^index\.md/ });

    hubSocket.simulateMessage({ type: "workspace_resync", reason: "overflow" });
    await settle();
    expect(listed).toEqual({ "": 2, wiki: 2 });

    listings.wiki = [file("index.md"), file("log.md")];
    hubSocket.simulateClose();
    const again = FakeWebSocket.last;
    again.simulateOpen();
    expect(await screen.findByRole("button", { name: /^log\.md/ })).toBeTruthy();
    expect(listed).toEqual({ "": 3, wiki: 3 });
  });

  it("shows an agent's file that appears, from the agent's own socket", async () => {
    serve("/api/agents/scout/workspace/files");
    listings[""] = [file("SOUL.md")];
    setViewedAgent("scout");
    const socket = agentSocket("scout");
    socket.simulateOpen();

    render(FilesPlace, { source: SCOUT });
    await screen.findByRole("button", { name: /^SOUL\.md/ });
    expect(frames(socket, "watch_workspace")).toEqual([
      { type: "watch_workspace", prefixes: [""] },
    ]);

    listings[""] = [file("SOUL.md"), file("notes.md")];
    socket.simulateMessage({
      type: "workspace_changed",
      changes: [{ path: "notes.md", kind: "created" }],
    });
    expect(await screen.findByRole("button", { name: /^notes\.md/ })).toBeTruthy();
  });

  it("gives its watch up when it goes away, and leaves another owner's in place", async () => {
    serve("/api/team/workspace/files");
    const hubSocket = openHubSocket();
    const wiki = hub.teamWatches.register({ changed: () => {} });
    wiki.set(["team/wiki"]);

    const { unmount } = render(FilesPlace, { source: TEAM });
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
  });
});

describe("the tree's rows", () => {
  it("lists folders first, marks empty folders, and opens and closes a folder", async () => {
    serve("/api/agents/scout/workspace/files");
    listings[""] = [file("SOUL.md"), folder("inbox"), folder("memory")];
    listings.memory = [file("notes.md")];
    render(FilesPlace, { source: SCOUT });

    const rows = await screen.findAllByRole("button", { name: /^(inbox|memory|SOUL\.md)/ });
    expect(rows.map((row) => row.getAttribute("data-path"))).toEqual([
      "inbox",
      "memory",
      "SOUL.md",
    ]);

    await userEvent.click(screen.getByRole("button", { name: "inbox" }));
    expect(await screen.findByText("Empty")).toBeTruthy();
    const memory = screen.getByRole("button", { name: "memory" });
    await userEvent.click(memory);
    expect(memory.getAttribute("aria-expanded")).toBe("true");
    await screen.findByRole("button", { name: /^notes\.md/ });
    await userEvent.click(memory);
    expect(screen.queryByRole("button", { name: /^notes\.md/ })).toBeNull();
  });

  it("says why a folder couldn't be listed, and lists it again on Try again", async () => {
    let fail = true;
    mockFetch(() =>
      fail ? new Response("boom", { status: 500 }) : jsonResponse([file("SOUL.md")]),
    );
    vi.spyOn(console, "error").mockImplementation(() => {});
    render(FilesPlace, { source: SCOUT });
    expect((await screen.findByRole("alert")).textContent).toMatch(/Couldn't list these files\./);

    fail = false;
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("button", { name: /^SOUL\.md/ })).toBeTruthy();
  });

  it("deletes a file at once and offers Undo from the checkpoint taken first", async () => {
    serve("/api/agents/scout/workspace/files");
    listings[""] = [file("SOUL.md")];
    const success = vi.spyOn(toast, "success");
    render(FilesPlace, { source: SCOUT });

    await userEvent.click(await screen.findByRole("button", { name: "More for SOUL.md" }));
    listings[""] = [];
    await userEvent.click(screen.getByRole("menuitem", { name: "Delete" }));
    await settle();
    expect(requests.some((r) => r.method === "DELETE" && r.url.includes("path=SOUL.md"))).toBe(
      true,
    );
    const [message, action] = success.mock.calls.at(-1) ?? [];
    expect(message).toBe("Deleted SOUL.md.");
    expect(action?.label).toBe("Undo");

    listings[""] = [file("SOUL.md")];
    action?.onClick();
    await settle();
    const restore = requests.find((r) => r.url.includes("/restore"));
    expect(restore?.url).toBe("/api/agents/scout/checkpoints/cp1/restore");
    expect(JSON.parse(restore?.body ?? "{}")).toEqual({ repo: "workspace", path: "SOUL.md" });
    expect(await screen.findByRole("button", { name: /^SOUL\.md/ })).toBeTruthy();
  });

  it("renames inline, and a slash moves the file into a folder", async () => {
    serve("/api/agents/scout/workspace/files");
    listings[""] = [folder("notes"), file("draft.md")];
    render(FilesPlace, { source: SCOUT });

    await userEvent.click(await screen.findByRole("button", { name: "More for draft.md" }));
    await userEvent.click(screen.getByRole("menuitem", { name: "Rename" }));
    const input = screen.getByRole("textbox", { name: "New name for draft.md" });
    await userEvent.clear(input);
    await userEvent.type(input, "notes/plan.md{Enter}");
    await settle();
    const move = requests.find((r) => r.url.endsWith("/workspace/move"));
    expect(JSON.parse(move?.body ?? "{}")).toEqual({
      from: "draft.md",
      to: "notes/plan.md",
      overwrite: false,
    });
  });

  it("offers no rename or delete for an agent's config files, which only the coordinator writes", async () => {
    serve("/api/agents/scout/workspace/files");
    listings[""] = [folder("config")];
    listings.config = [file("config.toml"), file("channels.toml")];
    render(FilesPlace, { source: SCOUT });
    await userEvent.click(await screen.findByRole("button", { name: "config" }));

    await userEvent.click(await screen.findByRole("button", { name: "More for config.toml" }));
    expect(screen.getAllByRole("menuitem").map((item) => item.textContent.trim())).toEqual([
      "History",
    ]);
    await userEvent.keyboard("{Escape}");
    await userEvent.click(screen.getByRole("button", { name: "More for channels.toml" }));
    expect(screen.getAllByRole("menuitem").map((item) => item.textContent.trim())).toEqual([
      "History",
      "Rename",
      "Delete",
    ]);
  });
});
