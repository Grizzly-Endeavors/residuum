import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  ServerMessage,
  SessionListResponse,
  SessionSummary,
} from "../src/lib/generated/protocol";
import type { SessionTranscriptResponse } from "../src/lib/types";
import { sendSessionMessage, sessionRoutes, spawnSession, stopSession } from "./sessions";
import { createState, type MockState } from "./state";
import {
  captureFrames,
  fetchJson,
  fetchText,
  startRouteHarness,
  type RouteHarness,
} from "./test-support";

const ARTIFACT_HEADER = { "X-Residuum-Artifact": "tip-splitter" };
const DISCORD = "external-discord-4f1c9a2e7b3d0856";
const TELEGRAM = "external-telegram-a07d3e5519c2b4f8";
const RESEARCH = "spawned-research-3f9a";

const UNTRACKED = {
  usage: { input_tokens: 0, output_tokens: 0, context_tokens: null, tool_calls: 0 },
  outcome: null,
  error: null,
  error_details: null,
  overlap: null,
};

async function post(
  harness: RouteHarness,
  path: string,
  body: unknown,
  headers = {},
): Promise<{ status: number; body: Record<string, unknown> }> {
  const res = await fetchJson(`${harness.baseUrl}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", ...headers },
    body: JSON.stringify(body),
  });
  return { status: res.status, body: res.body as Record<string, unknown> };
}

describe("sessions endpoints", () => {
  let harness: RouteHarness;

  beforeEach(async () => {
    harness = await startRouteHarness(sessionRoutes);
  });

  afterEach(async () => {
    await harness.close();
  });

  const list = async (query = ""): Promise<{ status: number; body: SessionListResponse }> => {
    const res = await fetchJson(`${harness.baseUrl}/api/sessions${query}`);
    return { status: res.status, body: res.body as SessionListResponse };
  };

  it("lists every session with the fields of the generated summary", async () => {
    const { status, body } = await list("?limit=100");
    expect(status).toBe(200);
    const all = [...body.live, ...body.completed];
    expect(body.live).toHaveLength(3);
    expect(body.completed.length).toBeGreaterThan(30);
    expect(body.next_cursor).toBeNull();
    for (const session of all) expect(session).toMatchObject(UNTRACKED);
  });

  it("pages finished sessions by the cursor", async () => {
    const first = (await list("?limit=5")).body;
    expect(first.completed).toHaveLength(5);
    expect(first.next_cursor).toBe(first.completed[4]?.run_id);
    const second = (await list(`?limit=5&before=${first.next_cursor}`)).body;
    expect(second.completed).toHaveLength(5);
    const firstIds = new Set(first.completed.map((s) => s.run_id));
    expect(second.completed.some((s) => firstIds.has(s.run_id))).toBe(false);
  });

  it("starts from the top when the cursor names no session", async () => {
    const top = (await list("?limit=3")).body;
    const unknown = (await list("?limit=3&before=nope")).body;
    expect(unknown.completed).toEqual(top.completed);
  });

  it("filters by category, address and artifact", async () => {
    const external = (await list("?category=external&limit=100")).body;
    expect([...external.live, ...external.completed].every((s) => s.category === "external")).toBe(
      true,
    );
    expect(external.live.map((s) => s.address)).toEqual([DISCORD]);

    const byAddress = (await list(`?address=${RESEARCH}`)).body;
    expect(byAddress.live.map((s) => s.address)).toEqual([RESEARCH]);
    expect(byAddress.completed).toEqual([]);

    const artifact = (await list("?artifact=wiki-graph&limit=100")).body;
    const sources = [...artifact.live, ...artifact.completed].map((s) => s.source_label);
    expect(sources.length).toBeGreaterThan(1);
    expect(new Set(sources)).toEqual(new Set(["artifact:wiki-graph"]));
  });

  it("answers a transcript request for an unknown run with 404", async () => {
    const { status, body } = await fetchText(
      `${harness.baseUrl}/api/sessions/runs/nope/transcript`,
    );
    expect(status).toBe(404);
    expect(body).toBe("no such run");
  });

  it("serves a run's transcript with its summary, after the loading delay", async () => {
    const res = await fetchJson(`${harness.baseUrl}/api/sessions/runs/run-live-discord/transcript`);
    const body = res.body as SessionTranscriptResponse;
    expect(res.status).toBe(200);
    expect(body.session.address).toBe(DISCORD);
    expect(body.session).toMatchObject(UNTRACKED);
    expect(body.messages.map((m) => m.role)).toEqual(["user", "assistant", "user", "assistant"]);
  });

  describe("starting a session", () => {
    it("needs the artifact identity header", async () => {
      const missing = await post(harness, "/api/sessions", { prompt: "hi" });
      expect(missing.status).toBe(400);
      expect(String(missing.body.error)).toContain("X-Residuum-Artifact");
      const malformed = await post(
        harness,
        "/api/sessions",
        { prompt: "hi" },
        { "X-Residuum-Artifact": "Not A Name" },
      );
      expect(malformed.status).toBe(400);
    });

    it("rejects an empty prompt and an empty body", async () => {
      const blank = await post(harness, "/api/sessions", { prompt: "  " }, ARTIFACT_HEADER);
      expect(blank).toEqual({ status: 400, body: { error: "prompt must not be empty" } });
      const res = await fetch(`${harness.baseUrl}/api/sessions`, {
        method: "POST",
        headers: ARTIFACT_HEADER,
      });
      expect(res.status).toBe(400);
    });

    it("lists the new session first and announces it with every summary field", async () => {
      const prompt = "x".repeat(200);
      const { status, body } = await post(harness, "/api/sessions", { prompt }, ARTIFACT_HEADER);
      expect(status).toBe(202);
      expect(body).toEqual({ address: "artifact-tip-splitter-1001" });

      const live = (await list()).body.live;
      expect(live[0]?.address).toBe("artifact-tip-splitter-1001");
      expect(live[0]?.purpose).toHaveLength(140);
      expect(live[0]).toMatchObject({ category: "artifact", ...UNTRACKED });

      const started = harness.frames.find((f) => f.type === "session_started");
      expect(started).toMatchObject({ session: { run_id: "run-artifact-1", ...UNTRACKED } });
    });
  });

  describe("stopping a session", () => {
    it("refuses main and a session that is not live", async () => {
      const main = await post(harness, "/api/sessions/main/stop", {});
      expect(main.status).toBe(400);
      expect(main.body.code).toBe("invalid_request");
      const unknown = await post(harness, "/api/sessions/nope/stop", {});
      expect(unknown.status).toBe(404);
      expect(unknown.body.code).toBe("not_live");
    });

    it("accepts a live session and starts winding it down", async () => {
      const { status, body } = await post(harness, `/api/sessions/${RESEARCH}/stop`, {});
      expect(status).toBe(202);
      expect(body).toEqual({ address: RESEARCH });
      expect(harness.frames).toContainEqual({
        type: "session_state_changed",
        address: RESEARCH,
        run_id: "run-live-research",
        state: "completing",
      });
      const again = await post(harness, `/api/sessions/${RESEARCH}/stop`, {});
      expect(again.status).toBe(404);
    });
  });

  describe("messaging a session", () => {
    it("refuses empty content, main and an unknown address", async () => {
      const empty = await post(harness, `/api/sessions/${DISCORD}/messages`, { content: " " });
      expect(empty.body).toEqual({ error: "content must not be empty", code: "invalid_request" });
      const main = await post(harness, "/api/sessions/main/messages", { content: "hi" });
      expect(main.body).toEqual({
        error: "main can't be messaged this way",
        code: "invalid_request",
      });
      const unknown = await post(harness, "/api/sessions/nope/messages", { content: "hi" });
      expect(unknown.status).toBe(404);
      expect(unknown.body.code).toBe("unknown_address");
    });

    it("delivers to a live session, attributed to the artifact or the owner", async () => {
      const fromArtifact = await post(
        harness,
        `/api/sessions/${DISCORD}/messages`,
        { content: "status?" },
        ARTIFACT_HEADER,
      );
      expect(fromArtifact).toEqual({ status: 200, body: { outcome: "live" } });
      await post(harness, `/api/sessions/${DISCORD}/messages`, { content: "and now?" });

      const transcript = harness.state.sessions.transcripts.get("run-live-discord") ?? [];
      const last = transcript.slice(-2).map((m) => m.content);
      expect(last[0]).toMatch(/^\[Message from the workbench artifact "tip-splitter"/);
      expect(last[1]).toMatch(/^\[Message from the owner via the web UI/);
    });

    it("resumes a finished session as a new run", async () => {
      const { status, body } = await post(harness, `/api/sessions/${TELEGRAM}/messages`, {
        content: "hello again",
      });
      expect(status).toBe(200);
      expect(body).toEqual({ outcome: "resumed" });
      const live = (await list()).body.live;
      expect(live[0]).toMatchObject({
        address: TELEGRAM,
        run_id: "run-resumed-http-1",
        ...UNTRACKED,
      });
      expect(harness.frames.map((f) => f.type)).toContain("session_started");
    });
  });
});

describe("session commands and lifecycle", () => {
  let state: MockState;
  let frames: ServerMessage[];
  let replies: ServerMessage[];
  const reply = (frame: ServerMessage): void => {
    replies.push(frame);
  };
  const types = (list: ServerMessage[]): string[] => list.map((f) => f.type);
  const summaryOf = (runId: string): SessionSummary | undefined =>
    state.sessions.live.find((s) => s.run_id === runId) ??
    state.sessions.completed.find((s) => s.run_id === runId);

  beforeEach(() => {
    vi.useFakeTimers();
    state = createState("atlas");
    frames = captureFrames(state);
    replies = [];
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("refuses a message that contains 'busy' without touching the session", () => {
    sendSessionMessage(state, reply, "c1", DISCORD, "busy please");
    expect(replies).toEqual([
      {
        type: "session_command_failed",
        id: "c1",
        address: DISCORD,
        code: "busy",
        message: `${DISCORD} is busy and can't take another message yet. Try again shortly.`,
      },
    ]);
    expect(frames).toEqual([]);
  });

  it("delivers to a live session and runs a turn: running, tool call, reply, idle", () => {
    sendSessionMessage(state, reply, "c2", DISCORD, "status?");
    expect(replies).toEqual([
      { type: "session_message_delivered", id: "c2", address: DISCORD, outcome: "live" },
    ]);
    expect(types(frames)).toEqual(["session_state_changed", "session_turn_started"]);

    vi.advanceTimersByTime(2400);
    expect(types(frames)).toEqual([
      "session_state_changed",
      "session_turn_started",
      "session_broadcast_response",
      "session_tool_call",
      "session_tool_result",
      "session_response",
      "session_turn_ended",
      "session_state_changed",
    ]);
    expect(summaryOf("run-live-discord")?.state).toBe("idle");
    const reply2 = state.sessions.transcripts.get("run-live-discord")?.at(-1);
    expect(reply2?.content).toBe('Understood: "status?". Adjusting course.');
  });

  it("relays a spawned session's reply to main", () => {
    sendSessionMessage(state, reply, "c3", RESEARCH, "keep going");
    vi.advanceTimersByTime(2400);
    expect(frames.at(-1)).toMatchObject({ type: "session_message_to_main", address: RESEARCH });
  });

  it("rejects an address it has never seen", () => {
    sendSessionMessage(state, reply, "c4", "nope", "hi");
    expect(replies[0]).toMatchObject({ type: "session_command_failed", code: "unknown_address" });
  });

  it("resumes a finished session after a short wait, as a new forking run", () => {
    sendSessionMessage(state, reply, "c5", TELEGRAM, "resume me");
    expect(replies).toEqual([
      { type: "session_message_delivered", id: "c5", address: TELEGRAM, outcome: "resumed" },
    ]);
    expect(frames).toEqual([]);

    vi.advanceTimersByTime(300);
    expect(frames[0]).toMatchObject({
      type: "session_started",
      session: { address: TELEGRAM, run_id: "run-resumed-1", ...UNTRACKED },
    });
    expect(summaryOf("run-resumed-1")?.state).toBe("forking");
    vi.advanceTimersByTime(400);
    expect(summaryOf("run-resumed-1")?.state).toBe("running");
  });

  it("stops a live session, then moves it to the finished list", () => {
    stopSession(state, reply, "c6", DISCORD);
    expect(replies).toEqual([{ type: "session_stop_requested", id: "c6", address: DISCORD }]);
    expect(summaryOf("run-live-discord")?.state).toBe("completing");

    vi.advanceTimersByTime(800);
    expect(state.sessions.live.map((s) => s.address)).not.toContain(DISCORD);
    expect(state.sessions.completed[0]).toMatchObject({
      address: DISCORD,
      state: "completed",
      episode_id: null,
    });
    expect(frames.at(-1)).toMatchObject({
      type: "session_completed",
      run_id: "run-live-discord",
      status: "cancelled",
      error: null,
      error_details: null,
      episode_id: null,
    });
  });

  it("refuses to stop a session that is winding down, finished, or unknown", () => {
    stopSession(state, reply, "c7", DISCORD);
    stopSession(state, reply, "c8", DISCORD);
    vi.advanceTimersByTime(800);
    stopSession(state, reply, "c9", DISCORD);
    stopSession(state, reply, "c10", "nope");
    const failures = replies.filter((f) => f.type === "session_command_failed");
    expect(failures.map((f) => ("code" in f ? f.code : null))).toEqual([
      "not_live",
      "not_live",
      "not_live",
    ]);
  });

  it("drops the reply of a turn whose session was stopped mid-turn", () => {
    sendSessionMessage(state, reply, "c11", DISCORD, "status?");
    vi.advanceTimersByTime(1000);
    stopSession(state, reply, "c12", DISCORD);
    vi.advanceTimersByTime(5000);
    expect(types(frames)).not.toContain("session_response");
    expect(types(frames)).not.toContain("session_turn_ended");
  });

  it("spawns a subagent session that reports back to main", () => {
    const session = spawnSession(state, "Look into otters");
    expect(session).toMatchObject({
      address: "spawned-subagent-a001",
      run_id: "run-new-1",
      category: "spawned",
      spawner: "main",
      purpose: "Look into otters",
      ...UNTRACKED,
    });
    expect(state.sessions.live[0]).toBe(session);
    expect(frames[0]).toMatchObject({ type: "session_started" });

    vi.advanceTimersByTime(400 + 2400);
    expect(frames.at(-1)).toEqual({
      type: "session_message_to_main",
      address: "spawned-subagent-a001",
      run_id: "run-new-1",
      content: "Finished: Look into otters. Two items need a look; details are in the transcript.",
    });
  });
});
