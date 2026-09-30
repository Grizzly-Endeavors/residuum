import type {
  ServerMessage,
  SessionCommandErrorCode,
  SessionDeliveryOutcome,
  SessionListResponse,
  SessionRunStatus,
  SessionState,
  SessionSummary,
} from "../src/lib/generated/protocol";
import type { RecentMessage, SessionTranscriptResponse } from "../src/lib/types";
import { untrackedRunFields } from "./data/sessions";
import { artifactIdentity, json, parseJsonObject, readBody, stringField, text } from "./http";
import { decodedParam, type Route, type RouteContext } from "./routes";
import type { MockState } from "./state";

/**
 * Transcript fetches are slowed down so the "Loading transcript…" state (and
 * anything racing it, like messaging a session straight after opening it)
 * can be exercised by hand.
 */
const TRANSCRIPT_DELAY_MS = 700;

const OWNER_MESSAGE_LABEL =
  "[Message from the owner via the web UI — your response in this turn is shown to them directly]";

/** Where a session command's reply goes: the socket that sent the command. */
export type SessionReply = (frame: ServerMessage) => void;

type SessionMessage = Pick<RecentMessage, "role" | "content"> & Partial<RecentMessage>;

// Session lifecycle helpers shared by the WebSocket command handlers and the
// REST endpoints that start, stop, and message sessions on an artifact's
// behalf (`POST /api/sessions` and friends) — both need to mutate the same
// in-memory sessions and announce it over the same broadcast channel.

function recordMessage(state: MockState, session: SessionSummary, message: SessionMessage): void {
  const { transcripts } = state.sessions;
  const list = transcripts.get(session.run_id) ?? [];
  list.push({ timestamp: session.started_at, visibility: "user", ...message });
  transcripts.set(session.run_id, list);
}

function setSessionState(state: MockState, session: SessionSummary, next: SessionState): void {
  session.state = next;
  state.broadcast({
    type: "session_state_changed",
    address: session.address,
    run_id: session.run_id,
    state: next,
  });
}

function completeSession(
  state: MockState,
  session: SessionSummary,
  status: SessionRunStatus,
  error: string | null,
  errorDetails: string | null = null,
): void {
  const { sessions } = state;
  setSessionState(state, session, "completing");
  state.env.after(800, () => {
    sessions.live = sessions.live.filter((s) => s.run_id !== session.run_id);
    session.state = "completed";
    session.completed_at = state.env.clock.iso();
    session.episode_id = status === "completed" ? "ep-301" : null;
    sessions.completed.unshift(session);
    state.broadcast({
      type: "session_completed",
      address: session.address,
      run_id: session.run_id,
      status,
      error,
      error_details: errorDetails,
      episode_id: session.episode_id,
    });
  });
}

// One turn: running → tool → reply → idle, relaying to main when spawned by it.
function runSessionTurn(state: MockState, session: SessionSummary, reply: string): void {
  const { sessions } = state;
  const turnId = `${session.run_id}-t${String(state.env.nextId())}`;
  const toolId = `tc_s_${String(state.env.nextId())}`;
  setSessionState(state, session, "running");
  state.broadcast({
    type: "session_turn_started",
    address: session.address,
    run_id: session.run_id,
    turn_id: turnId,
  });
  state.env.after(500, () => {
    state.broadcast({
      type: "session_broadcast_response",
      address: session.address,
      run_id: session.run_id,
      content: "Checking the notes first.",
    });
    state.broadcast({
      type: "session_tool_call",
      address: session.address,
      run_id: session.run_id,
      id: toolId,
      name: "memory_search",
      arguments: { query: "fallback" },
    });
  });
  state.env.after(1200, () => {
    state.broadcast({
      type: "session_tool_result",
      address: session.address,
      run_id: session.run_id,
      tool_call_id: toolId,
      name: "memory_search",
      output: "1 result: notification-routing.md",
      is_error: false,
    });
  });
  state.env.after(2400, () => {
    if (!sessions.live.includes(session) || session.state !== "running") return;
    recordMessage(state, session, { role: "assistant", content: reply });
    state.broadcast({
      type: "session_response",
      address: session.address,
      run_id: session.run_id,
      turn_id: turnId,
      content: reply,
    });
    state.broadcast({
      type: "session_turn_ended",
      address: session.address,
      run_id: session.run_id,
      turn_id: turnId,
    });
    setSessionState(state, session, "idle");
    if (session.spawner === "main") {
      state.broadcast({
        type: "session_message_to_main",
        address: session.address,
        run_id: session.run_id,
        content: reply,
      });
    }
  });
}

/** Start a subagent session on the main agent's behalf, and let it run one turn. */
export function spawnSession(state: MockState, purpose: string): SessionSummary {
  const { sessions } = state;
  sessions.runCounter++;
  const session: SessionSummary = {
    address: `spawned-subagent-${(0xa000 + sessions.runCounter).toString(16)}`,
    run_id: `run-new-${sessions.runCounter}`,
    category: "spawned",
    source_label: "agent:subagent",
    state: "forking",
    spawner: "main",
    depth: 1,
    purpose,
    started_at: state.env.clock.iso(),
    completed_at: null,
    episode_id: null,
    interrupted: false,
    ...untrackedRunFields(),
  };
  sessions.live.unshift(session);
  recordMessage(state, session, { role: "user", content: purpose });
  state.broadcast({ type: "session_started", session });
  state.env.after(400, () => {
    runSessionTurn(
      state,
      session,
      `Finished: ${purpose}. Two items need a look; details are in the transcript.`,
    );
  });
  return session;
}

/** Bring a finished session back as a new run that answers `message` with `reply`. */
function resumeRun(
  state: MockState,
  prev: SessionSummary,
  options: { runIdPrefix: string; message: string; reply: string },
): SessionSummary {
  const { sessions } = state;
  sessions.runCounter++;
  const session: SessionSummary = {
    ...prev,
    run_id: `${options.runIdPrefix}-${sessions.runCounter}`,
    state: "forking",
    started_at: state.env.clock.iso(),
    completed_at: null,
    episode_id: null,
    interrupted: false,
    ...untrackedRunFields(),
  };
  sessions.live.unshift(session);
  recordMessage(state, session, { role: "user", content: options.message });
  state.broadcast({ type: "session_started", session });
  state.env.after(400, () => {
    runSessionTurn(state, session, options.reply);
  });
  return session;
}

/** The `session_send_message` socket command: message a live session, or resume a finished one. */
export function sendSessionMessage(
  state: MockState,
  reply: SessionReply,
  id: string,
  address: string,
  content: string,
): void {
  const { sessions } = state;
  const live = sessions.live.find((s) => s.address === address);
  if (content.includes("busy")) {
    reply({
      type: "session_command_failed",
      id,
      address,
      code: "busy",
      message: `${address} is busy and can't take another message yet. Try again shortly.`,
    });
    return;
  }
  if (live) {
    recordMessage(state, live, {
      role: "user",
      content: `${OWNER_MESSAGE_LABEL}\n${content}`,
    });
    reply({ type: "session_message_delivered", id, address, outcome: "live" });
    runSessionTurn(state, live, `Understood: "${content.slice(0, 60)}". Adjusting course.`);
    return;
  }
  const prev = sessions.completed.find((s) => s.address === address);
  if (!prev) {
    reply({
      type: "session_command_failed",
      id,
      address,
      code: "unknown_address",
      message: `There's no session called ${address}. It may have been from before a restart.`,
    });
    return;
  }
  reply({ type: "session_message_delivered", id, address, outcome: "resumed" });
  state.env.after(300, () => {
    resumeRun(state, prev, {
      runIdPrefix: "run-resumed",
      message: `${OWNER_MESSAGE_LABEL}\n${content}`,
      reply: "Picking this back up. Here's where it stands now.",
    });
  });
}

/** The `session_stop` socket command. */
export function stopSession(
  state: MockState,
  reply: SessionReply,
  id: string,
  address: string,
): void {
  const live = state.sessions.live.find((s) => s.address === address);
  if (!live || live.state === "completing") {
    reply({
      type: "session_command_failed",
      id,
      address,
      code: "not_live",
      message: `${address} isn't running, so there's nothing to stop.`,
    });
    return;
  }
  reply({ type: "session_stop_requested", id, address });
  completeSession(state, live, "cancelled", null);
}

// ─── REST ──────────────────────────────────────────────────────────────────────

/** An error body of the session endpoints: a message and the code clients branch on. */
function sessionError(
  ctx: RouteContext,
  status: number,
  error: string,
  code: SessionCommandErrorCode,
): void {
  json(ctx.res, status, { error, code });
}

function listSessions({ res, state, query }: RouteContext): void {
  const address = query.get("address");
  const category = query.get("category");
  const artifact = query.get("artifact");
  const limit = Number(query.get("limit") ?? "50");
  const before = query.get("before");
  const matches = (s: SessionSummary): boolean =>
    (!address || s.address === address) &&
    (!category || s.category === category) &&
    (!artifact || (s.category === "artifact" && s.source_label === `artifact:${artifact}`));
  const done = state.sessions.completed.filter(matches);
  const startIdx = before ? done.findIndex((s) => s.run_id === before) + 1 : 0;
  const page = done.slice(startIdx, startIdx + limit);
  const hasMore = startIdx + limit < done.length;
  json(res, 200, {
    live: state.sessions.live.filter(matches),
    completed: page,
    next_cursor: hasMore ? (page[page.length - 1]?.run_id ?? null) : null,
  } satisfies SessionListResponse);
}

async function sessionTranscript(ctx: RouteContext): Promise<void> {
  const { res, state } = ctx;
  const runId = decodedParam(ctx, 0);
  const session =
    state.sessions.live.find((s) => s.run_id === runId) ??
    state.sessions.completed.find((s) => s.run_id === runId);
  if (!session) {
    text(res, 404, "no such run");
    return;
  }
  await state.env.sleep(TRANSCRIPT_DELAY_MS);
  json(res, 200, {
    session,
    messages: state.sessions.transcripts.get(runId) ?? [],
  } satisfies SessionTranscriptResponse);
}

// POST /api/sessions — start an artifact session, the endpoint
// `residuum.sessions.start` calls through the bridge.
async function startArtifactSession(ctx: RouteContext): Promise<void> {
  const { req, res, state } = ctx;
  const artifactName = artifactIdentity(req);
  if (!artifactName) {
    json(res, 400, {
      error:
        "starting a session needs the X-Residuum-Artifact header: sessions are started by workbench artifacts, through residuum.sessions.start",
    });
    return;
  }
  const body = parseJsonObject((await readBody(req)) || "{}");
  const prompt = stringField(body, "prompt") ?? "";
  if (!prompt.trim()) {
    json(res, 400, { error: "prompt must not be empty" });
    return;
  }
  const { sessions } = state;
  sessions.runCounter++;
  const session: SessionSummary = {
    address: `artifact-${artifactName}-${(0x1000 + sessions.runCounter).toString(16)}`,
    run_id: `run-artifact-${sessions.runCounter}`,
    category: "artifact",
    source_label: `artifact:${artifactName}`,
    state: "forking",
    spawner: null,
    depth: 1,
    purpose: prompt.slice(0, 140),
    started_at: state.env.clock.iso(),
    completed_at: null,
    episode_id: null,
    interrupted: false,
    ...untrackedRunFields(),
  };
  sessions.live.unshift(session);
  recordMessage(state, session, {
    role: "user",
    content: `[This session was started by the workbench artifact "${artifactName}". Your responses are shown to that artifact, not to the main conversation.]\n\n${prompt}`,
  });
  state.broadcast({ type: "session_started", session });
  state.env.after(400, () => {
    runSessionTurn(state, session, `Working on it: ${prompt.slice(0, 80)}.`);
  });
  json(res, 202, { address: session.address });
}

// POST /api/sessions/:address/stop — stop any live session.
function stopSessionRoute(ctx: RouteContext): void {
  const { res, state } = ctx;
  const address = decodedParam(ctx, 0);
  if (address === "main") {
    sessionError(ctx, 400, "main can't be stopped this way", "invalid_request");
    return;
  }
  const live = state.sessions.live.find((s) => s.address === address);
  if (!live || live.state === "completing") {
    sessionError(ctx, 404, `${address} isn't running, so there's nothing to stop.`, "not_live");
    return;
  }
  completeSession(state, live, "cancelled", null);
  json(res, 202, { address });
}

// POST /api/sessions/:address/messages — message any session, attributed
// to the artifact naming itself with the identity header, or the owner.
async function messageSessionRoute(ctx: RouteContext): Promise<void> {
  const { req, res, state } = ctx;
  const address = decodedParam(ctx, 0);
  const body = parseJsonObject((await readBody(req)) || "{}");
  const content = stringField(body, "content") ?? "";
  if (!content.trim() || address === "main") {
    sessionError(
      ctx,
      400,
      content.trim() ? "main can't be messaged this way" : "content must not be empty",
      "invalid_request",
    );
    return;
  }
  const artifactName = artifactIdentity(req);
  const label = artifactName
    ? `[Message from the workbench artifact "${artifactName}" — your response in this turn is shown to it directly]`
    : OWNER_MESSAGE_LABEL;
  const { sessions } = state;
  const live = sessions.live.find((s) => s.address === address);
  if (live) {
    recordMessage(state, live, { role: "user", content: `${label}\n${content}` });
    runSessionTurn(state, live, `Understood: "${content.slice(0, 60)}".`);
    json(res, 200, { outcome: "live" satisfies SessionDeliveryOutcome });
    return;
  }
  const prev = sessions.completed.find((s) => s.address === address);
  if (!prev) {
    sessionError(
      ctx,
      404,
      `There's no session called ${address}. It may have been from before a restart.`,
      "unknown_address",
    );
    return;
  }
  resumeRun(state, prev, {
    runIdPrefix: "run-resumed-http",
    message: `${label}\n${content}`,
    reply: "Picking this back up.",
  });
  json(res, 200, { outcome: "resumed" satisfies SessionDeliveryOutcome });
}

/** The sessions endpoints, in the unscoped `/api/...` spelling. */
export const sessionRoutes: readonly Route[] = [
  { method: "GET", pattern: "/api/sessions", handler: listSessions },
  {
    method: "GET",
    pattern: /^\/api\/sessions\/runs\/([^/]+)\/transcript$/,
    handler: sessionTranscript,
  },
  { method: "POST", pattern: "/api/sessions", handler: startArtifactSession },
  { method: "POST", pattern: /^\/api\/sessions\/([^/]+)\/stop$/, handler: stopSessionRoute },
  {
    method: "POST",
    pattern: /^\/api\/sessions\/([^/]+)\/messages$/,
    handler: messageSessionRoute,
  },
];
