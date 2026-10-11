import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeWebSocket } from "../test/fake-websocket";
import { setViewedAgent } from "./viewed-agent";
import { ws } from "./ws.svelte";
import { scheduled } from "./scheduled.svelte";

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
        await held.get(`${agent}:usage`);
        const tokens = agent === "scout" ? 111 : 222;
        return json({
          input_tokens: tokens,
          output_tokens: tokens,
          context_tokens: null,
          tool_calls: 0,
        });
      }
      if (input.includes("/sessions")) {
        return json({ live: [], completed: [], next_cursor: null });
      }
      if (input.includes("/a2a/outbound")) return json([]);
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
  setViewedAgent(null);
  vi.unstubAllGlobals();
});

describe("agent connection follows the viewed agent", () => {
  it("opens the agent's own socket and loads its history", async () => {
    const server = installServer();
    setViewedAgent("scout");
    expect(FakeWebSocket.last.url).toBe("ws://localhost:7700/api/agents/scout/ws");
    FakeWebSocket.last.simulateOpen();
    await flush();

    expect(server.urls).toContain("/api/agents/scout/chat/history");
    expect(feedText()).toEqual(["hello from scout"]);
    expect(ws.agent).toBe("scout");
  });

  it("keeps a message sent before the history arrived, after the history", async () => {
    const server = installServer();
    const held = server.hold("scout");
    setViewedAgent("scout");
    FakeWebSocket.last.simulateOpen();
    ws.sendChat("sent early");
    held.release();
    await flush();

    expect(feedText()).toEqual(["hello from scout", "sent early"]);
  });

  it("closes the old socket and opens the new agent's on a switch", () => {
    installServer();
    setViewedAgent("scout");
    const first = FakeWebSocket.last;
    first.simulateOpen();
    setViewedAgent("atlas");

    expect(first.readyState).toBe(FakeWebSocket.CLOSED);
    expect(FakeWebSocket.sockets).toHaveLength(2);
    expect(FakeWebSocket.last.url).toBe("ws://localhost:7700/api/agents/atlas/ws");
  });

  it("does not reconnect the old agent after it is left", () => {
    vi.useFakeTimers();
    try {
      installServer();
      setViewedAgent("scout");
      const first = FakeWebSocket.last;
      first.simulateOpen();
      setViewedAgent("atlas");
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
    setViewedAgent("scout");
    const socket = FakeWebSocket.last;
    setViewedAgent(null);
    expect(socket.readyState).toBe(FakeWebSocket.CLOSED);
    expect(ws.agent).toBeNull();
  });
});

describe("switching agents leaves nothing of the old agent behind", () => {
  it("replaces the feed, sessions, and scheduled state", async () => {
    installServer();
    setViewedAgent("scout");
    FakeWebSocket.last.simulateOpen();
    await flush();
    scheduled.pulses = [{ name: "p" } as never];
    scheduled.loaded = true;
    ws.sessions.live = [{ run_id: "r1" } as never];
    ws.store.sessionUsage = { input_tokens: 5, output_tokens: 5, cost: null } as never;
    expect(feedText()).toEqual(["hello from scout"]);
    const scoutStore = ws.store;
    const scoutSessions = ws.sessions;

    setViewedAgent("atlas");

    expect(ws.store).not.toBe(scoutStore);
    expect(ws.sessions).not.toBe(scoutSessions);
    expect(ws.store.feed).toEqual([]);
    expect(ws.store.sessionUsage).toBeNull();
    expect(ws.sessions.live).toEqual([]);
    expect(scheduled.pulses).toEqual([]);
    expect(scheduled.loaded).toBe(false);
  });

  it("loads the new agent's history and usage, from its own paths", async () => {
    const server = installServer();
    setViewedAgent("scout");
    FakeWebSocket.last.simulateOpen();
    await flush();
    server.urls.length = 0;

    setViewedAgent("atlas");
    FakeWebSocket.last.simulateOpen();
    await flush();

    expect(feedText()).toEqual(["hello from atlas"]);
    expect(server.urls.length).toBeGreaterThan(0);
    expect(server.urls.every((url) => url.startsWith("/api/agents/atlas/"))).toBe(true);
  });

  it("drops a message queued for the old agent instead of sending it to the new one", () => {
    installServer();
    setViewedAgent("scout");
    ws.sendChat("for scout only");
    expect(ws.transport.pendingCount).toBe(1);

    setViewedAgent("atlas");
    expect(ws.transport.pendingCount).toBe(0);
    FakeWebSocket.last.simulateOpen();

    // Only the frames every connection starts with: the tool frames turned
    // on, and the turn in flight asked for.
    expect(FakeWebSocket.last.sent).toEqual([
      JSON.stringify({ type: "set_verbose", enabled: true }),
      JSON.stringify({ type: "resync_turn" }),
    ]);
  });

  it("ignores frames that arrive from the old agent's socket after the switch", async () => {
    installServer();
    setViewedAgent("scout");
    const first = FakeWebSocket.last;
    first.simulateOpen();
    setViewedAgent("atlas");
    FakeWebSocket.last.simulateOpen();
    await flush();

    first.simulateMessage({ type: "response", reply_to: "x", content: "late scout reply" });

    expect(feedText()).toEqual(["hello from atlas"]);
  });

  it("does not show history that was still loading for the old agent", async () => {
    const server = installServer();
    const slow = server.hold("scout");
    setViewedAgent("scout");
    FakeWebSocket.last.simulateOpen();
    await flush();

    setViewedAgent("atlas");
    FakeWebSocket.last.simulateOpen();
    await flush();
    expect(feedText()).toEqual(["hello from atlas"]);

    slow.release();
    await flush();
    expect(feedText()).toEqual(["hello from atlas"]);
  });

  it("does not let the old agent's late usage totals land on the new agent", async () => {
    const server = installServer();
    const slow = server.hold("scout:usage");
    setViewedAgent("scout");
    FakeWebSocket.last.simulateOpen();
    setViewedAgent("atlas");
    await flush();
    expect(ws.store.sessionUsage?.input_tokens).toBe(222);

    slow.release();
    await flush();
    expect(ws.store.sessionUsage?.input_tokens).toBe(222);
  });

  it("starts the new agent's connection state fresh", async () => {
    installServer();
    setViewedAgent("scout");
    FakeWebSocket.last.simulateOpen();
    await flush();
    ws.store.handleMessage({ type: "turn_started", reply_to: "t1" } as never);
    expect(ws.store.isProcessing).toBe(true);

    setViewedAgent("atlas");

    expect(ws.store.isProcessing).toBe(false);
    expect(ws.store.activeTurnId).toBeNull();
  });
});
