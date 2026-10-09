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

function frameOf(scripted: ScriptedCall): ServerMessage {
  const { call } = scripted;
  return {
    type: "tool_call",
    id: call.id,
    name: call.name,
    arguments: scripted.args,
    server: call.server ?? null,
  };
}

function resultOf(scripted: ScriptedCall): ServerMessage {
  const { call } = scripted;
  return {
    type: "tool_result",
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

function scriptCall(
  nextId: NextId,
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
function segments(nextId: NextId): Scenario {
  const reads = ["config.toml", "docker-compose.yml", "README.md"].map((file) =>
    scriptCall(
      nextId,
      "read_file",
      { path: `team/wiki/${file}` },
      `   1\t# ${file}\n   2\t(as it is today)`,
    ),
  );
  const commands = [
    scriptCall(
      nextId,
      "exec",
      { command: "grep -n port config.toml" },
      "12:port = 8080\n47:port = 8081",
    ),
    scriptCall(nextId, "exec", { command: "systemctl status residuum" }, "active (running)"),
  ];
  const edit = scriptCall(
    nextId,
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
      { at: 300, frames: [{ type: "broadcast_response", content: first }, ...reads.map(frameOf)] },
      { at: 900, frames: reads.map(resultOf) },
      {
        at: 1300,
        frames: [{ type: "broadcast_response", content: second }, ...commands.map(frameOf)],
      },
      { at: 1900, frames: commands.map(resultOf) },
      { at: 2300, frames: [{ type: "broadcast_response", content: third }, frameOf(edit)] },
      { at: 2700, frames: [resultOf(edit)] },
    ],
    endAt: 3000,
    toolCalls: reads.length + commands.length + 1,
    reply,
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
function failing(nextId: NextId): Scenario {
  const search = scriptCall(
    nextId,
    "memory_search",
    { query: "notification routing", limit: 5 },
    '[{"text":"Found 3 relevant observations from recent conversations.","score":0.87}]',
  );
  return {
    steps: [
      {
        at: 300,
        frames: [
          { type: "broadcast_response", content: "Looking through recent notes first." },
          frameOf(search),
        ],
      },
      { at: 700, frames: [resultOf(search)] },
    ],
    endAt: 1000,
    toolCalls: 1,
    reply: null,
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
export function scenarioFor(content: string, nextId: NextId): Scenario | null {
  const lower = content.toLowerCase();
  if (lower.startsWith("segments")) return segments(nextId);
  if (lower.startsWith("error")) return failing(nextId);
  return null;
}
