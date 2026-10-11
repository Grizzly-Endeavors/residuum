import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeWebSocket } from "../test/fake-websocket";
import { setViewedAgent } from "./viewed-agent";
import { ws } from "./ws.svelte";
import type { WatchHandler, WatchOwner } from "./watch-registry";

/** The watch frames a socket was sent, parsed. */
function watchFrames(socket: FakeWebSocket): unknown[] {
  return socket.sentFrames().filter((f) => (f as { type: string }).type === "watch_workspace");
}

const nothing: WatchHandler = { changed: () => {} };

let owners: WatchOwner[] = [];

function register(handler: WatchHandler = nothing, agent?: string): WatchOwner {
  const owner = ws.watches.register(handler, agent === undefined ? {} : { agent });
  owners.push(owner);
  return owner;
}

beforeEach(() => {
  FakeWebSocket.install();
  vi.stubGlobal("location", { protocol: "http:", host: "localhost:7700" });
  // Connecting loads history and usage; none of it matters here, so it never answers.
  vi.stubGlobal(
    "fetch",
    vi.fn(() => new Promise<Response>(() => {})),
  );
});

afterEach(() => {
  for (const owner of owners) owner.release();
  owners = [];
  setViewedAgent(null);
  vi.unstubAllGlobals();
});

describe("the agent socket's watch registry", () => {
  it("sends the union of its owners' prefixes once the socket opens", () => {
    setViewedAgent("scout");
    register().set(["config"]);
    register().set(["team/workbench", "config"]);
    expect(watchFrames(FakeWebSocket.last)).toEqual([]);

    FakeWebSocket.last.simulateOpen();
    expect(watchFrames(FakeWebSocket.last)).toEqual([
      { type: "watch_workspace", prefixes: ["config", "team/workbench"] },
    ]);

    register().set(["memory"]);
    expect(watchFrames(FakeWebSocket.last).at(-1)).toEqual({
      type: "watch_workspace",
      prefixes: ["config", "memory", "team/workbench"],
    });
  });

  it("sends the union again after a reconnect", () => {
    vi.useFakeTimers();
    try {
      setViewedAgent("scout");
      FakeWebSocket.last.simulateOpen();
      register().set(["config"]);
      register().set(["memory"]);

      FakeWebSocket.last.simulateClose();
      vi.advanceTimersByTime(1000);
      const reconnected = FakeWebSocket.last;
      expect(FakeWebSocket.sockets).toHaveLength(2);
      reconnected.simulateOpen();
      expect(watchFrames(reconnected)).toEqual([
        { type: "watch_workspace", prefixes: ["config", "memory"] },
      ]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("sends the prefixes that still apply on a bound-agent switch", () => {
    setViewedAgent("scout");
    const first = FakeWebSocket.last;
    first.simulateOpen();
    register().set(["config"]);
    register(nothing, "scout").set([""]);
    register(nothing, "atlas").set(["memory"]);
    expect(watchFrames(first).at(-1)).toEqual({
      type: "watch_workspace",
      prefixes: ["", "config"],
    });

    setViewedAgent("atlas");
    const second = FakeWebSocket.last;
    expect(second).not.toBe(first);
    second.simulateOpen();
    expect(watchFrames(second)).toEqual([
      { type: "watch_workspace", prefixes: ["config", "memory"] },
    ]);
  });

  it("hands each owner its own changes, and every owner a resync, from the socket's frames", () => {
    setViewedAgent("scout");
    FakeWebSocket.last.simulateOpen();
    const heard: string[] = [];
    register(
      {
        changed: (changes) => heard.push(`config:${changes.map((c) => c.path).join(",")}`),
        resync: (reason) => heard.push(`config:${reason}`),
      },
      "scout",
    ).set(["config"]);
    register({
      changed: (changes) => heard.push(`wiki:${changes.map((c) => c.path).join(",")}`),
      resync: (reason) => heard.push(`wiki:${reason}`),
    }).set(["team/wiki"]);

    FakeWebSocket.last.simulateMessage({
      type: "workspace_changed",
      changes: [
        { path: "config/model.toml", kind: "modified" },
        { path: "team/wiki/a.md", kind: "created" },
        { path: "notes.md", kind: "modified" },
      ],
    });
    FakeWebSocket.last.simulateMessage({ type: "workspace_resync", reason: "watcher_restarted" });

    expect(heard).toEqual([
      "config:config/model.toml",
      "wiki:team/wiki/a.md",
      "config:watcher_restarted",
      "wiki:watcher_restarted",
    ]);
  });

  it("does not hear the old agent's changes after a switch", () => {
    setViewedAgent("scout");
    FakeWebSocket.last.simulateOpen();
    const heard: string[] = [];
    register({ changed: (changes) => heard.push(...changes.map((c) => c.path)) }, "scout").set([
      "",
    ]);

    setViewedAgent("atlas");
    FakeWebSocket.last.simulateOpen();
    FakeWebSocket.last.simulateMessage({
      type: "workspace_changed",
      changes: [{ path: "memory/a.md", kind: "modified" }],
    });
    expect(heard).toEqual([]);
  });
});
