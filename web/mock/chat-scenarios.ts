import type { ServerMessage } from "../src/lib/generated/protocol";
import type { JsonValue } from "../src/lib/generated/serde_json/JsonValue";
import type { RecentMessage } from "../src/lib/types";

/** A message of a turn as history records it, before the mock stamps it with a time and the turn's id. */
export type ScenarioMessage = Pick<
  RecentMessage,
  "role" | "content" | "tool_calls" | "tool_call_id"
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

/** A scripted chat turn, chosen by how the user's message begins. */
export interface Scenario {
  /** What the agent does on the way, in order. */
  steps: ScenarioStep[];
  /** When the turn ends, after its last step. */
  endAt: number;
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

type ToolCall = NonNullable<RecentMessage["tool_calls"]>[number];

interface ScriptedCall {
  call: ToolCall;
  /** The call's arguments, which the live frame carries as JSON. */
  args: Record<string, JsonValue>;
  output: string;
  isError?: boolean;
}

/** What a scenario needs to address its frames to the turn it runs for. */
interface Context {
  /** The correlation id of the turn. */
  replyTo: string;
  nextId: NextId;
}

function frameOf(ctx: Context, call: number, scripted: ScriptedCall): ServerMessage {
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

function resultOf(ctx: Context, scripted: ScriptedCall): ServerMessage {
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

/** The history records of one model call: its text and tool calls, then each result. */
function recordCall(text: string, calls: ScriptedCall[]): ScenarioMessage[] {
  return [
    { role: "assistant", content: text, tool_calls: calls.map((scripted) => scripted.call) },
    ...calls.map(
      (scripted): ScenarioMessage => ({
        role: "tool",
        content: scripted.output,
        tool_call_id: scripted.call.id,
      }),
    ),
  ];
}

/** Hands out the ids of a scenario's tool calls. */
export type NextId = () => number;

/** A text the agent sends on the way, written by the model call numbered `call`. */
function textOf(ctx: Context, call: number, content: string): ServerMessage {
  return { type: "broadcast_response", reply_to: ctx.replyTo, call, content };
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
 * A turn the agent works through in three rounds, saying what it is about to
 * do before each: it reads three files, then runs two commands, then edits
 * the file, and its reply follows. Its texts and its work alternate, so the
 * chat shows runs of steps between messages.
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
  const first = "Let me check the config first.";
  const second = "The port is set twice. Fixing:";
  const third = "One edit should do it.";
  const reply = "Done. The port is set once now, at 8080, and the service picked it up.";
  return {
    steps: [
      { at: 300, frames: [textOf(ctx, 0, first), ...reads.map((r) => frameOf(ctx, 0, r))] },
      { at: 900, frames: reads.map((r) => resultOf(ctx, r)) },
      { at: 1300, frames: [textOf(ctx, 1, second), ...commands.map((c) => frameOf(ctx, 1, c))] },
      { at: 1900, frames: commands.map((c) => resultOf(ctx, c)) },
      { at: 2300, frames: [textOf(ctx, 2, third), frameOf(ctx, 2, edit)] },
      { at: 2700, frames: [resultOf(ctx, edit)] },
    ],
    endAt: 3000,
    toolCalls: reads.length + commands.length + 1,
    reply,
    replyCall: 3,
    failure: null,
    recorded: [
      ...recordCall(first, reads),
      ...recordCall(second, commands),
      ...recordCall(third, [edit]),
      { role: "assistant", content: reply },
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
  return {
    steps: [
      {
        at: 300,
        frames: [textOf(ctx, 0, "Looking through recent notes first."), frameOf(ctx, 0, search)],
      },
      { at: 700, frames: [resultOf(ctx, search)] },
    ],
    endAt: 1000,
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
 * The scripted turn for a message, or null for one the plain simulated turn
 * answers. A message starting with:
 * - `segments` has the agent work in three rounds, with a text before each
 * - `error` has the turn fail after a search
 */
export function scenarioFor(content: string, replyTo: string, nextId: NextId): Scenario | null {
  const ctx: Context = { replyTo, nextId };
  const lower = content.toLowerCase();
  if (lower.startsWith("segments")) return segments(ctx);
  if (lower.startsWith("error")) return failing(ctx);
  return null;
}
