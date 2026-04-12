// ── Feed store (Svelte 5 runes) ──────────────────────────────────────

import { SvelteMap } from "svelte/reactivity";
import { nextFeedId } from "./feed-id";
import type {
  ServerMessage,
  RecentMessage,
  FeedItem,
  DividerFeedItem,
  ToolGroupFeedItem,
  ToolCallState,
  ImageAttachment,
  FileAttachmentFeedItem,
  RecentHistorySegment,
  EpisodeHistorySegment,
} from "./types";

const DAY_DIVIDER_FORMATTER = new Intl.DateTimeFormat(undefined, {
  month: "long",
  day: "numeric",
});

function dayKey(iso: string): string {
  // Timestamps are either "YYYY-MM-DD" or "YYYY-MM-DDTHH:MM". Slice to date.
  return iso.slice(0, 10);
}

function dayLabel(iso: string): string {
  const date = new Date(iso.length === 10 ? `${iso}T00:00` : iso);
  if (Number.isNaN(date.getTime())) return iso.slice(0, 10);
  return DAY_DIVIDER_FORMATTER.format(date);
}

/** Manages the chat feed state and processes incoming server messages. */
export class FeedStore {
  feed = $state<FeedItem[]>([]);
  isProcessing = $state(false);
  oldestEpisodeCursor = $state<string | null>(null);
  hasMoreHistory = $state(false);
  isLoadingOlder = $state(false);

  private pendingToolCalls = new SvelteMap<string, ToolCallState>();
  private lastLiveDayKey: string | null = null;
  private compressedMarkerInserted = false;

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

  /** Populate the feed from a Recent history segment. */
  loadHistory(segment: RecentHistorySegment): void {
    this.feed.length = 0;
    this.pendingToolCalls.clear();
    this.lastLiveDayKey = null;
    this.compressedMarkerInserted = false;
    this.oldestEpisodeCursor = segment.next_cursor;
    this.hasMoreHistory = segment.next_cursor !== null;

    const items = this.convertMessages(segment.messages, {
      withDayDividers: true,
    });
    for (const item of items) this.feed.push(item);
  }

  /**
   * Prepend an episode segment to the top of the feed.
   *
   * Inserts an `ep-NNN · date` divider above the episode's messages. On the
   * first prepend, also inserts a `compressed-marker` between the episode
   * block and the already-present live messages so the user sees where
   * the observer cut.
   */
  prependEpisode(segment: EpisodeHistorySegment): void {
    const block: FeedItem[] = [
      {
        id: nextFeedId(),
        kind: "divider",
        variant: "episode",
        label: `${segment.episode_id} · ${segment.date}`,
      } satisfies DividerFeedItem,
      ...this.convertMessages(segment.messages, { withDayDividers: false }),
    ];

    if (!this.compressedMarkerInserted) {
      block.push({ id: nextFeedId(), kind: "compressed-marker" });
      this.compressedMarkerInserted = true;
    }

    this.feed.splice(0, 0, ...block);
    this.oldestEpisodeCursor = segment.next_cursor;
    this.hasMoreHistory = segment.next_cursor !== null;
  }

  /** Add a user message to the feed. */
  pushUserMessage(content: string, images?: ImageAttachment[]): void {
    // Live user messages carry an implicit "now" timestamp — inject a day
    // divider if the calendar day has rolled over since the last live entry.
    // eslint-disable-next-line svelte/prefer-svelte-reactivity
    const nowIso = new Date(Date.now()).toISOString();
    this.maybePushDayDivider(nowIso);
    this.feed.push({ id: nextFeedId(), kind: "user", content, images });
    this.isProcessing = true;
  }

  /** Append an arbitrary feed item (e.g. from commands). */
  appendFeedItem(item: FeedItem): void {
    this.feed.push(item);
  }

  // ── Private ──────────────────────────────────────────────────────────

  /**
   * Convert a RecentMessage list into feed items using the same
   * role-based logic as `loadHistory` used to inline. Optionally emit
   * day dividers when the per-message timestamp crosses a day boundary.
   */
  private convertMessages(
    messages: RecentMessage[],
    opts: { withDayDividers: boolean },
  ): FeedItem[] {
    const out: FeedItem[] = [];
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- non-reactive scratch
    const toolCallItems = new Map<string, ToolCallState>();

    for (const msg of messages.filter((m) => m.visibility !== "background")) {
      if (opts.withDayDividers && msg.timestamp) {
        const key = dayKey(msg.timestamp);
        if (this.lastLiveDayKey !== null && key !== this.lastLiveDayKey) {
          out.push({
            id: nextFeedId(),
            kind: "divider",
            variant: "day",
            label: dayLabel(msg.timestamp),
          } satisfies DividerFeedItem);
        }
        this.lastLiveDayKey = key;
      }

      const content = msg.content || "";
      switch (msg.role) {
        case "user":
          out.push({ id: nextFeedId(), kind: "user", content });
          break;
        case "assistant": {
          if (content.trim()) {
            out.push({ id: nextFeedId(), kind: "assistant", content });
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
            out.push({ id: nextFeedId(), kind: "tool-group", calls });
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

    return out;
  }

  private maybePushDayDivider(iso: string): void {
    const key = dayKey(iso);
    if (this.lastLiveDayKey !== null && key !== this.lastLiveDayKey) {
      this.feed.push({
        id: nextFeedId(),
        kind: "divider",
        variant: "day",
        label: dayLabel(iso),
      } satisfies DividerFeedItem);
    }
    this.lastLiveDayKey = key;
  }

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
