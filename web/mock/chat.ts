import type {
  AgentSender,
  ClientMessage,
  ImageAttachment,
  MessageAttachment,
  MessageSender,
  ServerMessage,
} from "../src/lib/generated/protocol";
import type { ChatHistorySegment, RecentMessage } from "../src/lib/types";
import {
  deltaFrames,
  externalTurn,
  sessionMessageTurn,
  PIECE_MS,
  retriedTurn,
  scenarioFor,
  type Scenario,
  type ScenarioStep,
} from "./chat-scenarios";
import {
  cannedResponses,
  imageShowcase,
  markdownShowcase,
  sampleEpisodes,
  sampleRecentMessages,
} from "./data/chat";
import { json } from "./http";
import type { Route } from "./routes";
import type { MockAgent, MockHub, MockState } from "./state";

const TURN_USAGE = { input: 37_000, output: 420, tools: 3, context: 17_900, growth: 500 } as const;

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

const READ_PATHS = ["team/wiki/index.md", "team/wiki/projects/residuum.md"] as const;

const PLAIN_THOUGHT =
  "The message is about the wiki, so I should search my memory for recent notes before I read any pages.";

function turnLengthMs(drop: boolean, finishWhileDown: boolean): number {
  if (!drop) return 1500;
  return finishWhileDown ? 900 : 4000;
}

export const RETRY_GONE =
  "That message can't be retried anymore because the conversation has moved on. Send it again to try once more.";

export interface MessageSource {
  endpoint: string;
  sender?: MessageSender;
  images?: ImageAttachment[];
  attachments?: MessageAttachment[];
  agentSender?: AgentSender;
}

interface TurnInFlight {
  content: string;
  source: MessageSource;
  rerun: boolean;
  cancels: Array<() => void>;
  folded: RecentMessage[];
}

export interface ChatSimulator {
  send: (
    msg: SendMessage,
    attachments?: readonly MessageAttachment[],
    images?: readonly ImageAttachment[],
  ) => void;
  /** Start a turn for a message another channel brought, such as Telegram. */
  receive: (content: string, source: MessageSource) => void;
  receiveFromSession: (content: string, sender: AgentSender) => void;
  cancel: (replyTo: string) => void;
  retry: (replyTo: string) => boolean;
  sendWaitingNow: (id: string) => boolean;
  withdrawWaiting: (id: string) => boolean;
}

function showcaseFor(lower: string): string | undefined {
  if (lower.startsWith("markdown")) return markdownShowcase;
  if (lower.startsWith("images")) return imageShowcase;
  return undefined;
}

export function createChatSimulator(hub: MockHub, agent: MockAgent): ChatSimulator {
  const { state } = agent;
  const { env } = hub;
  const inFlight = new Map<string, TurnInFlight>();
  const waiting: Array<{ id: string; content: string; source: MessageSource }> = [];
  let latestFailure: { id: string; content: string; source: MessageSource } | null = null;
  let responseIndex = 0;
  const WEB: MessageSource = { endpoint: "ws" };

  function userRecord(content: string, source: MessageSource): RecentMessage {
    return {
      role: "user",
      content,
      timestamp: env.clock.iso(),
      visibility: source.agentSender === undefined ? "user" : "background",
      ...(source.images === undefined || source.images.length === 0
        ? {}
        : { images: source.images }),
      ...(source.attachments === undefined || source.attachments.length === 0
        ? {}
        : { attachments: source.attachments }),
      ...(source.sender === undefined ? {} : { sender: source.sender }),
      ...(source.agentSender === undefined ? {} : { agent_sender: source.agentSender }),
    };
  }

  function recordTurn(turn: TurnInFlight, opening: RecentMessage, own: RecentMessage[]): void {
    state.extraRecent.push(
      ...(turn.rerun ? [] : [opening]),
      ...own.slice(0, -1),
      ...turn.folded,
      ...own.slice(-1),
    );
  }

  function addUsage(tools: number): void {
    const { usage } = state;
    state.usage = {
      input_tokens: usage.input_tokens + TURN_USAGE.input,
      cache_read_tokens: usage.cache_read_tokens,
      cache_write_tokens: usage.cache_write_tokens,
      output_tokens: usage.output_tokens + TURN_USAGE.output,
      context_tokens: (usage.context_tokens ?? TURN_USAGE.context) + TURN_USAGE.growth,
      tool_calls: usage.tool_calls + tools,
    };
  }

  function startFrames(
    replyTo: string,
    content: string,
    source: MessageSource,
    rerun = false,
  ): ServerMessage[] {
    const turnStarted: ServerMessage = {
      type: "turn_started",
      reply_to: replyTo,
      origin: {
        endpoint: source.endpoint,
        ...(source.sender === undefined ? {} : { sender: source.sender }),
        visibility: source.agentSender === undefined ? "user" : "background",
      },
    };
    if (rerun) return [turnStarted];
    return [
      {
        type: "user_message",
        id: replyTo,
        turn_id: replyTo,
        content,
        ...(source.images === undefined || source.images.length === 0
          ? {}
          : { images: source.images }),
        ...(source.attachments === undefined || source.attachments.length === 0
          ? {}
          : { attachments: source.attachments }),
        ...(source.sender === undefined ? {} : { sender: source.sender }),
        ...(source.agentSender === undefined ? {} : { agent_sender: source.agentSender }),
        endpoint: source.endpoint,
      },
      turnStarted,
    ];
  }

  function settleTurn(failed: boolean): void {
    hub.setBusy(agent, false);
    if (failed) return;
    hub.teamEvents.agentReplied(agent);
    if (agent.connectedClients() === 0) hub.addUnread(agent);
  }

  function runScenario(
    replyTo: string,
    content: string,
    scenario: Scenario,
    source: MessageSource,
    rerun = false,
  ): void {
    const turn: TurnInFlight = { content, source, rerun, cancels: [], folded: [] };
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

    emit(startFrames(replyTo, content, source, rerun));
    hub.setBusy(agent, true);
    schedule(scenario.steps);
    later(scenario.endAt, () => {
      turn.cancels.push(
        env.whenTurnReleased("end", () => {
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
            if (scenario.failure !== null) latestFailure = { id: replyTo, content, source };
            const ofTurn = {
              timestamp: env.clock.iso(),
              visibility: "user",
              turn_id: replyTo,
            } as const;
            recordTurn(
              turn,
              { ...userRecord(content, source), turn_id: replyTo },
              scenario.recorded.map((message) => ({ ...message, ...ofTurn })),
            );
            hub.overview.changed(agent);
            startNextWaiting();
          });
        }),
      );
    });
  }

  function scenarioOf(replyTo: string, content: string, source: MessageSource): Scenario | null {
    if (source.endpoint === "ws") return scenarioFor(content, replyTo, env.nextId);
    if (source.agentSender !== undefined) {
      return sessionMessageTurn(replyTo, env.nextId, source.agentSender.address);
    }
    return externalTurn(replyTo, env.nextId);
  }

  function start(replyTo: string, content: string, source: MessageSource): void {
    latestFailure = null;
    const scenario = scenarioOf(replyTo, content, source);
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
    const reads = READ_PATHS.map((path, i) => ({
      id: `tc_mock_${String(env.nextId())}`,
      path,
      output:
        i === 1 && lower.startsWith("fail")
          ? `file not found: ${path}`
          : `   1\t# ${path}\n   2\t(the page as it is today)`,
      isError: i === 1 && lower.startsWith("fail"),
    }));
    const showcase = showcaseFor(lower);
    const response = showcase ?? cannedResponses[responseIndex % cannedResponses.length] ?? "";
    if (showcase === undefined) responseIndex++;

    const turn: TurnInFlight = { content, source, rerun: false, cancels: [], folded: [] };
    inFlight.set(replyTo, turn);
    const later = (ms: number, action: () => void): void => {
      turn.cancels.push(env.after(ms, action));
    };

    let down = false;
    const live = (frame: ServerMessage): void => {
      if (down) state.journalOnly(frame);
      else state.broadcast(frame);
    };

    for (const frame of startFrames(replyTo, content, source)) live(frame);
    hub.setBusy(agent, true);
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
      if (!finishWhileDown) {
        later(3500, () => {
          down = false;
        });
      }
    }

    later(turnLengthMs(drop, finishWhileDown), () => {
      turn.cancels.push(env.whenTurnReleased("end", endTurn));
    });

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
      const ofTurn = { timestamp: now, visibility: "user", turn_id: replyTo } as const;
      recordTurn(turn, { ...userRecord(content, source), turn_id: replyTo }, [
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
      ]);
      hub.overview.changed(agent);
      startNextWaiting();
    }
  }

  function arrive(id: string, content: string, source: MessageSource): void {
    if (inFlight.size === 0 && waiting.length === 0) {
      start(id, content, source);
      return;
    }
    waiting.push({ id, content, source });
    state.broadcast({
      type: "message_queued",
      message: {
        id,
        content,
        ...(source.images === undefined || source.images.length === 0
          ? {}
          : { images: source.images }),
        ...(source.attachments === undefined || source.attachments.length === 0
          ? {}
          : { attachments: source.attachments }),
        ...(source.sender === undefined ? {} : { sender: source.sender }),
        endpoint: source.endpoint,
      },
    });
  }

  function startNextWaiting(): void {
    if (inFlight.size > 0) return;
    const next = waiting.shift();
    if (next !== undefined) start(next.id, next.content, next.source);
  }

  function send(
    msg: SendMessage,
    attachments: readonly MessageAttachment[] = [],
    images: readonly ImageAttachment[] = [],
  ): void {
    arrive(msg.id, msg.content, {
      ...WEB,
      ...(images.length === 0 ? {} : { images: [...images] }),
      ...(attachments.length === 0 ? {} : { attachments: [...attachments] }),
    });
  }

  function receive(content: string, source: MessageSource): void {
    arrive(`${source.endpoint}-${String(env.nextId())}`, content, source);
  }

  function receiveFromSession(content: string, sender: AgentSender): void {
    const source: MessageSource = { endpoint: "background", agentSender: sender };
    const id = `background-${String(env.nextId())}`;
    const running = inFlight.entries().next().value;
    if (running === undefined) {
      start(id, content, source);
      return;
    }
    const [turnId, turn] = running;
    state.broadcast({
      type: "user_message",
      id,
      turn_id: turnId,
      content,
      agent_sender: sender,
      endpoint: source.endpoint,
    });
    turn.folded.push({ ...userRecord(content, source), turn_id: turnId });
  }

  function retry(replyTo: string): boolean {
    const turn = latestFailure;
    if (turn?.id !== replyTo || inFlight.size > 0) return false;
    latestFailure = null;
    runScenario(replyTo, turn.content, retriedTurn(replyTo, env.nextId), turn.source, true);
    return true;
  }

  function cancel(replyTo: string): void {
    const turn = inFlight.get(replyTo);
    if (turn === undefined) return;
    inFlight.delete(replyTo);
    for (const stop of turn.cancels) stop();
    state.broadcast({ type: "turn_ended", reply_to: replyTo });
    hub.setBusy(agent, false);
    recordTurn(turn, userRecord(turn.content, turn.source), []);
    hub.overview.changed(agent);
    startNextWaiting();
  }

  function sendWaitingNow(id: string): boolean {
    const index = waiting.findIndex((message) => message.id === id);
    if (index === -1) return false;
    const [message] = waiting.splice(index, 1);
    if (message !== undefined) waiting.unshift(message);
    for (const replyTo of [...inFlight.keys()]) cancel(replyTo);
    return true;
  }

  function withdrawWaiting(id: string): boolean {
    const index = waiting.findIndex((message) => message.id === id);
    if (index === -1) return false;
    waiting.splice(index, 1);
    state.broadcast({ type: "message_withdrawn", id });
    return true;
  }

  return { send, receive, receiveFromSession, cancel, retry, sendWaitingNow, withdrawWaiting };
}
