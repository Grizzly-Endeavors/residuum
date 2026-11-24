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

export const TOOL_RESULT_MARKER = "─── result ───\n";

function appendResult(call: ToolCallState, output: string): void {
  call.result = (call.result ? call.result + "\n" : "") + TOOL_RESULT_MARKER + output;
}

function sentWsMessage(call: ToolCallState, resultContent: string): string | null {
  if (call.name !== "send_message") return null;
  if (call.arguments.endpoint !== "ws") return null;
  if (typeof call.arguments.file_path === "string" && call.arguments.file_path.length > 0) {
    return null;
  }
  if (typeof call.arguments.conversation === "string" && call.arguments.conversation.length > 0) {
    return null;
  }
  const message = call.arguments.message;
  if (typeof message !== "string" || !message.trim()) return null;
  const firstLine = resultContent.split("\n")[0];
  if (firstLine !== "Message published to endpoint 'ws'") return null;
  return message;
}

export function countByTurn(messages: readonly RecentMessage[]): ReadonlyMap<string, number> {
  const counts = new Map<string, number>();
  for (const msg of messages) {
    if (msg.turn_id !== undefined) counts.set(msg.turn_id, (counts.get(msg.turn_id) ?? 0) + 1);
  }
  return counts;
}

export type BackgroundTurnState = "shown" | "hidden" | "unknown";

export interface HistoryConversionOptions {
  mode: "main" | "session";
  dayDivider?: (timestamp: string) => DividerFeedItem | null;
  timestamps?: boolean;
  carriedTurn?: BackgroundTurnState;
}

export interface HistoryConversion {
  items: FeedItem[];
  undecidedHead: RecentMessage[];
  endTurn: BackgroundTurnState;
}

interface Previous {
  msg: RecentMessage;
  midTurn: boolean;
}

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

export function convertHistory(
  messages: RecentMessage[],
  opts: HistoryConversionOptions,
): HistoryConversion {
  const out: FeedItem[] = [];
  const undecidedHead: RecentMessage[] = [];
  const toolCallItems = new Map<string, ToolCallState>();
  let turn: BackgroundTurnState = opts.carriedTurn ?? "unknown";
  let before: Previous | undefined;
  const stamped = opts.timestamps ?? opts.mode === "main";

  for (const msg of messages) {
    if (msg.context === true) continue;
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
    const sent = stamped && msg.timestamp ? { timestamp: msg.timestamp } : {};
    switch (msg.role) {
      case "user": {
        if (agentMessage) {
          out.push({
            id: nextFeedId(),
            kind: "agent-message",
            from: agentMessage.from,
            category: agentMessage.category,
            content: agentMessage.body,
            runId: msg.agent_sender?.run_id ?? null,
            ...ofTurn,
            ...sent,
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
            ...sent,
          });
          break;
        }
        const ownerBody = opts.mode === "session" ? parseOwnerMessage(content) : null;
        out.push({
          id: nextFeedId(),
          kind: "user",
          content: ownerBody ?? content,
          ...(msg.images === undefined || msg.images.length === 0 ? {} : { images: msg.images }),
          ...(msg.attachments === undefined || msg.attachments.length === 0
            ? {}
            : { attachments: msg.attachments }),
          sender: msg.sender,
          ...ofTurn,
          ...sent,
        });
        break;
      }
      case "assistant": {
        for (const thought of msg.thinking ?? []) {
          if (thought.trim()) {
            out.push({ id: nextFeedId(), kind: "thinking", content: thought, ...ofTurn });
          }
        }
        if (content.trim()) {
          out.push({ id: nextFeedId(), kind: "assistant", content, ...ofTurn, ...sent });
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
        const call = msg.tool_call_id ? toolCallItems.get(msg.tool_call_id) : undefined;
        if (msg.tool_call_id) {
          if (call && content) appendResult(call, content);
          toolCallItems.delete(msg.tool_call_id);
        }
        if (msg.sent_attachment) {
          out.push({
            id: nextFeedId(),
            kind: "file-attachment",
            filename: msg.sent_attachment.filename,
            mimeType: msg.sent_attachment.mime_type,
            size: msg.sent_attachment.size,
            url: msg.sent_attachment.url,
            caption: msg.sent_attachment.caption ?? null,
            ...ofTurn,
            ...sent,
          });
        } else if (call) {
          const wsMessage = sentWsMessage(call, content);
          if (wsMessage !== null) {
            out.push({
              id: nextFeedId(),
              kind: "assistant",
              content: wsMessage,
              ...ofTurn,
              ...sent,
            });
          }
        }
        break;
      }
      case "system":
        break;
    }
  }

  return { items: out, undecidedHead, endTurn: turn };
}

export function convertHistoryMessages(
  messages: RecentMessage[],
  opts: HistoryConversionOptions,
): FeedItem[] {
  return convertHistory(messages, opts).items;
}

export function feedItemSignature(item: FeedItem): string | null {
  if (item.kind === "user" || item.kind === "assistant" || item.kind === "agent-message") {
    return `${item.kind}\u0000${item.content}`;
  }
  if (item.kind === "file-attachment") {
    return `${item.kind}\u0000${item.filename}\u0000${item.caption ?? ""}`;
  }
  return null;
}

export function appendToolCall(
  feed: FeedItem[],
  pending: Map<string, ToolCallState>,
  call: { id: string; name: string; arguments: unknown; server?: string | null },
  turnId?: string,
  modelCall?: number,
  at: number = Date.now(),
): void {
  const state: ToolCallState = {
    id: call.id,
    name: call.name,
    arguments: normalizeToolArgs(call.arguments),
    status: "running",
    server: call.server,
    startedAt: at,
  };
  const last = feed[feed.length - 1];
  if (
    last?.kind === "tool-group" &&
    last.closed !== true &&
    last.turnId === turnId &&
    last.call === modelCall
  ) {
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
  const group = feed[feed.length - 1];
  if (group?.kind !== "tool-group") return;
  const stored = group.calls[group.calls.length - 1];
  if (stored) pending.set(call.id, stored);
}

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

export function countTurnCalls(items: readonly FeedItem[], turnId: string): number {
  let count = 0;
  for (const item of items) {
    if (item.kind === "tool-group" && item.turnId === turnId) count += item.calls.length;
  }
  return count;
}

export function applyToolResult(
  pending: Map<string, ToolCallState>,
  result: {
    tool_call_id: string;
    output: string;
    is_error: boolean;
    auto_mode?: AutoModeVerdict;
  },
  at: number = Date.now(),
): void {
  const call = pending.get(result.tool_call_id);
  if (!call) return;
  call.status = result.is_error ? "error" : "done";
  call.endedAt = at;
  if (result.auto_mode) call.autoMode = result.auto_mode;
  if (result.output) appendResult(call, result.output);
  pending.delete(result.tool_call_id);
}
