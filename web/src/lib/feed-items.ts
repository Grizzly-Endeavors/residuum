// ── Feed item building shared by the main chat and session views ─────

import { nextFeedId } from "./feed-id";
import { historyAgentMessage, parseArtifactMessage, parseOwnerMessage } from "./relay";
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

const RESULT_SEPARATOR = "─── result ───\n";

function appendResult(call: ToolCallState, output: string): void {
  call.result = (call.result ? call.result + "\n" : "") + RESULT_SEPARATOR + output;
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

/** Convert chat-history-shaped messages into feed items. */
export function convertHistory(
  messages: RecentMessage[],
  opts: HistoryConversionOptions,
): HistoryConversion {
  const out: FeedItem[] = [];
  const undecidedHead: RecentMessage[] = [];
  const toolCallItems = new Map<string, ToolCallState>();
  let turn: BackgroundTurnState = opts.carriedTurn ?? "unknown";

  for (const msg of messages) {
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
          });
          break;
        }
        const ownerBody = opts.mode === "session" ? parseOwnerMessage(content) : null;
        out.push({
          id: nextFeedId(),
          kind: "user",
          content: ownerBody ?? content,
          sender: msg.sender,
        });
        break;
      }
      case "assistant": {
        if (content.trim()) {
          out.push({ id: nextFeedId(), kind: "assistant", content });
        }
        if (msg.tool_calls && msg.tool_calls.length > 0) {
          const calls: ToolCallState[] = msg.tool_calls.map((tc) => {
            const call: ToolCallState = {
              id: tc.id,
              name: tc.name,
              arguments: normalizeToolArgs(tc.arguments),
              status: "done",
            };
            toolCallItems.set(tc.id, call);
            return call;
          });
          out.push({ id: nextFeedId(), kind: "tool-group", calls });
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
 * there is one, and remember it in `pending` so its result can find it.
 */
export function appendToolCall(
  feed: FeedItem[],
  pending: Map<string, ToolCallState>,
  call: { id: string; name: string; arguments: unknown },
): void {
  const state: ToolCallState = {
    id: call.id,
    name: call.name,
    arguments: normalizeToolArgs(call.arguments),
    status: "running",
  };
  const last = feed[feed.length - 1];
  if (last?.kind === "tool-group") {
    last.calls.push(state);
  } else {
    feed.push({ id: nextFeedId(), kind: "tool-group", calls: [state] });
  }
  // Re-read through `feed` so a `$state` feed hands back its proxied call
  // and later mutations stay reactive.
  const group = feed[feed.length - 1];
  if (group?.kind !== "tool-group") return;
  const stored = group.calls[group.calls.length - 1];
  if (stored) pending.set(call.id, stored);
}

/** Apply a live tool result to the call `pending` remembers for it. */
export function applyToolResult(
  pending: Map<string, ToolCallState>,
  result: { tool_call_id: string; output: string; is_error: boolean },
): void {
  const call = pending.get(result.tool_call_id);
  if (!call) return;
  call.status = result.is_error ? "error" : "done";
  if (result.output) appendResult(call, result.output);
  pending.delete(result.tool_call_id);
}
