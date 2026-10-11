import type {
  ClientMessage,
  ImageAttachment,
  MessageSender,
  ServerMessage,
} from "../src/lib/generated/protocol";
import type { ChatHistorySegment, RecentMessage } from "../src/lib/types";
import {
  deltaFrames,
  externalTurn,
  PIECE_MS,
  scenarioFor,
  type Scenario,
  type ScenarioStep,
} from "./chat-scenarios";
import {
  cannedResponses,
  markdownShowcase,
  sampleEpisodes,
  sampleRecentMessages,
} from "./data/chat";
import { json } from "./http";
import type { Route } from "./routes";
import type { MockAgent, MockHub, MockState } from "./state";

/**
 * What one simulated turn adds to the conversation's totals: two model calls
 * and three tools, with the context growing by the turn's messages.
 */
const TURN_USAGE = { input: 37_000, output: 420, tools: 3, context: 17_900, growth: 500 } as const;

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
    handler: ({ res, state }) => {
      json(res, 200, state.usage);
    },
  },
];

type SendMessage = Extract<ClientMessage, { type: "send_message" }>;

/** The files a chat turn reads, after searching memory. */
const READ_PATHS = ["team/wiki/index.md", "team/wiki/projects/residuum.md"] as const;

/** What a chat turn's first model call thinks before it searches memory. */
const PLAIN_THOUGHT =
  "The message is about the wiki, so I should search my memory for recent notes before I read any pages.";

/**
 * How long a turn runs. One that loses its connection either ends while the
 * page is away, or is still running when the page is back.
 */
function turnLengthMs(drop: boolean, finishWhileDown: boolean): number {
  if (!drop) return 1500;
  return finishWhileDown ? 900 : 4000;
}

/** Where a turn's message came from: this page's own socket, or another channel. */
export interface MessageSource {
  /** The endpoint the message arrived on: `ws`, `telegram`, ... */
  endpoint: string;
  /** The person behind it, for a channel that identifies one. */
  sender?: MessageSender;
  /** Images sent with it. */
  images?: ImageAttachment[];
}

/** A main-agent turn that has started and not yet ended. */
interface TurnInFlight {
  content: string;
  source: MessageSource;
  cancels: Array<() => void>;
}

/** Runs simulated main-agent turns for one agent. */
export interface ChatSimulator {
  /** Start a turn for a message this page sent. */
  send: (msg: SendMessage) => void;
  /** Start a turn for a message another channel brought, such as Telegram. */
  receive: (content: string, source: MessageSource) => void;
  /** Stop the turn answering `replyTo`, if it is still running. */
  cancel: (replyTo: string) => void;
}

/**
 * Simulate main-agent turns: live frames to every connected page, then the
 * whole turn recorded in history when it ends, as the real gateway does. A
 * message first reaches every page as a `user_message` frame, then the turn
 * starts. The agent thinks and writes in pieces, the way a model streams,
 * and each piece is followed by the complete message.
 *
 * A message starting with "drop" loses the connection mid-turn:
 * - "drop finish": the turn ends while disconnected; history has it on reconnect.
 * - "drop compress": as "drop", and the observer compresses history into a
 *   new episode meanwhile, so the page has to reload history.
 * - "drop" (anything else): the turn is still running at reconnect and
 *   finishes live afterwards.
 *
 * Each turn reports its usage, and adds it to the conversation's totals. A
 * message starting with "remember" is followed by two seconds of memory work.
 * Other prefixes run the scripted turns of `chat-scenarios.ts`.
 * A message starting with "markdown" is answered with `markdownShowcase`
 * instead of the next canned reply, which keeps its place.
 */
export function createChatSimulator(hub: MockHub, agent: MockAgent): ChatSimulator {
  const { state } = agent;
  const { env } = hub;
  const inFlight = new Map<string, TurnInFlight>();
  let responseIndex = 0;
  const WEB: MessageSource = { endpoint: "ws" };

  /** The user's message as history records it. */
  function userRecord(content: string, source: MessageSource): RecentMessage {
    return {
      role: "user",
      content,
      timestamp: env.clock.iso(),
      visibility: "user",
      ...(source.sender === undefined ? {} : { sender: source.sender }),
    };
  }

  function recordUserMessage(content: string, source: MessageSource): void {
    state.extraRecent.push(userRecord(content, source));
  }

  /** Add a turn's model calls and tools to the conversation's totals. */
  function addUsage(tools: number): void {
    const { usage } = state;
    state.usage = {
      input_tokens: usage.input_tokens + TURN_USAGE.input,
      output_tokens: usage.output_tokens + TURN_USAGE.output,
      context_tokens: (usage.context_tokens ?? TURN_USAGE.context) + TURN_USAGE.growth,
      tool_calls: usage.tool_calls + tools,
    };
  }

  /** The frames that start a turn: the message every page is told of, then the turn. */
  function startFrames(replyTo: string, content: string, source: MessageSource): ServerMessage[] {
    return [
      {
        type: "user_message",
        id: replyTo,
        turn_id: replyTo,
        content,
        ...(source.images === undefined || source.images.length === 0
          ? {}
          : { images: source.images }),
        ...(source.sender === undefined ? {} : { sender: source.sender }),
        endpoint: source.endpoint,
      },
      {
        type: "turn_started",
        reply_to: replyTo,
        origin: {
          endpoint: source.endpoint,
          ...(source.sender === undefined ? {} : { sender: source.sender }),
          visibility: "user",
        },
      },
    ];
  }

  /** A turn is over: the agent is no longer busy, and a reply the user hasn't seen counts as unread. */
  function settleTurn(failed: boolean): void {
    hub.setBusy(agent, false);
    if (failed) return;
    hub.teamEvents.agentReplied(agent);
    if (agent.connectedClients() === 0) hub.addUnread(agent);
  }

  /** Run a scripted turn: its frames at their times, then its ending, held where a test holds turns. */
  function runScenario(
    replyTo: string,
    content: string,
    scenario: Scenario,
    source: MessageSource,
  ): void {
    const turn: TurnInFlight = { content, source, cancels: [] };
    inFlight.set(replyTo, turn);
    const later = (ms: number, action: () => void): void => {
      turn.cancels.push(env.after(ms, action));
    };
    const emit = (frames: ServerMessage[]): void => {
      for (const frame of frames) state.broadcast(frame);
    };
    const schedule = (steps: ScenarioStep[]): void => {
      for (const step of steps) {
        const onlyResults = step.frames.every((frame) => frame.type === "tool_result");
        later(step.at, () => {
          if (!onlyResults) {
            emit(step.frames);
            return;
          }
          turn.cancels.push(
            env.whenTurnReleased("results", () => {
              emit(step.frames);
            }),
          );
        });
      }
    };

    emit(startFrames(replyTo, content, source));
    hub.setBusy(agent, true);
    schedule(scenario.steps);
    later(scenario.endAt, () => {
      turn.cancels.push(
        env.whenTurnReleased("end", () => {
          // What the turn's last model call writes, timed from the moment it is let end.
          for (const step of scenario.finale) {
            later(step.at, () => {
              emit(step.frames);
            });
          }
          later(scenario.finaleMs, () => {
            inFlight.delete(replyTo);
            addUsage(scenario.toolCalls);
            emit([
              {
                type: "turn_usage",
                reply_to: replyTo,
                output_tokens: TURN_USAGE.output,
                has_usage: true,
                tool_calls: scenario.toolCalls,
                session_totals: state.usage,
              },
            ]);
            if (scenario.failure !== null) {
              emit([
                {
                  type: "error",
                  reply_to: replyTo,
                  message: scenario.failure.message,
                  details: scenario.failure.details,
                },
              ]);
            } else if (scenario.reply !== null) {
              emit([
                {
                  type: "response",
                  reply_to: replyTo,
                  call: scenario.replyCall,
                  endpoint: source.endpoint,
                  content: scenario.reply,
                },
              ]);
            }
            emit([{ type: "turn_ended", reply_to: replyTo }]);
            settleTurn(scenario.failure !== null);
            if (scenario.recorded.length === 0) {
              recordUserMessage(content, source);
            } else {
              const ofTurn = {
                timestamp: env.clock.iso(),
                visibility: "user",
                turn_id: replyTo,
              } as const;
              state.extraRecent.push(
                { ...userRecord(content, source), turn_id: replyTo },
                ...scenario.recorded.map((message) => ({ ...message, ...ofTurn })),
              );
            }
            hub.overview.changed(agent);
          });
        }),
      );
    });
  }

  function start(replyTo: string, content: string, source: MessageSource): void {
    const scenario =
      source.endpoint === "ws"
        ? scenarioFor(content, replyTo, env.nextId)
        : externalTurn(replyTo, env.nextId);
    if (scenario !== null) {
      runScenario(replyTo, content, scenario, source);
      return;
    }
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
    // Then two files read at once; a message starting with "fail" can't read the second.
    const reads = READ_PATHS.map((path, i) => ({
      id: `tc_mock_${String(env.nextId())}`,
      path,
      output:
        i === 1 && lower.startsWith("fail")
          ? `file not found: ${path}`
          : `   1\t# ${path}\n   2\t(the page as it is today)`,
      isError: i === 1 && lower.startsWith("fail"),
    }));
    const showcase = lower.startsWith("markdown");
    const response = showcase
      ? markdownShowcase
      : (cannedResponses[responseIndex % cannedResponses.length] ?? "");
    if (!showcase) responseIndex++;

    const turn: TurnInFlight = { content, source, cancels: [] };
    inFlight.set(replyTo, turn);
    const later = (ms: number, action: () => void): void => {
      turn.cancels.push(env.after(ms, action));
    };

    // Live frames stop while the connection is down; the agent's turn journal still keeps them.
    let down = false;
    const live = (frame: ServerMessage): void => {
      if (down) state.journalOnly(frame);
      else state.broadcast(frame);
    };

    for (const frame of startFrames(replyTo, content, source)) live(frame);
    hub.setBusy(agent, true);
    // The first model call thinks, then writes its note in pieces, ahead of the complete messages.
    const note = "Looking through recent notes first.";
    deltaFrames(replyTo, "thinking", 0, PLAIN_THOUGHT).forEach((frame, i) => {
      later(20 + i * PIECE_MS, () => {
        live(frame);
      });
    });
    later(200, () => {
      live({ type: "thinking", reply_to: replyTo, call: 0, content: PLAIN_THOUGHT });
    });
    deltaFrames(replyTo, "text", 0, note).forEach((frame, i) => {
      later(215 + i * PIECE_MS, () => {
        live(frame);
      });
    });
    later(300, () => {
      live({ type: "broadcast_response", reply_to: replyTo, call: 0, content: note });
      live({
        type: "tool_call",
        reply_to: replyTo,
        call: 0,
        id: toolCallId,
        name: "memory_search",
        arguments: toolArgs,
        server: null,
      });
    });
    later(600, () => {
      live({
        type: "tool_result",
        reply_to: replyTo,
        tool_call_id: toolCallId,
        name: "memory_search",
        output: toolOutput,
        is_error: false,
      });
      live({
        type: "turn_usage",
        reply_to: replyTo,
        output_tokens: TURN_USAGE.output / 2,
        has_usage: true,
        tool_calls: 1,
        session_totals: null,
      });
      for (const read of reads) {
        live({
          type: "tool_call",
          reply_to: replyTo,
          call: 1,
          id: read.id,
          name: "read_file",
          arguments: { path: read.path },
          server: null,
        });
      }
    });
    reads.forEach((read, i) => {
      later(900 + i * 300, () => {
        turn.cancels.push(
          env.whenTurnReleased("results", () => {
            live({
              type: "tool_result",
              reply_to: replyTo,
              tool_call_id: read.id,
              name: "read_file",
              output: read.output,
              is_error: read.isError,
            });
          }),
        );
      });
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
      turn.cancels.push(env.whenTurnReleased("end", endTurn));
    });

    /** The last model call writes its reply in pieces, unless no page is listening, then the turn ends. */
    function endTurn(): void {
      if (down) {
        finishTurn();
        return;
      }
      const written = deltaFrames(replyTo, "text", 2, response);
      written.forEach((frame, i) => {
        later(i * PIECE_MS, () => {
          live(frame);
        });
      });
      later(written.length * PIECE_MS + 40, () => {
        turn.cancels.push(env.whenTurnReleased("finish", finishTurn));
      });
    }

    function finishTurn(): void {
      inFlight.delete(replyTo);
      addUsage(TURN_USAGE.tools);
      live({
        type: "turn_usage",
        reply_to: replyTo,
        output_tokens: TURN_USAGE.output,
        has_usage: true,
        tool_calls: TURN_USAGE.tools,
        session_totals: state.usage,
      });
      live({
        type: "response",
        reply_to: replyTo,
        call: 2,
        endpoint: source.endpoint,
        content: response,
      });
      live({ type: "turn_ended", reply_to: replyTo });
      if (lower.startsWith("remember")) {
        live({ type: "post_turn_activity", kind: "memory", active: true });
        later(2000, () => {
          live({ type: "post_turn_activity", kind: "memory", active: false });
        });
      }
      settleTurn(false);
      const now = env.clock.iso();
      // Like the backend, every message of the turn carries its correlation id.
      const ofTurn = { timestamp: now, visibility: "user", turn_id: replyTo } as const;
      state.extraRecent.push(
        { ...userRecord(content, source), turn_id: replyTo },
        {
          role: "assistant",
          content: note,
          thinking: [PLAIN_THOUGHT],
          tool_calls: [
            { id: toolCallId, name: "memory_search", arguments: toolArgs, server: null },
          ],
          ...ofTurn,
        },
        { role: "tool", content: toolOutput, tool_call_id: toolCallId, ...ofTurn },
        {
          role: "assistant",
          content: "",
          tool_calls: reads.map((r) => ({
            id: r.id,
            name: "read_file",
            arguments: { path: r.path },
            server: null,
          })),
          ...ofTurn,
        },
        ...reads.map(
          (r) => ({ role: "tool", content: r.output, tool_call_id: r.id, ...ofTurn }) as const,
        ),
        { role: "assistant", content: response, ...ofTurn },
      );
      hub.overview.changed(agent);
    }
  }

  function send(msg: SendMessage): void {
    start(msg.id, msg.content, {
      ...WEB,
      ...(msg.images === undefined ? {} : { images: msg.images }),
    });
  }

  function receive(content: string, source: MessageSource): void {
    start(`${source.endpoint}-${String(env.nextId())}`, content, source);
  }

  function cancel(replyTo: string): void {
    const turn = inFlight.get(replyTo);
    if (turn === undefined) return;
    inFlight.delete(replyTo);
    for (const stop of turn.cancels) stop();
    state.broadcast({ type: "turn_ended", reply_to: replyTo });
    hub.setBusy(agent, false);
    recordUserMessage(turn.content, turn.source);
    hub.overview.changed(agent);
  }

  return { send, receive, cancel };
}
