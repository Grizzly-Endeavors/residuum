// ── Feed store (Svelte 5 runes) ──────────────────────────────────────

import { SvelteMap } from "svelte/reactivity";
import { nextFeedId } from "./feed-id";
import type {
  ServerMessage,
  RecentMessage,
  FeedItem,
  ToolGroupFeedItem,
  ToolCallState,
  ImageAttachment,
  FileAttachmentFeedItem,
} from "./types";

/** Manages the chat feed state and processes incoming server messages. */
export class FeedStore {
  feed = $state<FeedItem[]>([]);
  isProcessing = $state(false);

  private pendingToolCalls = new SvelteMap<string, ToolCallState>();

  /** Dispatch a server message into the feed. */
  handleMessage(msg: ServerMessage): void {
    switch (msg.type) {
      case "turn_started":
        this.isProcessing = true;
        break;

      case "tool_call":
        this.handleToolCall(msg);
        break;

      case "tool_result":
        this.handleToolResult(msg);
        break;

      case "response":
        this.isProcessing = false;
        if (msg.content) {
          this.feed.push({
            id: nextFeedId(),
            kind: "assistant",
            content: msg.content,
          });
        }
        break;

      case "broadcast_response":
        if (msg.content) {
          this.feed.push({
            id: nextFeedId(),
            kind: "assistant",
            content: msg.content,
          });
        }
        break;

      case "error":
        this.isProcessing = false;
        this.feed.push({
          id: nextFeedId(),
          kind: "error",
          content: msg.message,
        });
        break;

      case "notice":
        this.feed.push({
          id: nextFeedId(),
          kind: "notice",
          content: msg.message,
        });
        break;

      case "reloading":
        this.feed.push({
          id: nextFeedId(),
          kind: "system",
          content: "Gateway is reloading...",
        });
        break;

      case "file_attachment": {
        const item: FileAttachmentFeedItem = {
          id: nextFeedId(),
          kind: "file-attachment",
          filename: msg.filename,
          mimeType: msg.mime_type,
          size: msg.size,
          url: msg.url,
          caption: msg.caption,
        };
        this.feed.push(item);
        this.isProcessing = false;
        break;
      }

      case "pong":
        break;
    }
  }

  /** Populate the feed from chat history. */
  loadHistory(messages: RecentMessage[]): void {
    this.feed.length = 0;
    this.pendingToolCalls.clear();
    if (!messages.length) return;

    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- local non-reactive scratch map
    const toolCallItems = new Map<string, ToolCallState>();

    for (const msg of messages.filter((m) => m.visibility !== "background")) {
      const content = msg.content || "";
      switch (msg.role) {
        case "user":
          this.feed.push({ id: nextFeedId(), kind: "user", content });
          break;
        case "assistant": {
          if (content.trim()) {
            this.feed.push({ id: nextFeedId(), kind: "assistant", content });
          }
          if (msg.tool_calls?.length) {
            const calls: ToolCallState[] = msg.tool_calls.map((tc) => {
              const args =
                typeof tc.arguments === "string"
                  ? (JSON.parse(tc.arguments) as Record<string, unknown>)
                  : ((tc.arguments as Record<string, unknown>) ?? {});
              const call: ToolCallState = {
                id: tc.id,
                name: tc.name,
                arguments: args,
                status: "done",
              };
              toolCallItems.set(tc.id, call);
              return call;
            });
            this.feed.push({ id: nextFeedId(), kind: "tool-group", calls });
          }
          break;
        }
        case "tool": {
          if (msg.tool_call_id) {
            const call = toolCallItems.get(msg.tool_call_id);
            if (call && content) {
              call.result =
                (call.result ? call.result + "\n" : "") +
                "\u2500\u2500\u2500 result \u2500\u2500\u2500\n" +
                content;
            }
            toolCallItems.delete(msg.tool_call_id);
          }
          break;
        }
        case "system":
          break;
      }
    }

    this.feed.push({
      id: nextFeedId(),
      kind: "divider",
      label: "\u2014 session resumed \u2014",
    });
  }

  /** Add a user message to the feed. */
  pushUserMessage(content: string, images?: ImageAttachment[]): void {
    this.feed.push({ id: nextFeedId(), kind: "user", content, images });
    this.isProcessing = true;
  }

  /** Append an arbitrary feed item (e.g. from commands). */
  appendFeedItem(item: FeedItem): void {
    this.feed.push(item);
  }

  // ── Private ──────────────────────────────────────────────────────────

  private handleToolCall(msg: Extract<ServerMessage, { type: "tool_call" }>): void {
    const args =
      typeof msg.arguments === "string"
        ? (JSON.parse(msg.arguments) as Record<string, unknown>)
        : ((msg.arguments as Record<string, unknown>) ?? {});

    const call: ToolCallState = {
      id: msg.id,
      name: msg.name,
      arguments: args,
      status: "running",
    };

    // Find or create a tool group at the end of the feed
    const last = this.feed[this.feed.length - 1];
    if (last?.kind === "tool-group") {
      last.calls.push(call);
    } else {
      this.feed.push({
        id: nextFeedId(),
        kind: "tool-group",
        calls: [call],
      });
    }

    // Store the proxied reference from the $state feed so mutations
    // in handleToolResult go through Svelte's reactivity system
    const group = this.feed[this.feed.length - 1] as ToolGroupFeedItem;
    const lastCall = group.calls[group.calls.length - 1];
    if (lastCall) this.pendingToolCalls.set(msg.id, lastCall);
  }

  private handleToolResult(msg: Extract<ServerMessage, { type: "tool_result" }>): void {
    const call = this.pendingToolCalls.get(msg.tool_call_id);
    if (call) {
      call.status = msg.is_error ? "error" : "done";
      if (msg.output) {
        call.result =
          (call.result ? call.result + "\n" : "") +
          "\u2500\u2500\u2500 result \u2500\u2500\u2500\n" +
          msg.output;
      }
      this.pendingToolCalls.delete(msg.tool_call_id);
    }
  }
}
