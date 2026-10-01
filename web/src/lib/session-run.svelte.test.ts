import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { HubClientMessage, HubServerMessage } from "./hub-types";
import { SessionRun, type SessionRelayLink } from "./session-run.svelte";
import type { RecentMessage, ServerMessage, SessionSummary } from "./types";

const ADDRESS = "spawned-research-3f9a";

function summary(runId: string, overrides: Partial<SessionSummary> = {}): SessionSummary {
  return {
    address: ADDRESS,
    run_id: runId,
    category: "spawned",
    source_label: "agent:researcher",
    state: "running",
    spawner: "main",
    depth: 1,
    purpose: "Compare fallback strategies",
    started_at: "2026-09-23T12:00:00Z",
    completed_at: null,
    episode_id: null,
    interrupted: false,
    usage: { input_tokens: 0, output_tokens: 0, context_tokens: null, tool_calls: 0 },
    outcome: null,
    error: null,
    error_details: null,
    overlap: null,
    ...overrides,
  };
}

/** A hub socket the test drives: what the run sent, and frames to hand it. */
class FakeRelay implements SessionRelayLink {
  connected = true;
  sent: HubClientMessage[] = [];
  private listeners = new Set<(msg: HubServerMessage) => void>();
  send(msg: HubClientMessage): void {
    this.sent.push(msg);
  }
  onFrame(listener: (msg: HubServerMessage) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }
  emit(msg: HubServerMessage): void {
    for (const listener of this.listeners) listener(msg);
  }
  relay(agent: string, frame: ServerMessage): void {
    this.emit({ type: "session_frame", agent, frame });
  }
}

/** What the agent's routes answer, and the requests they saw. */
let transcripts: Record<string, { session: SessionSummary; messages: RecentMessage[] }>;
let messageAnswer: () => Response;
let requests: string[];
/** While set, transcripts answer only once it resolves. */
let holdTranscripts: Promise<void> | null;

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

beforeEach(() => {
  requests = [];
  transcripts = {
    "run-1": {
      session: summary("run-1"),
      messages: [
        {
          role: "user",
          content: "Research fallbacks.",
          timestamp: "2026-09-23T12:00:00Z",
          visibility: "user",
        },
      ],
    },
  };
  messageAnswer = () => json({ outcome: "live" });
  holdTranscripts = null;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      requests.push(`${init?.method ?? "GET"} ${url}`);
      const run = /\/runs\/([^/]+)\/transcript$/.exec(url)?.[1];
      if (run !== undefined) {
        if (holdTranscripts !== null) await holdTranscripts;
        const found = transcripts[run];
        return found ? json(found) : json({ error: "no such run" }, 404);
      }
      if (url.endsWith("/messages")) return messageAnswer();
      return json({ address: ADDRESS }, 202);
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
});

async function opened(
  known: ConstructorParameters<typeof SessionRun>[2] = {},
): Promise<{ run: SessionRun; relay: FakeRelay; close: () => void }> {
  const relay = new FakeRelay();
  const run = new SessionRun("atlas", "run-1", known, relay);
  const close = run.open();
  await vi.waitFor(() => {
    expect(run.loaded).toBe(true);
  });
  return { run, relay, close };
}

describe("following a run through the hub's session relay", () => {
  it("subscribes at once when it knows the address, and stops on close", async () => {
    const { relay, close } = await opened({ address: ADDRESS });
    expect(relay.sent).toEqual([{ type: "subscribe_session", agent: "atlas", address: ADDRESS }]);
    close();
    expect(relay.sent.at(-1)).toEqual({
      type: "unsubscribe_session",
      agent: "atlas",
      address: ADDRESS,
    });
  });

  it("learns the address from the transcript, and subscribes then", async () => {
    const { relay } = await opened();
    expect(relay.sent).toEqual([{ type: "subscribe_session", agent: "atlas", address: ADDRESS }]);
  });

  it("reads a live run again once the subscription is active, and after a lag", async () => {
    const { relay } = await opened({ address: ADDRESS });
    expect(requests).toHaveLength(1);
    relay.emit({ type: "subscribed", kind: "session", agent: "atlas", address: ADDRESS });
    await vi.waitFor(() => {
      expect(requests).toHaveLength(2);
    });
    relay.emit({ type: "session_relay_lagged" });
    await vi.waitFor(() => {
      expect(requests).toHaveLength(3);
    });
  });

  it("subscribes again on every new hub connection", async () => {
    const { relay } = await opened({ address: ADDRESS });
    relay.emit({ type: "hub_boot", boot_id: "boot-2" });
    expect(relay.sent.filter((m) => m.type === "subscribe_session")).toHaveLength(2);
  });

  it("waits for the hub socket before subscribing", () => {
    const relay = new FakeRelay();
    relay.connected = false;
    const run = new SessionRun("atlas", "run-1", { address: ADDRESS }, relay);
    run.open();
    expect(relay.sent).toEqual([]);
    relay.connected = true;
    relay.emit({ type: "hub_boot", boot_id: "boot-1" });
    expect(relay.sent).toHaveLength(1);
  });

  it("shows its own run's frames and nothing of other sessions or agents", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    relay.relay("atlas", {
      type: "session_response",
      address: ADDRESS,
      run_id: "run-1",
      turn_id: "t1",
      content: "Cascade first.",
    });
    relay.relay("scout", {
      type: "session_response",
      address: ADDRESS,
      run_id: "run-1",
      turn_id: "t1",
      content: "Not atlas's.",
    });
    relay.relay("atlas", {
      type: "session_response",
      address: "spawned-other",
      run_id: "run-9",
      turn_id: "t2",
      content: "Another session.",
    });
    relay.relay("atlas", {
      type: "session_state_changed",
      address: ADDRESS,
      run_id: "run-1",
      state: "idle",
    });
    expect(run.items.map((item) => ("content" in item ? item.content : item.kind))).toEqual([
      "Research fallbacks.",
      "Cascade first.",
    ]);
    expect(run.summary?.state).toBe("idle");
  });

  it("takes the outcome of a run that finishes", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    relay.relay("atlas", {
      type: "session_completed",
      address: ADDRESS,
      run_id: "run-1",
      status: "failed",
      error: "the site timed out",
      error_details: "timeout after 30s",
      episode_id: null,
    });
    expect(run.summary).toMatchObject({
      state: "completed",
      outcome: "failed",
      error: "the site timed out",
      error_details: "timeout after 30s",
    });
    expect(run.items.at(-1)).toMatchObject({
      kind: "status",
      tone: "error",
      content: "Session failed: the site timed out",
    });
  });

  it("tags the message that started a turn, so it can offer Undo this turn", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    run.draft = "Check outages too.";
    await run.send();
    relay.relay("atlas", {
      type: "session_turn_ended",
      address: ADDRESS,
      run_id: "run-1",
      turn_id: "turn-7",
    });
    const sent = run.items.find(
      (item) => item.kind === "user" && item.content === "Check outages too.",
    );
    expect(sent).toMatchObject({ turn: { turnId: "turn-7", changed: null } });
  });
});

describe("the run's commands", () => {
  it("sends the draft as the owner and says where it landed", async () => {
    const { run } = await opened({ address: ADDRESS });
    run.draft = "  Weigh safety over speed.  ";
    await run.send();
    expect(requests).toContain(`POST /api/agents/atlas/sessions/${ADDRESS}/messages`);
    expect(run.draft).toBe("");
    expect(run.items.slice(-2)).toMatchObject([
      { kind: "user", content: "Weigh safety over speed." },
      { kind: "status", tone: "info", content: "Delivered." },
    ]);
  });

  it("keeps the draft and says why when the message couldn't be sent", async () => {
    messageAnswer = () =>
      json({ error: "The session is busy. Try again shortly.", code: "busy" }, 409);
    const { run } = await opened({ address: ADDRESS });
    run.draft = "Hurry up";
    await run.send();
    expect(run.draft).toBe("Hurry up");
    expect(run.items.at(-1)).toMatchObject({
      kind: "status",
      tone: "error",
      content: "Couldn't send it. The session is busy. Try again shortly.",
    });
  });

  it("follows a finished session into the run its message started", async () => {
    transcripts["run-1"] = { session: summary("run-1", { state: "completed" }), messages: [] };
    messageAnswer = () => json({ outcome: "resumed" });
    const { run, relay } = await opened({ address: ADDRESS });
    run.draft = "Pick this back up";
    const sending = run.send();
    // The new run can announce itself before the message's answer.
    relay.relay("atlas", { type: "session_started", session: summary("run-2") });
    await sending;
    expect(run.runId).toBe("run-2");
    expect(run.items.map((item) => item.kind)).toContain("divider");
    expect(run.items.at(-1)).toMatchObject({
      content: "This session had finished, so your message started a new run.",
    });
  });

  it("keeps its notes, and shows a reply once, when the new run races its transcript", async () => {
    const finished = summary("run-1", { state: "completed" });
    transcripts["run-1"] = { session: finished, messages: [] };
    transcripts["run-2"] = {
      session: summary("run-2", { state: "idle" }),
      messages: [
        { role: "user", content: "Again", timestamp: "2026-09-23T12:00:00Z", visibility: "user" },
        {
          role: "assistant",
          content: "Picking this back up.",
          timestamp: "2026-09-23T12:00:00Z",
          visibility: "user",
        },
      ],
    };
    messageAnswer = () => json({ outcome: "resumed" });
    let release = (): void => {};
    holdTranscripts = new Promise((resolve) => {
      release = resolve;
    });
    const relay = new FakeRelay();
    const run = new SessionRun("atlas", "run-1", { summary: finished }, relay);
    run.open();
    run.draft = "Again";
    const sending = run.send();
    relay.relay("atlas", { type: "session_started", session: summary("run-2") });
    relay.relay("atlas", {
      type: "session_response",
      address: ADDRESS,
      run_id: "run-2",
      turn_id: "t1",
      content: "Picking this back up.",
    });
    await sending;
    release();
    await vi.waitFor(() => {
      expect(run.loaded).toBe(true);
    });
    expect(run.runId).toBe("run-2");
    expect(run.items.map((item) => ("content" in item ? item.content : item.kind))).toEqual([
      "Again",
      "Picking this back up.",
      "This session had finished, so your message started a new run.",
    ]);
  });

  it("doesn't follow a new run it didn't start", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    relay.relay("atlas", { type: "session_started", session: summary("run-2") });
    expect(run.runId).toBe("run-1");
  });

  it("stops the run, and says so when it can't", async () => {
    const { run } = await opened({ address: ADDRESS });
    await run.stop();
    expect(requests).toContain(`POST /api/agents/atlas/sessions/${ADDRESS}/stop`);
    expect(run.stopping).toBe(true);
    expect(run.items.at(-1)).toMatchObject({ content: "Stopping this session…" });

    run.stopping = false;
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(json({ error: "not live", code: "not_live" }, 404))),
    );
    await run.stop();
    expect(run.stopping).toBe(false);
    expect(run.items.at(-1)).toMatchObject({
      tone: "error",
      content: "Couldn't stop it. It had already finished.",
    });
  });

  it("says why a transcript couldn't load", async () => {
    const relay = new FakeRelay();
    const run = new SessionRun("atlas", "run-gone", { address: ADDRESS }, relay);
    await run.load();
    expect(run.loaded).toBe(false);
    expect(run.loadError).toBe(
      "Couldn't load this session's transcript. Its history isn't available. It may be from before a restart.",
    );
  });
});
