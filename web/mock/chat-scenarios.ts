import type { ServerMessage } from "../src/lib/generated/protocol";
import type { JsonValue } from "../src/lib/generated/serde_json/JsonValue";
import type { RecentMessage } from "../src/lib/types";

/** A message of a turn as history records it, before the mock stamps it with a time and the turn's id. */
export type ScenarioMessage = Pick<
  RecentMessage,
  "role" | "content" | "tool_calls" | "tool_call_id" | "thinking"
>;

/** Frames the mock sends a number of milliseconds into a turn. */
export interface ScenarioStep {
  at: number;
  frames: ServerMessage[];
}

/** How a scripted turn goes wrong. */
export interface ScenarioFailure {
  /** What the agent says went wrong, in plain words. */
  message: string;
  /** The technical cause chain behind it. */
  details: string;
}

/** A scripted chat turn. */
export interface Scenario {
  /** What the agent does on the way, in order. */
  steps: ScenarioStep[];
  /** When the steps are over. A test that holds turns keeps the turn here until it lets it end. */
  endAt: number;
  /** What follows once the turn is let end: its last model call, timed from that moment. */
  finale: ScenarioStep[];
  /** How long the finale takes. */
  finaleMs: number;
  /** The tool calls the turn makes, which add to the conversation's totals. */
  toolCalls: number;
  /** The reply that ends the turn, or null when it fails instead. */
  reply: string | null;
  /** The model call that wrote the reply, counting from zero. */
  replyCall: number;
  failure: ScenarioFailure | null;
  /** What history records for the turn after the user's message; empty for a turn that failed. */
  recorded: ScenarioMessage[];
}

/** Hands out the ids of a scenario's tool calls. */
export type NextId = () => number;

/** What a scenario needs to address its frames to the turn it runs for. */
interface Context {
  /** The correlation id of the turn. */
  replyTo: string;
  nextId: NextId;
}

type ToolCall = NonNullable<RecentMessage["tool_calls"]>[number];

interface ScriptedCall {
  call: ToolCall;
  /** The call's arguments, which the live frame carries as JSON. */
  args: Record<string, JsonValue>;
  output: string;
  isError?: boolean;
}

function scriptCall(
  { nextId }: Context,
  name: string,
  args: Record<string, JsonValue>,
  output: string,
  isError = false,
): ScriptedCall {
  return {
    call: { id: `tc_mock_${String(nextId())}`, name, arguments: args, server: null },
    args,
    output,
    isError,
  };
}

/**
 * How a scenario's frames are timed: words arrive in pieces of about this
 * many characters, one piece this many milliseconds after the last.
 */
const PIECE_CHARS = 28;
export const PIECE_MS = 45;

/** `text` in pieces that end at word breaks, about `PIECE_CHARS` long. */
export function pieces(text: string): string[] {
  const out: string[] = [];
  let piece = "";
  for (const word of text.match(/\S+\s*/g) ?? []) {
    if (piece !== "" && piece.length + word.length > PIECE_CHARS) {
      out.push(piece);
      piece = "";
    }
    piece += word;
  }
  if (piece !== "") out.push(piece);
  return out;
}

/** The frames that stream `text` (or reasoning) of model call `call` to the page, in pieces. */
export function deltaFrames(
  replyTo: string,
  kind: "text" | "thinking",
  call: number,
  text: string,
): ServerMessage[] {
  return pieces(text).map((piece) =>
    kind === "text"
      ? { type: "text_delta", reply_to: replyTo, call, text: piece }
      : { type: "thinking_delta", reply_to: replyTo, call, text: piece },
  );
}

/**
 * The frames of a turn no person started, such as the one a teammate's message
 * starts, from its start to its end: the reply written in pieces, then
 * complete, and the turn's usage. The message that started it has no frame,
 * as the backend announces only what a person sent, and the reply went
 * nowhere, so its endpoint is empty.
 */
export function backgroundTurnFrames(replyTo: string, reply: string): ServerMessage[] {
  return [
    {
      type: "turn_started",
      reply_to: replyTo,
      origin: { endpoint: "background", visibility: "background" },
    },
    ...deltaFrames(replyTo, "text", 0, reply),
    { type: "response", reply_to: replyTo, call: 0, endpoint: "", content: reply },
    {
      type: "turn_usage",
      reply_to: replyTo,
      output_tokens: Math.ceil(reply.length / 4),
      has_usage: true,
      tool_calls: 0,
      session_totals: null,
    },
    { type: "turn_ended", reply_to: replyTo },
  ];
}

/** Builds the timed frames of one stretch of a turn. */
class Timeline {
  readonly steps: ScenarioStep[] = [];

  constructor(private readonly ctx: Context) {}

  /** Send `frames` at `at`. */
  at(at: number, ...frames: ServerMessage[]): number {
    this.steps.push({ at, frames });
    return at;
  }

  /** Send the text or reasoning of model call `call` in pieces from `from`; the time after the last. */
  stream(kind: "text" | "thinking", call: number, text: string, from: number): number {
    let at = from;
    for (const frame of deltaFrames(this.ctx.replyTo, kind, call, text)) {
      this.at(at, frame);
      at += PIECE_MS;
    }
    return at;
  }

  /** The reasoning of model call `call` is complete. */
  thought(at: number, call: number, content: string): number {
    return this.at(at, {
      type: "thinking",
      reply_to: this.ctx.replyTo,
      call,
      content,
    });
  }

  /** A text the agent sends on the way, complete. */
  text(at: number, call: number, content: string): ServerMessage {
    const frame: ServerMessage = {
      type: "broadcast_response",
      reply_to: this.ctx.replyTo,
      call,
      content,
    };
    this.at(at, frame);
    return frame;
  }

  call(at: number, call: number, ...calls: ScriptedCall[]): number {
    return this.at(at, ...calls.map((scripted) => callFrame(this.ctx, call, scripted)));
  }

  results(at: number, ...calls: ScriptedCall[]): number {
    return this.at(at, ...calls.map((scripted) => resultFrame(this.ctx, scripted)));
  }
}

function callFrame(ctx: Context, call: number, scripted: ScriptedCall): ServerMessage {
  const { call: record } = scripted;
  return {
    type: "tool_call",
    reply_to: ctx.replyTo,
    call,
    id: record.id,
    name: record.name,
    arguments: scripted.args,
    server: record.server ?? null,
  };
}

function resultFrame(ctx: Context, scripted: ScriptedCall): ServerMessage {
  const { call } = scripted;
  return {
    type: "tool_result",
    reply_to: ctx.replyTo,
    tool_call_id: call.id,
    name: call.name,
    output: scripted.output,
    is_error: scripted.isError ?? false,
  };
}

/** The history records of one model call: its reasoning and text, its tool calls, then each result. */
function recordCall(
  text: string,
  calls: ScriptedCall[],
  thinking: string[] = [],
): ScenarioMessage[] {
  return [
    {
      role: "assistant",
      content: text,
      tool_calls: calls.map((scripted) => scripted.call),
      ...(thinking.length === 0 ? {} : { thinking }),
    },
    ...calls.map(
      (scripted): ScenarioMessage => ({
        role: "tool",
        content: scripted.output,
        tool_call_id: scripted.call.id,
      }),
    ),
  ];
}

/**
 * A turn the agent works through in rounds, the way the activity reads in the
 * chat: it thinks and reads three files, then says what it will check and runs
 * two commands, then says what it found and edits the file. Its last model
 * call thinks, then answers. Texts and work alternate, so the chat shows runs
 * of steps between messages.
 */
function segments(ctx: Context): Scenario {
  const reads = ["config.toml", "docker-compose.yml", "README.md"].map((file) =>
    scriptCall(
      ctx,
      "read_file",
      { path: `team/wiki/${file}` },
      `   1\t# ${file}\n   2\t(as it is today)`,
    ),
  );
  const commands = [
    scriptCall(
      ctx,
      "exec",
      { command: "grep -n port config.toml" },
      "12:port = 8080\n47:port = 8081",
    ),
    scriptCall(ctx, "exec", { command: "systemctl status residuum" }, "active (running)"),
  ];
  const edit = scriptCall(
    ctx,
    "edit_file",
    { path: "team/wiki/config.toml" },
    "edited team/wiki/config.toml",
  );
  const readingThought =
    "The report is about a port clash. I should read the config, the compose file and the README before I touch anything.";
  const lastThought =
    "The edit is in. The service has to restart before the port takes effect, so I should check it picked the change up.";
  const first = "Let me check the config first.";
  const second = "The port is set twice. Fixing:";
  const reply = "Done. The port is set once now, at 8080, and the service picked it up.";

  const run = new Timeline(ctx);
  let at = run.stream("thinking", 0, readingThought, 20);
  run.thought(at, 0, readingThought);
  run.call(at + 40, 0, ...reads);
  run.results(at + 440, ...reads);
  at = run.stream("text", 1, first, at + 480);
  run.text(at + 20, 1, first);
  run.call(at + 20, 1, ...commands);
  run.results(at + 520, ...commands);
  at = run.stream("text", 2, second, at + 560);
  run.text(at + 20, 2, second);
  run.call(at + 20, 2, edit);
  run.results(at + 420, edit);
  const endAt = at + 460;

  const finale = new Timeline(ctx);
  const thinkingEnds = finale.stream("thinking", 3, lastThought, 0);
  finale.thought(thinkingEnds, 3, lastThought);
  const replyEnds = finale.stream("text", 3, reply, thinkingEnds + 40);

  return {
    steps: run.steps,
    endAt,
    finale: finale.steps,
    finaleMs: replyEnds + 40,
    toolCalls: reads.length + commands.length + 1,
    reply,
    replyCall: 3,
    failure: null,
    recorded: [
      ...recordCall("", reads, [readingThought]),
      ...recordCall(first, commands),
      ...recordCall(second, [edit]),
      { role: "assistant", content: reply, thinking: [lastThought] },
    ],
  };
}

/**
 * A turn that gets as far as a search and then can't go on: the model
 * provider stops answering, and the agent reports it.
 */
function failing(ctx: Context): Scenario {
  const search = scriptCall(
    ctx,
    "memory_search",
    { query: "notification routing", limit: 5 },
    '[{"text":"Found 3 relevant observations from recent conversations.","score":0.87}]',
  );
  const note = "Looking through recent notes first.";
  const run = new Timeline(ctx);
  const at = run.stream("text", 0, note, 20);
  run.text(at + 20, 0, note);
  run.call(at + 20, 0, search);
  run.results(at + 420, search);
  return {
    steps: run.steps,
    endAt: at + 700,
    finale: [],
    finaleMs: 0,
    toolCalls: 1,
    reply: null,
    replyCall: 0,
    failure: {
      message: "The model provider didn't answer. Try sending your message again in a moment.",
      details:
        "model call failed after 3 attempts\n  caused by: provider returned 503 Service Unavailable\n  caused by: upstream connect timeout after 30s",
    },
    recorded: [],
  };
}

/**
 * A reply whose first attempt stops partway: the stream starts over, and the
 * second attempt is what the user is left with.
 */
function retrying(ctx: Context): Scenario {
  const attempt = "Let me look at the notification routing doc and see what";
  const answer = "The routing doc sends urgent notices to every channel and the rest to one.";
  const run = new Timeline(ctx);
  const first = run.stream("text", 0, attempt, 20);
  run.at(first + 400, { type: "stream_restart", reply_to: ctx.replyTo, call: 0 });
  const second = run.stream("text", 0, answer, first + 1200);
  return {
    steps: run.steps,
    endAt: second + 60,
    finale: [],
    finaleMs: 0,
    toolCalls: 0,
    reply: answer,
    replyCall: 0,
    failure: null,
    recorded: [{ role: "assistant", content: answer }],
  };
}

/**
 * A short turn that is mostly thought: a long stretch of reasoning, then a
 * brief answer.
 */
function thinkingAloud(ctx: Context): Scenario {
  const thought = [
    "The question is whether to cascade to the next channel or park the notice.",
    "Cascading is what most setups expect, but parking is the safest default when every channel is down.",
    "The doc says retry three times with backoff first, so cascade only after that.",
    "I should answer with that order and name where the doc says it.",
  ].join("\n");
  const reply = "Retry three times with backoff, then cascade, then park it in the inbox.";
  const finale = new Timeline(ctx);
  const thoughtEnds = finale.stream("thinking", 0, thought, 0);
  finale.thought(thoughtEnds, 0, thought);
  const replyEnds = finale.stream("text", 0, reply, thoughtEnds + 40);
  return {
    steps: [],
    endAt: 40,
    finale: finale.steps,
    finaleMs: replyEnds + 40,
    toolCalls: 0,
    reply,
    replyCall: 0,
    failure: null,
    recorded: [{ role: "assistant", content: reply, thinking: [thought] }],
  };
}

/**
 * A turn another channel started, such as a message from Telegram: a short
 * thought, a search, and a reply.
 */
export function externalTurn(replyTo: string, nextId: NextId): Scenario {
  const ctx: Context = { replyTo, nextId };
  const search = scriptCall(
    ctx,
    "memory_search",
    { query: "notification routing", limit: 5 },
    '[{"text":"Found 3 relevant observations from recent conversations.","score":0.87}]',
  );
  const thought = "Alex is asking about the routing doc. A memory search will have the summary.";
  const note = "Checking the routing notes.";
  const reply =
    "Urgent notices go to every channel at once. Everything else goes to the first one that answers.";
  const run = new Timeline(ctx);
  let at = run.stream("thinking", 0, thought, 20);
  run.thought(at, 0, thought);
  at = run.stream("text", 0, note, at + 40);
  run.text(at + 20, 0, note);
  run.call(at + 20, 0, search);
  run.results(at + 420, search);
  const finale = new Timeline(ctx);
  const replyEnds = finale.stream("text", 1, reply, 0);
  return {
    steps: run.steps,
    endAt: at + 460,
    finale: finale.steps,
    finaleMs: replyEnds + 40,
    toolCalls: 1,
    reply,
    replyCall: 1,
    failure: null,
    recorded: [...recordCall(note, [search], [thought]), { role: "assistant", content: reply }],
  };
}

/**
 * The scripted turn for a message, or null for one the plain simulated turn
 * answers. A message starting with:
 * - `segments` has the agent work in rounds, with a text before each
 * - `error` has the turn fail after a search
 * - `retry` has a reply's stream start over
 * - `think` has the agent think at length before a short answer
 */
export function scenarioFor(content: string, replyTo: string, nextId: NextId): Scenario | null {
  const ctx: Context = { replyTo, nextId };
  const lower = content.toLowerCase();
  if (lower.startsWith("segments")) return segments(ctx);
  if (lower.startsWith("error")) return failing(ctx);
  if (lower.startsWith("retry")) return retrying(ctx);
  if (lower.startsWith("think")) return thinkingAloud(ctx);
  return null;
}
