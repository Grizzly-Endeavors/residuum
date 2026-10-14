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

const USER_ORIGIN = { endpoint: "ws", visibility: "user" };

/**
 * Scout's history once a turn is recorded: the earlier message, unless it was
 * compressed into the episode `cursor`, then the finished turn.
 */
function recordedTurn(
  cursor: string | null,
  turnId: string,
  question: string,
  answer: string,
): Record<string, unknown> {
  const at = { timestamp: "2026-09-29T12:01:00Z", project_context: "", turn_id: turnId };
  return {
    kind: "recent",
    messages: [
      ...(cursor === null ? (history("hello from scout").messages as unknown[]) : []),
      { role: "user", content: question, ...at },
      { role: "assistant", content: answer, ...at },
    ],
    next_cursor: cursor,
  };
}

/** The id of the last message the page sent on `socket`, which names the turn it starts. */
function sentMessageId(socket: FakeWebSocket): string {
  const sent = socket
    .sentFrames()
    .filter((frame) => (frame as { type?: string }).type === "send_message");
  return (sent[sent.length - 1] as { id: string }).id;
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
  /** Answer this agent's history requests from now on with `segment`. */
  setHistory: (agent: string, segment: Record<string, unknown>) => void;
}

function installServer(): FakeServer {
  const urls: string[] = [];
  const held = new Map<string, Promise<void>>();
  const histories = new Map<string, Record<string, unknown>>();
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string) => {
      urls.push(input);
      const agent = /\/api\/agents\/([^/]+)\//.exec(input)?.[1] ?? "";
      const episode = /[?&]episode=([^&]+)/.exec(input)?.[1];
      if (episode !== undefined) {
        return json({
          ...history(`hello from ${agent}`),
          kind: "episode",
          episode_id: episode,
          date: "2026-09-29",
        });
      }
      if (input.includes("/chat/history")) {
        // The history as it stood when asked, however long the answer takes.
        const segment = histories.get(agent) ?? history(`hello from ${agent}`);
        await held.get(agent);
        held.delete(agent);
        return json(segment);
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
    setHistory: (agent, segment) => histories.set(agent, segment),
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

describe("a turn that ends while the history is on its way stays in the feed", () => {
  it("on binding the agent", async () => {
    const server = installServer();
    const held = server.hold("scout");
    setViewedAgent("scout");
    const socket = FakeWebSocket.last;
    socket.simulateOpen();
    await flush();
    ws.sendChat("sent early");
    const turnId = sentMessageId(socket);
    socket.simulateMessage({ type: "turn_started", reply_to: turnId, origin: USER_ORIGIN });
    socket.simulateMessage({ type: "response", reply_to: turnId, endpoint: "ws", content: "done" });
    // History records the turn before its end reaches the page.
    server.setHistory("scout", recordedTurn(null, turnId, "sent early", "done"));
    socket.simulateMessage({ type: "turn_ended", reply_to: turnId });
    held.release();
    await flush();

    expect(feedText()).toEqual(["hello from scout", "sent early", "done"]);
  });

  it("on reconnecting after history was compressed into a new episode", async () => {
    const server = installServer();
    setViewedAgent("scout");
    const first = FakeWebSocket.last;
    first.simulateOpen();
    await flush();
    ws.sendChat("tidy the wiki");
    const turnId = sentMessageId(first);
    first.simulateMessage({ type: "turn_started", reply_to: turnId, origin: USER_ORIGIN });
    first.simulateClose();
    // While the page is away the earlier message is compressed into ep-004;
    // the turn in flight isn't recorded yet.
    server.setHistory("scout", { kind: "recent", messages: [], next_cursor: "ep-004" });
    const held = server.hold("scout");
    ws.transport.reconnectNow();
    const second = FakeWebSocket.last;
    second.simulateOpen();
    await flush();
    second.simulateMessage({ type: "response", reply_to: turnId, endpoint: "ws", content: "done" });
    server.setHistory("scout", recordedTurn("ep-004", turnId, "tidy the wiki", "done"));
    second.simulateMessage({ type: "turn_ended", reply_to: turnId });
    held.release();
    await flush();

    expect(feedText()).toEqual(["hello from scout", "tidy the wiki", "done"]);
  });
});
