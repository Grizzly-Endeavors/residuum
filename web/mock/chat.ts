import type {
  ClientMessage,
  ServerMessage,
  SessionUsageTotals,
} from "../src/lib/generated/protocol";
import type { ChatHistorySegment } from "../src/lib/types";
import { cannedResponses, sampleEpisodes, sampleRecentMessages } from "./data/chat";
import { json } from "./http";
import type { Route } from "./routes";
import type { MockAgent, MockHub, MockState } from "./state";

/** What `GET /api/usage` reports: the mock counts no model calls. */
const NO_USAGE: SessionUsageTotals = {
  input_tokens: 0,
  output_tokens: 0,
  context_tokens: null,
  tool_calls: 0,
};

/**
 * One page of the main conversation, matching the backend's
 * `ChatHistorySegment` tagged union (see `gateway/web/config.rs`), or `null`
 * when `cursor` names no episode. An agent that has no conversation yet has
 * neither the sample messages nor the sample episodes.
 */
export function chatHistorySegment(
  state: MockState,
  cursor: string | null,
): ChatHistorySegment | null {
  const { clock } = state.env;
  const sample = state.hasConversation ? sampleRecentMessages(clock) : [];
  const episodes = state.hasConversation ? sampleEpisodes(clock) : [];

  if (state.compressedAt !== null) {
    if (cursor === null) {
      return {
        kind: "recent",
        messages: state.extraRecent.slice(state.compressedAt),
        next_cursor: "ep-004",
      };
    }
    if (cursor === "ep-004") {
      const compressed = [...sample, ...state.extraRecent.slice(0, state.compressedAt)];
      return {
        kind: "episode",
        episode_id: "ep-004",
        date: clock.dateDaysAgo(0),
        // Episodes don't record visibility.
        messages: compressed.map((m) => ({ ...m, visibility: "user" })),
        next_cursor: episodes[0]?.id ?? null,
      };
    }
  }

  if (cursor === null) {
    return {
      kind: "recent",
      messages: [...sample, ...state.extraRecent],
      next_cursor: episodes[0]?.id ?? null,
    };
  }

  const idx = episodes.findIndex((ep) => ep.id === cursor);
  const ep = episodes[idx];
  if (ep === undefined) {
    return null;
  }
  return {
    kind: "episode",
    episode_id: ep.id,
    date: ep.date,
    messages: ep.messages,
    next_cursor: episodes[idx + 1]?.id ?? null,
  };
}

/** The chat history and usage routes, in the unscoped `/api/...` spelling. */
export const chatRoutes: readonly Route[] = [
  {
    method: "GET",
    pattern: "/api/chat/history",
    handler: ({ res, state, query }) => {
      const segment = chatHistorySegment(state, query.get("episode"));
      if (segment === null) {
        json(res, 404, { error: "episode not found" });
        return;
      }
      json(res, 200, segment);
    },
  },
  {
    method: "GET",
    pattern: "/api/usage",
    handler: ({ res }) => {
      json(res, 200, NO_USAGE);
    },
  },
];

type SendMessage = Extract<ClientMessage, { type: "send_message" }>;

/**
 * How long a turn runs. One that loses its connection either ends while the
 * page is away, or is still running when the page is back.
 */
function turnLengthMs(drop: boolean, finishWhileDown: boolean): number {
  if (!drop) return 1500;
  return finishWhileDown ? 900 : 4000;
}

/** A main-agent turn that has started and not yet ended. */
interface TurnInFlight {
  content: string;
  cancels: Array<() => void>;
}

/** Runs simulated main-agent turns for one agent. */
export interface ChatSimulator {
  /** Start a turn for a user message. */
  send: (msg: SendMessage) => void;
  /** Stop the turn answering `replyTo`, if it is still running. */
  cancel: (replyTo: string) => void;
}

/**
 * Simulate main-agent turns: live frames to every connected page, then the
 * whole turn recorded in history when it ends, as the real gateway does.
 *
 * A message starting with "drop" loses the connection mid-turn:
 * - "drop finish": the turn ends while disconnected; history has it on reconnect.
 * - "drop compress": as "drop", and the observer compresses history into a
 *   new episode meanwhile, so the page has to reload history.
 * - "drop" (anything else): the turn is still running at reconnect and
 *   finishes live afterwards.
 */
export function createChatSimulator(hub: MockHub, agent: MockAgent): ChatSimulator {
  const { state } = agent;
  const { env } = hub;
  const inFlight = new Map<string, TurnInFlight>();
  let responseIndex = 0;

  function recordUserMessage(content: string): void {
    state.extraRecent.push({
      role: "user",
      content,
      timestamp: env.clock.iso(),
      visibility: "user",
    });
  }

  function send(msg: SendMessage): void {
    const replyTo = msg.id;
    const { content } = msg;
    const lower = content.toLowerCase();
    const drop = lower.startsWith("drop");
    const finishWhileDown = lower.startsWith("drop finish");
    const compress = lower.startsWith("drop compress");
    const toolCallId = `tc_mock_${String(env.nextId())}`;
    const toolArgs = { query: content.slice(0, 100), limit: 5 };
    const toolOutput = JSON.stringify([
      {
        text: "Found 3 relevant observations from recent conversations.",
        score: 0.87,
        timestamp: env.clock.iso(),
      },
    ]);
    const response = cannedResponses[responseIndex % cannedResponses.length] ?? "";
    responseIndex++;

    const turn: TurnInFlight = { content, cancels: [] };
    inFlight.set(replyTo, turn);
    const later = (ms: number, action: () => void): void => {
      turn.cancels.push(env.after(ms, action));
    };

    // Live frames stop while the connection is down.
    let down = false;
    const live = (frame: ServerMessage): void => {
      if (!down) state.broadcast(frame);
    };

    live({ type: "turn_started", reply_to: replyTo });
    hub.setBusy(agent, true);
    later(300, () => {
      live({ type: "broadcast_response", content: "Looking through recent notes first." });
      live({ type: "tool_call", id: toolCallId, name: "memory_search", arguments: toolArgs });
    });

    if (drop) {
      later(600, () => {
        down = true;
        if (compress) state.compressedAt = state.extraRecent.length;
        state.dropSockets();
      });
      // Reconnected by the time a still-running turn finishes.
      if (!finishWhileDown) {
        later(3500, () => {
          down = false;
        });
      }
    }

    later(turnLengthMs(drop, finishWhileDown), () => {
      inFlight.delete(replyTo);
      live({
        type: "tool_result",
        tool_call_id: toolCallId,
        name: "memory_search",
        output: toolOutput,
        is_error: false,
      });
      live({ type: "response", reply_to: replyTo, content: response });
      live({ type: "turn_ended", reply_to: replyTo });
      hub.setBusy(agent, false);
      hub.teamEvents.agentReplied(agent);
      if (agent.connectedClients() === 0) hub.addUnread(agent);
      const now = env.clock.iso();
      state.extraRecent.push(
        { role: "user", content, timestamp: now, visibility: "user" },
        {
          role: "assistant",
          content: "Looking through recent notes first.",
          tool_calls: [{ id: toolCallId, name: "memory_search", arguments: toolArgs }],
          timestamp: now,
          visibility: "user",
        },
        {
          role: "tool",
          content: toolOutput,
          tool_call_id: toolCallId,
          timestamp: now,
          visibility: "user",
        },
        { role: "assistant", content: response, timestamp: now, visibility: "user" },
      );
      hub.overview.changed(agent);
    });
  }

  function cancel(replyTo: string): void {
    const turn = inFlight.get(replyTo);
    if (turn === undefined) return;
    inFlight.delete(replyTo);
    for (const stop of turn.cancels) stop();
    state.broadcast({ type: "turn_ended", reply_to: replyTo });
    hub.setBusy(agent, false);
    recordUserMessage(turn.content);
    hub.overview.changed(agent);
  }

  return { send, cancel };
}
