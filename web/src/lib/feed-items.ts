// ── Feed item building shared by the main chat and session views ─────

import { nextFeedId } from "./feed-id";
import { parseAgentMessage, parseOwnerMessage } from "./relay";
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

export interface HistoryConversionOptions {
  /**
   * `main`: the main agent's history. Background-visibility turns are
   * hidden, except one an agent message kicked off (a session's relayed
   * result and main's reply to it, which was shown live). `session`: a
   * session's transcript, where every message is shown.
   */
  mode: "main" | "session";
  /** Called with each message's timestamp; returns a day divider to insert before it, if any. */
  dayDivider?: (timestamp: string) => DividerFeedItem | null;
}

/** Convert chat-history-shaped messages into feed items. */
export function convertHistoryMessages(
  messages: RecentMessage[],
  opts: HistoryConversionOptions,
): FeedItem[] {
  const out: FeedItem[] = [];
  const toolCallItems = new Map<string, ToolCallState>();
  // Whether the background turn in progress was started by an agent message.
  let showingBackgroundTurn = false;

  for (const msg of messages) {
    const agentMessage = msg.role === "user" ? parseAgentMessage(msg.content) : null;
    if (opts.mode === "main") {
      if (msg.role === "user") {
        showingBackgroundTurn = msg.visibility === "background" && agentMessage !== null;
      }
      if (msg.visibility === "background" && !showingBackgroundTurn) continue;
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

  return out;
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
