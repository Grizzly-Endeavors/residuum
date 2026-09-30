import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeWebSocket } from "../test/fake-websocket";
import { setCurrentAgent } from "./paths";
import { ws } from "./ws.svelte";
import { userInbox } from "./inbox.svelte";
import { scheduled } from "./scheduled.svelte";
import type { UserInboxItem } from "./types";

const INBOX_ITEM: UserInboxItem = {
  id: "item-1",
  title: "For scout",
  body: "",
  source: "agent",
  timestamp: "2026-09-29T12:00:00Z",
  read: false,
  attachments: [],
};

/** A history segment whose only message says `text`. */
function history(text: string): Record<string, unknown> {
  return {
    kind: "recent",
    messages: [
      { role: "user", content: text, timestamp: "2026-09-29T12:00:00Z", project_context: "" },
    ],
    next_cursor: null,
  };
}

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
}

/** What the fake server answers, and the URLs it was asked for. */
interface FakeServer {
  urls: string[];
  /** Hold the next history request for this agent until `release` is called. */
  hold: (agent: string) => { release: () => void };
}

function installServer(): FakeServer {
  const urls: string[] = [];
  const held = new Map<string, Promise<void>>();
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string) => {
      urls.push(input);
      const agent = /\/api\/agents\/([^/]+)\//.exec(input)?.[1] ?? "";
      if (input.includes("/chat/history")) {
        await held.get(agent);
        held.delete(agent);
        return json(history(`hello from ${agent}`));
      }
      if (input.includes("/usage")) {
        return json({ input_tokens: 1, output_tokens: 1, cost: null });
      }
      if (input.includes("/sessions")) {
        return json({ live: [], completed: [], next_cursor: null });
      }
      if (input.includes("/a2a/outbound")) return json([]);
      if (input.includes("/inbox")) return json([INBOX_ITEM]);
      if (input.includes("/scheduled/")) return json([]);
      return new Response("unexpected", { status: 500 });
    }),
  );
  return {
    urls,
    hold: (agent) => {
      let release = (): void => {};
      held.set(
        agent,
        new Promise<void>((resolve) => {
          release = resolve;
        }),
      );
      return { release };
    },
  };
}

/** Let promise callbacks (fetches and the stores' follow-ups) run. */
async function flush(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
}

function feedText(): string[] {
  return ws.store.feed.flatMap((item) => ("content" in item ? [item.content] : []));
}

beforeEach(() => {
  FakeWebSocket.install();
  vi.stubGlobal("location", { protocol: "http:", host: "localhost:7700" });
});

afterEach(() => {
  setCurrentAgent(null);
  vi.unstubAllGlobals();
});

describe("agent connection follows the current agent", () => {
  it("opens the agent's own socket and loads its history", async () => {
    const server = installServer();
    setCurrentAgent("scout");
    expect(FakeWebSocket.last.url).toBe("ws://localhost:7700/api/agents/scout/ws");
    FakeWebSocket.last.simulateOpen();
    await flush();

    expect(server.urls).toContain("/api/agents/scout/chat/history");
    expect(feedText()).toEqual(["hello from scout"]);
    expect(ws.agent).toBe("scout");
  });

  it("closes the old socket and opens the new agent's on a switch", () => {
    installServer();
    setCurrentAgent("scout");
    const first = FakeWebSocket.last;
    first.simulateOpen();
    setCurrentAgent("atlas");

    expect(first.readyState).toBe(FakeWebSocket.CLOSED);
    expect(FakeWebSocket.sockets).toHaveLength(2);
    expect(FakeWebSocket.last.url).toBe("ws://localhost:7700/api/agents/atlas/ws");
  });

  it("does not reconnect the old agent after it is left", () => {
    vi.useFakeTimers();
    try {
      installServer();
      setCurrentAgent("scout");
      const first = FakeWebSocket.last;
      first.simulateOpen();
      setCurrentAgent("atlas");
      first.simulateClose();
      vi.advanceTimersByTime(60_000);
      expect(FakeWebSocket.sockets.map((s) => s.url)).toEqual([
        "ws://localhost:7700/api/agents/scout/ws",
        "ws://localhost:7700/api/agents/atlas/ws",
      ]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("closes the connection when no agent is current", () => {
    installServer();
    setCurrentAgent("scout");
    const socket = FakeWebSocket.last;
    setCurrentAgent(null);
    expect(socket.readyState).toBe(FakeWebSocket.CLOSED);
    expect(ws.agent).toBeNull();
  });
});

describe("switching agents leaves nothing of the old agent behind", () => {
  it("replaces the feed, sessions, inbox, and scheduled state", async () => {
    installServer();
    setCurrentAgent("scout");
    FakeWebSocket.last.simulateOpen();
    await flush();
    await userInbox.refresh();
    scheduled.pulses = [{ name: "p" } as never];
    scheduled.loaded = true;
    ws.sessions.live = [{ run_id: "r1" } as never];
    ws.store.sessionUsage = { input_tokens: 5, output_tokens: 5, cost: null } as never;
    expect(feedText()).toEqual(["hello from scout"]);
    expect(userInbox.items).toHaveLength(1);
    const scoutStore = ws.store;
    const scoutSessions = ws.sessions;

    setCurrentAgent("atlas");

    expect(ws.store).not.toBe(scoutStore);
    expect(ws.sessions).not.toBe(scoutSessions);
    expect(ws.store.feed).toEqual([]);
    expect(ws.store.sessionUsage).toBeNull();
    expect(ws.sessions.live).toEqual([]);
    expect(userInbox.items).toEqual([]);
    expect(scheduled.pulses).toEqual([]);
    expect(scheduled.loaded).toBe(false);
  });

  it("loads the new agent's history and usage, from its own paths", async () => {
    const server = installServer();
    setCurrentAgent("scout");
    FakeWebSocket.last.simulateOpen();
    await flush();
    server.urls.length = 0;

    setCurrentAgent("atlas");
    FakeWebSocket.last.simulateOpen();
    await flush();

    expect(feedText()).toEqual(["hello from atlas"]);
    expect(server.urls.length).toBeGreaterThan(0);
    expect(server.urls.every((url) => url.startsWith("/api/agents/atlas/"))).toBe(true);
  });

  it("drops a message queued for the old agent instead of sending it to the new one", () => {
    installServer();
    setCurrentAgent("scout");
    ws.sendChat("for scout only");
    expect(ws.transport.pendingCount).toBe(1);

    setCurrentAgent("atlas");
    expect(ws.transport.pendingCount).toBe(0);
    FakeWebSocket.last.simulateOpen();

    expect(FakeWebSocket.last.sent).toEqual([]);
  });

  it("ignores frames that arrive from the old agent's socket after the switch", async () => {
    installServer();
    setCurrentAgent("scout");
    const first = FakeWebSocket.last;
    first.simulateOpen();
    setCurrentAgent("atlas");
    FakeWebSocket.last.simulateOpen();
    await flush();

    first.simulateMessage({ type: "response", reply_to: "x", content: "late scout reply" });

    expect(feedText()).toEqual(["hello from atlas"]);
  });

  it("does not show history that was still loading for the old agent", async () => {
    const server = installServer();
    const slow = server.hold("scout");
    setCurrentAgent("scout");
    FakeWebSocket.last.simulateOpen();
    await flush();

    setCurrentAgent("atlas");
    FakeWebSocket.last.simulateOpen();
    await flush();
    expect(feedText()).toEqual(["hello from atlas"]);

    slow.release();
    await flush();
    expect(feedText()).toEqual(["hello from atlas"]);
  });

  it("does not let the old agent's late usage totals land on the new agent", async () => {
    installServer();
    setCurrentAgent("scout");
    FakeWebSocket.last.simulateOpen();
    setCurrentAgent("atlas");
    await flush();
    expect(ws.store.sessionUsage).toBeNull();
  });

  it("does not send a session command from the old agent's store to the new agent", async () => {
    installServer();
    setCurrentAgent("scout");
    FakeWebSocket.last.simulateOpen();
    await flush();
    const oldSessions = ws.sessions;

    setCurrentAgent("atlas");
    FakeWebSocket.last.simulateOpen();
    await flush();
    const sentBefore = FakeWebSocket.last.sent.length;

    oldSessions.sendMessage("main", "stale command");

    expect(FakeWebSocket.last.sent).toHaveLength(sentBefore);
  });

  it("starts the new agent's connection state fresh", async () => {
    installServer();
    setCurrentAgent("scout");
    FakeWebSocket.last.simulateOpen();
    await flush();
    ws.store.handleMessage({ type: "turn_started", reply_to: "t1" } as never);
    expect(ws.store.isProcessing).toBe(true);

    setCurrentAgent("atlas");

    expect(ws.store.isProcessing).toBe(false);
    expect(ws.store.activeTurnId).toBeNull();
  });
});
