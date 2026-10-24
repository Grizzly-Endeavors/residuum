import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { HubClientMessage, HubServerMessage } from "./hub-types";
import { SessionRun, type SessionRelayLink } from "./session-run.svelte";
import type { RecentMessage, ServerMessage, SessionSummary } from "./types";
import { waitFor } from "../test/wait";

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
    model: null,
    resolved_model: null,
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
/** The bodies of the messages sent to the session, in order. */
let sentBodies: { content: string; id: string }[];
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
  sentBodies = [];
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
  messageAnswer = () => json({ outcome: "live", id: sentBodies.at(-1)?.id });
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
      if (url.endsWith("/messages")) {
        sentBodies.push(JSON.parse(init?.body as string) as { content: string; id: string });
        return messageAnswer();
      }
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
  await waitFor(() => {
    expect(run.loaded).toBe(true);
  });
  return { run, relay, close };
}

/** The frames of a turn starting and ending. */
function turn(
  relay: FakeRelay,
  type: "session_turn_started" | "session_turn_ended",
  turnId: string,
  runId = "run-1",
): void {
  relay.relay("atlas", { type, address: ADDRESS, run_id: runId, turn_id: turnId });
}

/** The session read a message, the owner's unless `from` says otherwise. */
function read(
  relay: FakeRelay,
  message: { id: string; content: string; turnId: string; runId?: string },
  from = "owner",
  fromCategory = "owner",
): void {
  relay.relay("atlas", {
    type: "session_user_message",
    address: ADDRESS,
    run_id: message.runId ?? "run-1",
    id: message.id,
    turn_id: message.turnId,
    content: message.content,
    from,
    from_category: fromCategory,
  });
}

/** Send `content` from the message box; resolves to the id it went out with. */
async function sendMessage(run: SessionRun, content: string): Promise<string> {
  run.draft = content;
  await run.send();
  const body = sentBodies.at(-1);
  expect(body?.content).toBe(content);
  return body?.id ?? "";
}

/** The transcript's lines, by what they say. */
function lines(run: SessionRun): string[] {
  return run.items.map((item) => ("content" in item ? item.content : item.kind));
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
    await waitFor(() => {
      expect(requests).toHaveLength(2);
    });
    relay.emit({ type: "session_relay_lagged" });
    await waitFor(() => {
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

  it("keeps a tool call's server and groups calls by the turn they ran in", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    const call = (id: string, name: string, server: string): void => {
      relay.relay("atlas", {
        type: "session_tool_call",
        address: ADDRESS,
        run_id: "run-1",
        id,
        name,
        arguments: {},
        server,
      });
    };
    relay.relay("atlas", {
      type: "session_turn_started",
      address: ADDRESS,
      run_id: "run-1",
      turn_id: "t1",
    });
    call("c1", "read_file", "files");
    call("c2", "list_dir", "files");
    relay.relay("atlas", {
      type: "session_turn_ended",
      address: ADDRESS,
      run_id: "run-1",
      turn_id: "t1",
    });
    relay.relay("atlas", {
      type: "session_turn_started",
      address: ADDRESS,
      run_id: "run-1",
      turn_id: "t2",
    });
    call("c3", "create_issue", "github");

    expect(run.items.filter((item) => item.kind === "tool-group")).toMatchObject([
      {
        turnId: "t1",
        calls: [
          { id: "c1", name: "read_file", server: "files" },
          { id: "c2", name: "list_dir", server: "files" },
        ],
      },
      { turnId: "t2", calls: [{ id: "c3", name: "create_issue", server: "github" }] },
    ]);
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

    const totals = { input_tokens: 1200, output_tokens: 300, context_tokens: 1500, tool_calls: 4 };
    relay.relay("atlas", {
      type: "session_turn_usage",
      address: ADDRESS,
      run_id: "run-1",
      output_tokens: 300,
      has_usage: true,
      tool_calls: 4,
      session_totals: totals,
    });
    expect(run.summary?.usage).toEqual(totals);
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

  it("keeps no failure details when the finish frame carries none", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    relay.relay("atlas", {
      type: "session_completed",
      address: ADDRESS,
      run_id: "run-1",
      status: "completed",
      error: null,
      episode_id: null,
    });
    expect(run.summary?.outcome).toBe("completed");
    expect(run.summary?.error_details).toBeNull();
  });

  it("tags the message that started a turn, so it can offer Undo this turn", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    const id = await sendMessage(run, "Check outages too.");
    turn(relay, "session_turn_started", "turn-7");
    read(relay, { id, content: "Check outages too.", turnId: "turn-7" });
    turn(relay, "session_turn_ended", "turn-7");
    const sent = run.items.find(
      (item) => item.kind === "user" && item.content === "Check outages too.",
    );
    expect(sent).toMatchObject({ turn: { turnId: "turn-7", changed: null } });
  });
});

describe("when a session's messages were sent", () => {
  it("shows no time for the transcript, which holds the run's start for every message, and a time for what arrives live", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2026, 9, 9, 10, 5, 30));
    try {
      const { run, relay } = await opened({ address: ADDRESS });
      relay.relay("atlas", {
        type: "session_response",
        address: ADDRESS,
        run_id: "run-1",
        turn_id: "t1",
        content: "Here are the fallbacks.",
      });
      const id = await sendMessage(run, "Thanks.");
      read(relay, { id, content: "Thanks.", turnId: "t2" });

      const said = run.items.filter((item) => item.kind === "user" || item.kind === "assistant");
      expect(said.map((item) => [item.content, item.timestamp])).toEqual([
        ["Research fallbacks.", undefined],
        ["Here are the fallbacks.", "2026-10-09T10:05:30"],
        ["Thanks.", "2026-10-09T10:05:30"],
      ]);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("a message waiting for the session to read it", () => {
  const sentItem = (run: SessionRun, content: string): unknown =>
    run.items.find((item) => item.kind === "user" && item.content === content);

  it("waits apart from the transcript until the session reads it", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    const id = await sendMessage(run, "Weigh safety over speed.");
    expect(id).toMatch(/^web-/);
    expect(run.waiting).toEqual([{ id, content: "Weigh safety over speed." }]);
    expect(sentItem(run, "Weigh safety over speed.")).toBeUndefined();

    turn(relay, "session_turn_started", "t1");
    read(relay, { id, content: "Weigh safety over speed.", turnId: "t1" });
    expect(run.waiting).toEqual([]);
    expect(sentItem(run, "Weigh safety over speed.")).toMatchObject({
      turnId: "t1",
      messageId: id,
    });
  });

  it("joins the turn in flight where the session read it, after what the turn did first", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    turn(relay, "session_turn_started", "t1");
    relay.relay("atlas", {
      type: "session_tool_call",
      address: ADDRESS,
      run_id: "run-1",
      id: "c1",
      name: "read_file",
      arguments: {},
      server: null,
    });
    const id = await sendMessage(run, "Skip the old notes.");
    relay.relay("atlas", {
      type: "session_tool_result",
      address: ADDRESS,
      run_id: "run-1",
      tool_call_id: "c1",
      name: "read_file",
      output: "notes",
      is_error: false,
    });
    expect(sentItem(run, "Skip the old notes.")).toBeUndefined();
    read(relay, { id, content: "Skip the old notes.", turnId: "t1" });

    expect(run.items.at(-1)).toMatchObject({
      kind: "user",
      content: "Skip the old notes.",
      turnId: "t1",
      midTurn: true,
    });
    turn(relay, "session_turn_ended", "t1");
    expect(sentItem(run, "Skip the old notes.")).not.toHaveProperty("turn");
  });

  it("starts the next turn when the turn in flight ended before reading it", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    turn(relay, "session_turn_started", "t1");
    const id = await sendMessage(run, "Now summarize.");
    relay.relay("atlas", {
      type: "session_response",
      address: ADDRESS,
      run_id: "run-1",
      turn_id: "t1",
      content: "Here's what I found.",
    });
    turn(relay, "session_turn_ended", "t1");
    expect(run.waiting.map((message) => message.content)).toEqual(["Now summarize."]);

    turn(relay, "session_turn_started", "t2");
    read(relay, { id, content: "Now summarize.", turnId: "t2" });
    turn(relay, "session_turn_ended", "t2");

    const sent = sentItem(run, "Now summarize.");
    expect(sent).toMatchObject({ turnId: "t2", turn: { turnId: "t2", changed: null } });
    expect(sent).not.toHaveProperty("midTurn");
    expect(lines(run).slice(-3)).toEqual(["Delivered.", "Here's what I found.", "Now summarize."]);
  });

  /** The run ends, cancelled: what a Stop ends with. */
  const completed = (relay: FakeRelay, runId = "run-1"): void => {
    relay.relay("atlas", {
      type: "session_completed",
      address: ADDRESS,
      run_id: runId,
      status: "cancelled",
      error: null,
      episode_id: null,
    });
  };

  it("follows the new run that reads a message its stopped run left unread", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    turn(relay, "session_turn_started", "t1");
    const id = await sendMessage(run, "Also check the logs.");
    completed(relay);
    expect(run.waiting.map((message) => message.id)).toEqual([id]);

    // The backend resumes the unread message as a new run.
    relay.relay("atlas", { type: "session_started", session: summary("run-2") });
    turn(relay, "session_turn_started", "run-2-t1", "run-2");
    read(relay, { id, content: "Also check the logs.", turnId: "run-2-t1", runId: "run-2" });

    expect(run.runId).toBe("run-2");
    expect(run.waiting).toEqual([]);
    // It opens the new run's first turn, after the divider.
    expect(run.items.slice(-2).map((item) => item.kind)).toEqual(["divider", "user"]);
    expect(run.items.at(-1)).toMatchObject({
      content: "Also check the logs.",
      turnId: "run-2-t1",
      messageId: id,
    });
    expect(run.items.at(-1)).not.toHaveProperty("midTurn");
  });

  it("follows the new run when the stopped run's end and that run's start race a reload", async () => {
    transcripts["run-2"] = { session: summary("run-2"), messages: [] };
    const { run, relay } = await opened({ address: ADDRESS });
    turn(relay, "session_turn_started", "t1");
    const id = await sendMessage(run, "Also check the logs.");
    let release = (): void => {};
    holdTranscripts = new Promise((resolve) => {
      release = resolve;
    });
    relay.emit({ type: "subscribed", kind: "session", agent: "atlas", address: ADDRESS });
    await waitFor(() => {
      expect(requests.filter((r) => r.endsWith("/transcript"))).toHaveLength(2);
    });
    // Both arrive while the reload is in flight.
    completed(relay);
    relay.relay("atlas", { type: "session_started", session: summary("run-2") });
    release();

    turn(relay, "session_turn_started", "run-2-t1", "run-2");
    read(relay, { id, content: "Also check the logs.", turnId: "run-2-t1", runId: "run-2" });
    await waitFor(() => {
      expect(run.waiting).toEqual([]);
    });
    expect(run.runId).toBe("run-2");
  });

  it("forgets a dismissed message, and still places it if the session reads it", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    turn(relay, "session_turn_started", "t1");
    const id = await sendMessage(run, "Also check the logs.");
    run.dismissWaiting(id);
    expect(run.waiting).toEqual([]);

    read(relay, { id, content: "Also check the logs.", turnId: "t1" });
    expect(sentItem(run, "Also check the logs.")).toMatchObject({ turnId: "t1", messageId: id });
  });

  it("doesn't follow a later run once its stopped run's unread message is dismissed", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    turn(relay, "session_turn_started", "t1");
    const id = await sendMessage(run, "Also check the logs.");
    completed(relay);
    run.dismissWaiting(id);
    relay.relay("atlas", { type: "session_started", session: summary("run-2") });
    expect(run.runId).toBe("run-1");
  });

  it("doesn't follow a later run once the reloaded transcript shows the stopped run read its message", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    turn(relay, "session_turn_started", "t1");
    const id = await sendMessage(run, "Also check the logs.");
    let release = (): void => {};
    holdTranscripts = new Promise((resolve) => {
      release = resolve;
    });
    transcripts["run-1"]?.messages.push({
      role: "user",
      content: "Also check the logs.",
      timestamp: "2026-09-23T12:00:00Z",
      visibility: "user",
      id,
    });
    relay.emit({ type: "subscribed", kind: "session", agent: "atlas", address: ADDRESS });
    await waitFor(() => {
      expect(requests.filter((r) => r.endsWith("/transcript"))).toHaveLength(2);
    });
    completed(relay);
    release();
    await waitFor(() => {
      expect(run.waiting).toEqual([]);
    });
    relay.relay("atlas", { type: "session_started", session: summary("run-2") });
    expect(run.runId).toBe("run-1");
  });

  it("stays on a run that ends with nothing of its waiting", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    completed(relay);
    relay.relay("atlas", { type: "session_started", session: summary("run-2") });
    expect(run.runId).toBe("run-1");
  });

  it("shows what other senders sent the session where it read them", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    turn(relay, "session_turn_started", "t1");
    read(relay, { id: "m-main", content: "Weigh safety.", turnId: "t1" }, "main", "main");
    read(
      relay,
      { id: "m-art", content: "Use the chart.", turnId: "t1" },
      "artifact:wiki-graph",
      "artifact",
    );
    read(relay, { id: "m-tab", content: "From my phone.", turnId: "t1" });

    expect(run.items.slice(-3)).toMatchObject([
      {
        kind: "agent-message",
        from: "main",
        category: "main",
        content: "Weigh safety.",
        turnId: "t1",
      },
      {
        kind: "user",
        content: "Use the chart.",
        sender: { name: "wiki-graph", interface: "workbench artifact" },
        turnId: "t1",
      },
      { kind: "user", content: "From my phone.", turnId: "t1" },
    ]);
    // All three were read as the turn began, before it did anything.
    expect(run.items.slice(-3).some((item) => item.midTurn === true)).toBe(false);
    expect(run.waiting).toEqual([]);
  });

  it("shows a message once, however often its read arrives", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    const id = await sendMessage(run, "Once only.");
    turn(relay, "session_turn_started", "t1");
    read(relay, { id, content: "Once only.", turnId: "t1" });
    read(relay, { id, content: "Once only.", turnId: "t1" });
    expect(lines(run).filter((line) => line === "Once only.")).toHaveLength(1);
  });

  it("takes a message the reloaded transcript holds as read, and keeps one it doesn't waiting", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    turn(relay, "session_turn_started", "t1");
    const readId = await sendMessage(run, "Check the logs.");
    await sendMessage(run, "And the alerts.");
    expect(run.waiting.map((message) => message.content)).toEqual([
      "Check the logs.",
      "And the alerts.",
    ]);
    // The session read the first while the relay lagged, so its read never came.
    transcripts["run-1"]?.messages.push({
      role: "user",
      content:
        "[Message from the owner via the web UI — your response in this turn is shown to them directly]\nCheck the logs.",
      timestamp: "2026-09-23T12:00:00Z",
      visibility: "user",
      id: readId,
    });
    relay.emit({ type: "subscribed", kind: "session", agent: "atlas", address: ADDRESS });
    await waitFor(() => {
      expect(run.waiting.map((message) => message.content)).toEqual(["And the alerts."]);
    });
    await waitFor(() => {
      expect(requests.filter((r) => r.endsWith("/transcript"))).toHaveLength(2);
    });
    expect(lines(run).filter((line) => line === "Check the logs.")).toHaveLength(1);

    // Its read arriving late adds nothing.
    read(relay, { id: readId, content: "Check the logs.", turnId: "t1" });
    expect(lines(run).filter((line) => line === "Check the logs.")).toHaveLength(1);
  });

  it("places a read that raced the transcript once, from the transcript", async () => {
    const { run, relay } = await opened({ address: ADDRESS });
    const id = await sendMessage(run, "Raced.");
    let release = (): void => {};
    holdTranscripts = new Promise((resolve) => {
      release = resolve;
    });
    transcripts["run-1"]?.messages.push({
      role: "user",
      content: "Raced.",
      timestamp: "2026-09-23T12:00:00Z",
      visibility: "user",
      id,
    });
    relay.emit({ type: "subscribed", kind: "session", agent: "atlas", address: ADDRESS });
    await waitFor(() => {
      expect(requests.filter((r) => r.endsWith("/transcript"))).toHaveLength(2);
    });
    turn(relay, "session_turn_started", "t1");
    read(relay, { id, content: "Raced.", turnId: "t1" });
    release();
    await waitFor(() => {
      expect(run.waiting).toEqual([]);
    });
    expect(lines(run).filter((line) => line === "Raced.")).toHaveLength(1);
  });
});

describe("the run's commands", () => {
  it("sends the draft as the owner under its own id, and says where it landed", async () => {
    const { run } = await opened({ address: ADDRESS });
    run.draft = "  Weigh safety over speed.  ";
    await run.send();
    expect(requests).toContain(`POST /api/agents/atlas/sessions/${ADDRESS}/messages`);
    expect(sentBodies).toEqual([{ content: "Weigh safety over speed.", id: run.waiting[0]?.id }]);
    expect(run.draft).toBe("");
    expect(run.items.at(-1)).toMatchObject({ kind: "status", tone: "info", content: "Delivered." });
  });

  it("keeps the draft and says why when the message couldn't be sent, and nothing waits", async () => {
    messageAnswer = () =>
      json({ error: "The session is busy. Try again shortly.", code: "busy" }, 409);
    const { run } = await opened({ address: ADDRESS });
    run.draft = "Hurry up";
    await run.send();
    expect(run.draft).toBe("Hurry up");
    expect(run.waiting).toEqual([]);
    expect(run.items.at(-1)).toMatchObject({
      kind: "status",
      tone: "error",
      content: "Couldn't send it. The session is busy. Try again shortly.",
    });
  });

  it("follows a finished session into the run its message started", async () => {
    transcripts["run-1"] = { session: summary("run-1", { state: "completed" }), messages: [] };
    messageAnswer = () => json({ outcome: "resumed", id: sentBodies.at(-1)?.id });
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
    // The new run reads it as its first turn begins.
    const id = sentBodies[0]?.id ?? "";
    expect(run.waiting.map((message) => message.id)).toEqual([id]);
    turn(relay, "session_turn_started", "run-2-t1", "run-2");
    read(relay, { id, content: "Pick this back up", turnId: "run-2-t1", runId: "run-2" });
    expect(run.waiting).toEqual([]);
    expect(lines(run).at(-1)).toBe("Pick this back up");
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
    messageAnswer = () => json({ outcome: "resumed", id: sentBodies.at(-1)?.id });
    let release = (): void => {};
    holdTranscripts = new Promise((resolve) => {
      release = resolve;
    });
    const relay = new FakeRelay();
    const run = new SessionRun("atlas", "run-1", { summary: finished }, relay);
    run.open();
    run.draft = "Again";
    const sending = run.send();
    // The new run's transcript holds the message, under the id it went out with.
    const again = transcripts["run-2"].messages[0];
    if (again) again.id = run.waiting[0]?.id;
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
    await waitFor(() => {
      expect(run.loaded).toBe(true);
    });
    expect(run.runId).toBe("run-2");
    expect(lines(run)).toEqual([
      "Again",
      "Picking this back up.",
      "This session had finished, so your message started a new run.",
    ]);
    expect(run.waiting).toEqual([]);
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
