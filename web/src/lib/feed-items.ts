// ── Feed item building shared by the main chat and session transcripts

import { nextFeedId } from "./feed-id";
import { historyAgentMessage, parseArtifactMessage, parseOwnerMessage } from "./relay";
import type { AutoModeVerdict } from "./generated/AutoModeVerdict";
import type { DividerFeedItem, FeedItem, RecentMessage, ToolCallState } from "./types";

/**
 * Coerce tool-call arguments to an object, whatever shape they arrived in.
 *
 * Tool arguments reach the UI two ways and they do NOT agree: the history
 * endpoint serializes a Rust `serde_json::Value` (an **object**), while the
 * live socket has carried a JSON **string**. Every reader must go through
 * here — a bare `JSON.parse()` throws `SyntaxError` on the object form
 * (`JSON.parse` stringifies its argument first, yielding "[object Object]"),
 * and because feed building is a single pass, one throw blanks the entire
 * conversation rather than one message.
 *
 * Malformed input degrades to `{}` so a single bad record can't take the
 * feed down with it.
 */
export function normalizeToolArgs(value: unknown): Record<string, unknown> {
  if (typeof value === "string") {
    try {
      const parsed: unknown = JSON.parse(value);
      return typeof parsed === "object" && parsed !== null
        ? (parsed as Record<string, unknown>)
        : {};
    } catch {
      return {};
    }
  }
  return typeof value === "object" && value !== null ? (value as Record<string, unknown>) : {};
}

/**
 * Written ahead of each tool output so repeated results on one call stay
 * separable. The tool view strips it; it is not part of what the tool returned.
 */
export const TOOL_RESULT_MARKER = "─── result ───\n";

function appendResult(call: ToolCallState, output: string): void {
  call.result = (call.result ? call.result + "\n" : "") + TOOL_RESULT_MARKER + output;
}

/** How many of `messages` carry each turn id. */
export function countByTurn(messages: readonly RecentMessage[]): ReadonlyMap<string, number> {
  const counts = new Map<string, number>();
  for (const msg of messages) {
    if (msg.turn_id !== undefined) counts.set(msg.turn_id, (counts.get(msg.turn_id) ?? 0) + 1);
  }
  return counts;
}

/**
 * Whether the background turn in progress at some point in main's history is
 * shown: `shown` when an agent message kicked it off (a session's relayed
 * result and main's reply to it, which was shown live), `hidden` for other
 * background turns (pulses, scheduled work), `unknown` when the turn began in
 * older history that hasn't been converted.
 */
export type BackgroundTurnState = "shown" | "hidden" | "unknown";

export interface HistoryConversionOptions {
  /**
   * `main`: the main agent's history. Background-visibility turns are
   * hidden, except one an agent message kicked off. `session`: a session's
   * transcript, where every message is shown.
   */
  mode: "main" | "session";
  /** Called with each message's timestamp; returns a day divider to insert before it, if any. */
  dayDivider?: (timestamp: string) => DividerFeedItem | null;
  /**
   * `main` mode: the state of the background turn in progress where these
   * messages begin, when the older history before them is known.
   */
  carriedTurn?: BackgroundTurnState;
}

export interface HistoryConversion {
  items: FeedItem[];
  /**
   * `main` mode: leading background messages that continue a turn begun in
   * older history, left out of `items` until that history decides whether
   * the turn is shown. Empty when `carriedTurn` is known.
   */
  undecidedHead: RecentMessage[];
  /** `main` mode: the state of the background turn in progress where these messages end. */
  endTurn: BackgroundTurnState;
}

/** The message before the one being read, and whether it reached the agent mid-turn. */
interface Previous {
  msg: RecentMessage;
  midTurn: boolean;
}

/**
 * Whether `msg`, a user-role message, reached the agent while its turn ran.
 * The agent takes such a message in at its checkpoint after a tool batch, so
 * it follows a tool result of the same turn (or another message taken in at
 * the same checkpoint); a message that starts a turn follows the previous
 * turn's last reply, whatever id it carries.
 *
 * The main conversation's messages carry turn ids, which tie the two
 * together. Episodes and older records carry none, and each of their user
 * messages starts a turn. A session's transcript carries none either, but a
 * session is told of a message mid-turn only through that checkpoint, so
 * there a user message straight after a tool result counts. A turn that
 * ended on a tool result (it failed there) and is followed by a new message
 * reads as one that carried on.
 */
function reachedAgentMidTurn(
  msg: RecentMessage,
  previous: Previous | undefined,
  mode: HistoryConversionOptions["mode"],
): boolean {
  if (msg.role !== "user" || previous === undefined) return false;
  if (msg.turn_id !== previous.msg.turn_id) return false;
  if (msg.turn_id === undefined && mode !== "session") return false;
  return previous.msg.role === "tool" || (previous.msg.role === "user" && previous.midTurn);
}

/** Convert chat-history-shaped messages into feed items. */
export function convertHistory(
  messages: RecentMessage[],
  opts: HistoryConversionOptions,
): HistoryConversion {
  const out: FeedItem[] = [];
  const undecidedHead: RecentMessage[] = [];
  const toolCallItems = new Map<string, ToolCallState>();
  let turn: BackgroundTurnState = opts.carriedTurn ?? "unknown";
  /**
   * The message before this one. The main conversation leaves out the agent's
   * own notes to itself, which can come between a tool result and the message
   * taken in after it. A session's transcript has no ids to fall back on, so
   * there the messages must follow one another with nothing in between: a
   * note that a stop leaves after a tool result ends the turn.
   */
  let before: Previous | undefined;

  for (const msg of messages) {
    const midTurn = reachedAgentMidTurn(msg, before, opts.mode);
    if (msg.role !== "system" || opts.mode === "session") before = { msg, midTurn };
    const agentMessage = historyAgentMessage(msg, opts.mode);
    if (opts.mode === "main") {
      if (msg.role === "user") turn = agentMessage ? "shown" : "hidden";
      if (msg.visibility === "background") {
        if (turn === "unknown") {
          undecidedHead.push(msg);
          continue;
        }
        if (turn === "hidden") continue;
      }
    }

    if (opts.dayDivider && msg.timestamp) {
      const divider = opts.dayDivider(msg.timestamp);
      if (divider) out.push(divider);
    }

    const content = msg.content;
    const ofTurn = {
      ...(msg.turn_id === undefined ? {} : { turnId: msg.turn_id }),
      ...(midTurn ? { midTurn: true } : {}),
    };
    switch (msg.role) {
      case "user": {
        if (agentMessage) {
          out.push({
            id: nextFeedId(),
            kind: "agent-message",
            from: agentMessage.from,
            category: agentMessage.category,
            content: agentMessage.body,
            runId: null,
            ...ofTurn,
          });
          break;
        }
        const artifactMessage = opts.mode === "session" ? parseArtifactMessage(content) : null;
        if (artifactMessage) {
          out.push({
            id: nextFeedId(),
            kind: "user",
            content: artifactMessage.body,
            sender: {
              name: artifactMessage.artifact,
              id: `artifact:${artifactMessage.artifact}`,
              interface: "workbench artifact",
            },
            ...ofTurn,
          });
          break;
        }
        const ownerBody = opts.mode === "session" ? parseOwnerMessage(content) : null;
        out.push({
          id: nextFeedId(),
          kind: "user",
          content: ownerBody ?? content,
          sender: msg.sender,
          ...ofTurn,
        });
        break;
      }
      case "assistant": {
        // What the model reasoned comes before the text and the tool calls it led to.
        for (const thought of msg.thinking ?? []) {
          if (thought.trim()) {
            out.push({ id: nextFeedId(), kind: "thinking", content: thought, ...ofTurn });
          }
        }
        if (content.trim()) {
          out.push({ id: nextFeedId(), kind: "assistant", content, ...ofTurn });
        }
        if (msg.tool_calls && msg.tool_calls.length > 0) {
          const calls: ToolCallState[] = msg.tool_calls.map((tc) => {
            const call: ToolCallState = {
              id: tc.id,
              name: tc.name,
              arguments: normalizeToolArgs(tc.arguments),
              status: "done",
              server: tc.server,
            };
            toolCallItems.set(tc.id, call);
            return call;
          });
          out.push({ id: nextFeedId(), kind: "tool-group", calls, ...ofTurn });
        }
        break;
      }
      case "tool": {
        if (msg.tool_call_id) {
          const call = toolCallItems.get(msg.tool_call_id);
          if (call && content) appendResult(call, content);
          toolCallItems.delete(msg.tool_call_id);
        }
        break;
      }
      case "system":
        break;
    }
  }

  return { items: out, undecidedHead, endTurn: turn };
}

/** Convert chat-history-shaped messages into feed items, showing every message. */
export function convertHistoryMessages(
  messages: RecentMessage[],
  opts: HistoryConversionOptions,
): FeedItem[] {
  return convertHistory(messages, opts).items;
}

/**
 * The text identity of a feed item that both live frames and history
 * produce the same way, used to line the two up. `null` for items whose
 * shape differs between them (tool groups, dividers) or that history never
 * holds (local notes, status lines).
 */
export function feedItemSignature(item: FeedItem): string | null {
  if (item.kind === "user" || item.kind === "assistant" || item.kind === "agent-message") {
    return `${item.kind}\u0000${item.content}`;
  }
  return null;
}

/**
 * Append a live tool call to `feed`, joining the tool group at the tail if
 * there is one of the same turn and the same model call, and remember it in
 * `pending` so its result can find it. `turnId` is the turn in flight, and
 * `modelCall` the model call that made the tool call, when known.
 */
export function appendToolCall(
  feed: FeedItem[],
  pending: Map<string, ToolCallState>,
  call: { id: string; name: string; arguments: unknown; server?: string | null },
  turnId?: string,
  modelCall?: number,
): void {
  const state: ToolCallState = {
    id: call.id,
    name: call.name,
    arguments: normalizeToolArgs(call.arguments),
    status: "running",
    server: call.server,
    startedAt: Date.now(),
  };
  const last = feed[feed.length - 1];
  if (last?.kind === "tool-group" && last.turnId === turnId && last.call === modelCall) {
    last.calls.push(state);
  } else {
    feed.push({
      id: nextFeedId(),
      kind: "tool-group",
      calls: [state],
      ...(turnId === undefined ? {} : { turnId }),
      ...(modelCall === undefined ? {} : { call: modelCall }),
    });
  }
  // Re-read through `feed` so a `$state` feed hands back its proxied call
  // and later mutations stay reactive.
  const group = feed[feed.length - 1];
  if (group?.kind !== "tool-group") return;
  const stored = group.calls[group.calls.length - 1];
  if (stored) pending.set(call.id, stored);
}

/**
 * The turn ended with calls still waiting on results: `stopped` when it was
 * stopped or cut off, `done` when it finished and the page missed the result.
 */
export function settlePendingCalls(
  pending: Map<string, ToolCallState>,
  status: "done" | "stopped",
): void {
  const at = Date.now();
  for (const call of pending.values()) {
    call.status = status;
    call.endedAt = at;
  }
  pending.clear();
}

/** How many tool calls `items` hold for the turn `turnId`. */
export function countTurnCalls(items: readonly FeedItem[], turnId: string): number {
  let count = 0;
  for (const item of items) {
    if (item.kind === "tool-group" && item.turnId === turnId) count += item.calls.length;
  }
  return count;
}

/** Apply a live tool result to the call `pending` remembers for it. */
export function applyToolResult(
  pending: Map<string, ToolCallState>,
  result: {
    tool_call_id: string;
    output: string;
    is_error: boolean;
    auto_mode?: AutoModeVerdict;
  },
): void {
  const call = pending.get(result.tool_call_id);
  if (!call) return;
  call.status = result.is_error ? "error" : "done";
  call.endedAt = Date.now();
  if (result.auto_mode) call.autoMode = result.auto_mode;
  if (result.output) appendResult(call, result.output);
  pending.delete(result.tool_call_id);
}
