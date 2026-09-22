// ── Feed store (Svelte 5 runes) ──────────────────────────────────────

import { SvelteMap } from "svelte/reactivity";
import { nextFeedId } from "./feed-id";
import { appendToolCall, applyToolResult, convertHistoryMessages } from "./feed-items";
import type {
  ServerMessage,
  RecentMessage,
  FeedItem,
  DividerFeedItem,
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
  /**
   * Correlation id of the turn currently in flight, or `null` when idle.
   * Set on `turn_started`, cleared on `turn_ended` — the only frame the
   * protocol guarantees closes every turn — so a stop request always has
   * the right id to target even if `isProcessing` cleared earlier via a
   * `response`/`error` frame.
   */
  activeTurnId = $state<string | null>(null);
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
        this.activeTurnId = msg.reply_to;
        break;

      case "turn_ended":
        // Closes out turns that produce no response/error (e.g. zero-text
        // replies) — harmless no-op if isProcessing already cleared via
        // one of those paths.
        this.isProcessing = false;
        this.activeTurnId = null;
        break;

      case "tool_call":
        appendToolCall(this.feed, this.pendingToolCalls, msg);
        break;

      case "tool_result":
        applyToolResult(this.pendingToolCalls, msg);
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
        // Server error clears the thinking indicator. The notification
        // surface itself is dispatched by WsCoordinator before this runs.
        this.isProcessing = false;
        break;

      case "notice":
      case "reloading":
        // Surfaced by WsCoordinator; no chat-state side effect.
        break;

      case "inline_output":
        this.pushLocalSystem(msg.message);
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

      case "session_started":
      case "session_state_changed":
      case "session_completed":
      case "session_turn_started":
      case "session_turn_ended":
      case "session_tool_call":
      case "session_tool_result":
      case "session_broadcast_response":
      case "session_response":
      case "session_error":
      case "session_message_to_main":
      case "session_message_delivered":
      case "session_stop_requested":
      case "session_command_failed":
        // Agent session activity belongs to the sessions surface, never
        // the main chat feed.
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

    const items = this.convertMessages(segment.messages, { withDayDividers: true });
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

  /**
   * Push a client-only system message into the feed for inline rendering.
   * Used by slash commands like `/help` and `/status`, and by inbound
   * `inline_output` server messages (e.g. `/context` results). These items
   * never round-trip to history and vanish on reload.
   */
  pushLocalSystem(content: string): void {
    this.feed.push({ id: nextFeedId(), kind: "local-system", content });
  }

  /**
   * Add a message a session sent the main agent (its relayed result, or a
   * `message_agent` call), as it arrives live.
   */
  pushAgentMessage(from: string, runId: string, content: string, category: string | null): void {
    this.feed.push({ id: nextFeedId(), kind: "agent-message", from, category, content, runId });
  }

  /** Add a user message to the feed. */
  pushUserMessage(content: string, images?: ImageAttachment[]): void {
    // Live user messages carry an implicit "now" timestamp — inject a day
    // divider if the calendar day has rolled over since the last live entry.
    // eslint-disable-next-line svelte/prefer-svelte-reactivity
    const nowIso = new Date().toISOString();
    this.maybePushDayDivider(nowIso);
    this.feed.push({ id: nextFeedId(), kind: "user", content, images });
    this.isProcessing = true;
  }

  // ── Private ──────────────────────────────────────────────────────────

  /**
   * Convert main-agent history messages into feed items, optionally emitting
   * day dividers when the per-message timestamp crosses a day boundary.
   */
  private convertMessages(
    messages: RecentMessage[],
    opts: { withDayDividers: boolean },
  ): FeedItem[] {
    return convertHistoryMessages(messages, {
      mode: "main",
      dayDivider: opts.withDayDividers ? (ts) => this.dayDividerFor(ts) : undefined,
    });
  }

  private dayDividerFor(iso: string): DividerFeedItem | null {
    const key = dayKey(iso);
    const crossed = this.lastLiveDayKey !== null && key !== this.lastLiveDayKey;
    this.lastLiveDayKey = key;
    return crossed
      ? { id: nextFeedId(), kind: "divider", variant: "day", label: dayLabel(iso) }
      : null;
  }

  private maybePushDayDivider(iso: string): void {
    const divider = this.dayDividerFor(iso);
    if (divider) this.feed.push(divider);
  }
}
