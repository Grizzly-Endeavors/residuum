import { SvelteMap } from "svelte/reactivity";
import { failedAnnouncement, repliedAnnouncement, workingAnnouncement } from "./announce";
import { LiveStreams, MessageIds, nextAnimationFrame, type FrameScheduler } from "./feed-stream";
import { dayLabel } from "./day-label";
import { nextFeedId } from "./feed-id";
import { namedAgentMessage } from "./relay";
import {
  appendToolCall,
  applyToolResult,
  convertHistory,
  countByTurn,
  countTurnCalls,
  feedItemSignature,
  settlePendingCalls,
  type BackgroundTurnState,
} from "./feed-items";
import { ObservedTurns, type TurnEnding } from "./observed-turns.svelte";
import { localDay, localTimestamp, localTimestampAt } from "./time";
import type {
  ServerMessage,
  RecentMessage,
  FeedItem,
  UserFeedItem,
  DividerFeedItem,
  ToolCallState,
  MessageAttachment,
  FileAttachmentFeedItem,
  TurnFailureFeedItem,
  RecentHistorySegment,
  EpisodeHistorySegment,
  SessionUsageTotals,
  TurnInProgress,
  WaitingMessage,
} from "./types";

interface PendingHead {
  messages: RecentMessage[];
  index: number;
  recent: boolean;
}

const RECONCILE_ANCHOR_ITEMS = 3;

function afterSignedPrefix(fresh: (string | null)[], shown: string[]): number {
  let matched = 0;
  for (let i = 0; i < fresh.length; i++) {
    if (matched === shown.length) return i;
    const sig = fresh[i];
    if (sig === null || sig === undefined) continue;
    if (sig !== shown[matched]) return -1;
    matched++;
  }
  return matched === shown.length ? fresh.length : -1;
}

function afterLastRun(fresh: (string | null)[], anchor: string[]): number {
  if (anchor.length === 0) return 0;
  for (let end = fresh.length - 1; end >= 0; end--) {
    if (fresh[end] == null) continue;
    let k = anchor.length - 1;
    for (let i = end; i >= 0 && k >= 0; i--) {
      const sig = fresh[i];
      if (sig == null) continue;
      if (sig !== anchor[k]) break;
      k--;
    }
    if (k < 0) return end + 1;
  }
  return -1;
}

function recordedMessages(messages: RecentMessage[], turnId: string): number {
  return messages.filter((msg) => msg.turn_id === turnId).length;
}

type UserMessageFrame = Extract<ServerMessage, { type: "user_message" }>;
type QueuedMessageFrame = Extract<ServerMessage, { type: "message_queued" }>["message"];

function redrawnBySnapshot(item: FeedItem, echoed: readonly string[]): boolean {
  if (
    item.kind === "assistant" ||
    item.kind === "thinking" ||
    item.kind === "tool-group" ||
    item.kind === "file-attachment"
  ) {
    return true;
  }
  return (
    (item.kind === "user" || item.kind === "agent-message") &&
    item.messageId !== undefined &&
    echoed.includes(item.messageId)
  );
}

function deliveredElsewhere(endpoint: string): string | undefined {
  return endpoint === "" || endpoint === "ws" || endpoint === "background" ? undefined : endpoint;
}

function dayDivider(iso: string): DividerFeedItem {
  const date = localDay(iso);
  return { id: nextFeedId(), kind: "divider", variant: "day", label: dayLabel(date), date };
}

function dayDividerTracker(): (iso: string) => DividerFeedItem | null {
  let lastKey: string | null = null;
  return (iso) => {
    const key = localDay(iso);
    const crossed = lastKey !== null && key !== lastKey;
    lastKey = key;
    return crossed ? dayDivider(iso) : null;
  };
}

export class FeedStore {
  feed = $state<FeedItem[]>([]);
  isProcessing = $state(false);
  activeTurnId = $state<string | null>(null);
  readonly observed = new ObservedTurns();
  private sentTurnId: string | null = null;
  private settledTurnId: string | null = null;
  private historyTurnCounts: ReadonlyMap<string, number> = countByTurn([]);
  private liveBaseline: { turnId: string; recorded: number } | null = null;
  waiting = $state<WaitingMessage[]>([]);
  private connection = 0;
  turnOutputTokens = $state(0);
  turnHasUsage = $state(false);
  turnToolCalls = $state(0);
  sessionUsage = $state<SessionUsageTotals | null>(null);
  usageProblem = $state<string | null>(null);
  memoryWorking = $state(false);
  subconsciousWorking = $state(false);

  clearPostTurnActivity(): void {
    this.memoryWorking = false;
    this.subconsciousWorking = false;
  }
  oldestEpisodeCursor = $state<string | null>(null);
  hasMoreHistory = $state(false);
  isLoadingOlder = $state(false);

  generation = $state(0);
  historyLoaded = $state(false);

  private pendingToolCalls = new SvelteMap<string, ToolCallState>();
  private lastLiveDayKey: string | null = null;
  private compressedMarkerInserted = false;
  private pendingHeads: PendingHead[] = [];
  private recentStart = 0;
  private recentCursor: string | null = null;
  private recentCarriedTurn: BackgroundTurnState | undefined;
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- bookkeeping only, never rendered
  private loadedEpisodes = new Set<string>();
  private turnStart: number | null = null;
  private endedLiveTurns = 0;

  get turnsEndedLive(): number {
    return this.endedLiveTurns;
  }

  announcement = $state<{ id: number; text: string } | null>(null);
  private announced = 0;
  private readonly streams: LiveStreams;
  private readonly sentIds = new MessageIds();
  private replayClock: number | null = null;

  constructor(
    private readonly joinedTurnStart: () => number | null = () => null,
    private readonly agentName: () => string = () => "The agent",
    schedule: FrameScheduler = nextAnimationFrame,
  ) {
    this.streams = new LiveStreams(
      this.feed,
      schedule,
      () => localTimestampAt(this.now()),
      () => this.now(),
    );
  }

  private now(): number {
    return this.replayClock ?? Date.now();
  }

  private announce(text: string): void {
    if (this.replayClock !== null) return;
    this.announcement = { id: ++this.announced, text };
  }

  handleMessage(msg: ServerMessage): void {
    if (this.isOfSettledTurn(msg)) return;
    switch (msg.type) {
      case "turn_started":
        this.sentTurnId = null;
        this.isProcessing = true;
        this.activeTurnId = msg.reply_to;
        this.announce(workingAnnouncement(this.agentName()));
        this.captureBaseline(msg.reply_to);
        if (this.removeFailures(msg.reply_to)) this.closeToolGroup(msg.reply_to);
        this.turnStart ??= this.feed.length;
        for (const item of this.feed.slice(this.turnStart)) item.turnId = msg.reply_to;
        this.observed.start(msg.reply_to, this.now());
        this.turnOutputTokens = 0;
        this.turnHasUsage = false;
        this.turnToolCalls = 0;
        break;

      case "turn_ended":
        this.isProcessing = false;
        this.tagTurnStart(msg.reply_to);
        this.closeTurn(msg.reply_to);
        this.activeTurnId = null;
        this.turnStart = null;
        this.liveBaseline = null;
        this.endedLiveTurns++;
        break;

      case "turn_usage":
        this.joinTurn(msg.reply_to);
        this.turnOutputTokens = msg.output_tokens;
        this.turnHasUsage = msg.has_usage;
        this.turnToolCalls = msg.tool_calls;
        if (msg.session_totals) this.setSessionUsage(msg.session_totals);
        break;

      case "post_turn_activity":
        if (msg.kind === "memory") this.memoryWorking = msg.active;
        else this.subconsciousWorking = msg.active;
        break;

      case "tool_call":
        this.joinTurn(msg.reply_to);
        this.attempted(msg.reply_to);
        this.streams.endThought(msg.reply_to, msg.call);
        appendToolCall(
          this.feed,
          this.pendingToolCalls,
          msg,
          this.activeTurnId ?? undefined,
          msg.call,
          this.now(),
        );
        break;

      case "tool_result":
        this.joinTurn(msg.reply_to);
        applyToolResult(this.pendingToolCalls, msg, this.now());
        break;

      case "turn_snapshot":
        this.applyTurnSnapshot(msg.turn);
        this.reconcileWaiting(msg.queued);
        break;

      case "message_queued":
        this.addWaiting(msg.message);
        break;

      case "message_withdrawn":
        this.waiting = this.waiting.filter((message) => message.id !== msg.id);
        break;

      case "queued_message_unavailable": {
        const message = this.waiting.find((waiting) => waiting.id === msg.id);
        if (message !== undefined) delete message.asked;
        break;
      }

      case "response": {
        const standalone = msg.reply_to === "";
        if (!standalone) {
          this.isProcessing = false;
          this.attempted(msg.reply_to);
          if (msg.call !== undefined) this.streams.endThought(msg.reply_to, msg.call);
        }
        if (msg.content) {
          this.streams.completeText(
            msg.reply_to,
            standalone ? undefined : msg.call,
            msg.content,
            standalone ? {} : this.ofLiveTurn(),
            deliveredElsewhere(msg.endpoint),
          );
          this.announce(repliedAnnouncement(this.agentName(), msg.content));
        }
        break;
      }

      case "broadcast_response":
        this.joinTurn(msg.reply_to);
        this.attempted(msg.reply_to);
        this.streams.endThought(msg.reply_to, msg.call);
        this.streams.completeText(msg.reply_to, msg.call, msg.content, this.ofLiveTurn());
        break;

      case "text_delta":
        this.joinTurn(msg.reply_to);
        this.attempted(msg.reply_to);
        this.streams.endThought(msg.reply_to, msg.call);
        this.streams.appendText(msg.reply_to, msg.call, msg.text, this.ofLiveTurn());
        break;

      case "thinking_delta":
        this.joinTurn(msg.reply_to);
        this.attempted(msg.reply_to);
        this.streams.appendThought(msg.reply_to, msg.call, msg.text, this.ofLiveTurn());
        break;

      case "thinking":
        this.joinTurn(msg.reply_to);
        this.streams.completeThought(msg.reply_to, msg.call, msg.content, this.ofLiveTurn());
        break;

      case "stream_restart":
        this.joinTurn(msg.reply_to);
        this.streams.restart(msg.reply_to, msg.call);
        this.observed.retry(msg.reply_to, true);
        break;

      case "user_message":
        this.receiveUserMessage(msg);
        break;

      case "error":
        this.isProcessing = false;
        if (msg.reply_to !== null) {
          this.recordFailure(msg.reply_to, msg.message, msg.details);
          this.announce(failedAnnouncement(this.agentName()));
        }
        break;

      case "message_refused":
        this.withdrawRefused(msg.id);
        break;

      case "retry_refused":
        this.refuseRetry(msg.id, msg.message);
        break;

      case "notice":
      case "reloading":
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
          timestamp: localTimestampAt(this.now()),
          ...this.ofLiveTurn(),
        };
        this.feed.push(item);
        if (this.activeTurnId === null) {
          this.isProcessing = false;
        }
        break;
      }

      case "pong":
        break;

      case "session_started":
      case "session_state_changed":
      case "session_completed":
      case "session_memory_merged":
      case "session_turn_started":
      case "session_turn_ended":
      case "session_turn_usage":
      case "session_tool_call":
      case "session_tool_result":
      case "session_broadcast_response":
      case "session_response":
      case "session_outbound_a2a_task":
      case "session_start_failed":
      case "session_error":
      case "session_user_message":
      case "session_message_delivered":
      case "session_stop_requested":
      case "session_command_failed":
        break;

      case "pulse_fire_changed":
        break;

      case "artifact_updated":
      case "artifact_removed":
        break;

      case "workspace_changed":
      case "workspace_resync":
      case "workspace_watch_unavailable":
        break;
    }
  }

  loadHistory(segment: RecentHistorySegment): void {
    this.feed.length = 0;
    this.pendingToolCalls.clear();
    this.lastLiveDayKey = null;
    this.compressedMarkerInserted = false;
    this.loadedEpisodes.clear();
    this.oldestEpisodeCursor = segment.next_cursor;
    this.hasMoreHistory = segment.next_cursor !== null;
    this.recentStart = 0;
    this.turnStart = null;
    this.observed.clear(this.activeTurnId);
    this.generation++;
    this.recentCursor = segment.next_cursor;
    this.countHistoryTurns(segment.messages);
    this.recentCarriedTurn = segment.next_cursor === null ? "hidden" : undefined;
    this.historyLoaded = true;

    const conversion = convertHistory(segment.messages, {
      mode: "main",
      dayDivider: (ts) => this.dayDividerFor(ts),
      carriedTurn: this.recentCarriedTurn,
    });
    this.pendingHeads = conversion.undecidedHead.length
      ? [{ messages: conversion.undecidedHead, index: 0, recent: true }]
      : [];
    for (const item of conversion.items) this.feed.push(item);
  }

  reconcileRecent(segment: RecentHistorySegment): boolean {
    if (!this.historyLoaded || segment.next_cursor !== this.recentCursor) return false;
    this.countHistoryTurns(segment.messages);
    const fresh = convertHistory(segment.messages, {
      mode: "main",
      carriedTurn: this.recentCarriedTurn,
      dayDivider: dayDividerTracker(),
    }).items;
    const liveStart = this.turnStart ?? this.feed.length;
    const shown: string[] = [];
    let trailingToolGroups = 0;
    for (const item of this.feed.slice(this.recentStart, liveStart)) {
      const sig = feedItemSignature(item);
      if (sig !== null) {
        shown.push(sig);
        trailingToolGroups = 0;
      } else if (item.kind === "tool-group") {
        trailingToolGroups++;
      }
    }
    const freshSigs = fresh.map(feedItemSignature);
    const freshSigned = freshSigs.filter((sig): sig is string => sig !== null);
    if (
      freshSigned.length > 0 &&
      freshSigned.length < shown.length &&
      afterSignedPrefix(shown, freshSigned) >= 0
    ) {
      return true;
    }
    let appendFrom = afterSignedPrefix(freshSigs, shown);
    if (appendFrom < 0) appendFrom = afterLastRun(freshSigs, shown.slice(-RECONCILE_ANCHOR_ITEMS));
    if (appendFrom < 0) return false;
    while (trailingToolGroups > 0) {
      const next = fresh[appendFrom]?.kind;
      if (next !== "tool-group" && next !== "thinking") break;
      appendFrom++;
      if (next === "tool-group") trailingToolGroups--;
    }
    this.placeRecorded(fresh.slice(appendFrom), liveStart, segment.messages);
    this.syncDayKey(segment);
    return true;
  }

  reloadHistory(segment: RecentHistorySegment): void {
    const liveStart = this.turnStart;
    const live = liveStart === null ? [] : this.feed.slice(liveStart);
    const pendingTools = [...this.pendingToolCalls];
    const turnId = this.liveTurnId();
    this.loadHistory(segment);
    if (liveStart === null) return;
    if (turnId !== null && this.historyRecords(segment.messages, turnId)) {
      this.settleFromHistory(turnId);
      return;
    }
    this.turnStart = this.feed.length;
    for (const item of live) this.feed.push(item);
    for (const [id, call] of pendingTools) this.pendingToolCalls.set(id, call);
  }

  prependEpisode(segment: EpisodeHistorySegment): void {
    if (this.loadedEpisodes.has(segment.episode_id)) return;
    this.loadedEpisodes.add(segment.episode_id);

    const conversion = convertHistory(segment.messages, { mode: "main", timestamps: false });
    if (conversion.endTurn !== "unknown") this.resolvePendingHeads(conversion.endTurn);

    const block: FeedItem[] = [
      {
        id: nextFeedId(),
        kind: "divider",
        variant: "episode",
        label: dayLabel(segment.date),
        date: segment.date,
        episode: segment.episode_id,
      } satisfies DividerFeedItem,
      ...conversion.items,
    ];

    if (!this.compressedMarkerInserted) {
      block.push({ id: nextFeedId(), kind: "compressed-marker" });
      this.compressedMarkerInserted = true;
    }

    this.feed.splice(0, 0, ...block);
    this.recentStart += block.length;
    if (this.turnStart !== null) this.turnStart += block.length;
    for (const head of this.pendingHeads) head.index += block.length;
    if (conversion.undecidedHead.length) {
      this.pendingHeads.push({ messages: conversion.undecidedHead, index: 1, recent: false });
    }
    this.oldestEpisodeCursor = segment.next_cursor;
    this.hasMoreHistory = segment.next_cursor !== null;
  }

  pushLocalSystem(content: string): void {
    this.feed.push({ id: nextFeedId(), kind: "local-system", content });
  }

  setSessionUsage(totals: SessionUsageTotals): void {
    this.sessionUsage = totals;
    this.usageProblem = null;
  }

  pushUserMessage(
    content: string,
    attachments?: MessageAttachment[],
    id?: string,
    delivered = true,
  ): void {
    const files = attachments === undefined || attachments.length === 0 ? {} : { attachments };
    if (id !== undefined && this.activeTurnId !== null) {
      this.sentIds.add(id);
      this.waiting.push({
        id,
        content,
        ...files,
        endpoint: "ws",
        connection: delivered ? this.connection : this.connection + 1,
      });
      return;
    }
    const timestamp = localTimestamp();
    this.maybePushDayDivider(timestamp);
    if (id !== undefined) this.sentIds.add(id);
    if (this.activeTurnId === null && id !== undefined) {
      this.sentTurnId ??= id;
      this.captureBaseline(id);
    }
    this.turnStart ??= this.feed.length;
    this.feed.push({
      id: nextFeedId(),
      kind: "user",
      content,
      ...files,
      timestamp,
      ...(id === undefined ? {} : { messageId: id }),
      ...this.reachedLiveTurn(),
    });
    this.isProcessing = true;
  }

  withdrawRefused(id: string): void {
    this.sentIds.delete(id);
    if (this.waiting.some((message) => message.id === id)) {
      this.waiting = this.waiting.filter((message) => message.id !== id);
      return;
    }
    const index = this.feed.findLastIndex((item) => item.kind === "user" && item.messageId === id);
    if (index !== -1) this.feed.splice(index, 1);
    if (this.activeTurnId === null && this.sentTurnId === id) {
      if (this.turnStart !== null && this.turnStart >= this.feed.length) this.turnStart = null;
      this.sentTurnId = null;
      this.liveBaseline = null;
      this.isProcessing = false;
    }
  }

  abandonLiveTurn(): void {
    if (this.activeTurnId !== null) this.closeTurn(this.activeTurnId, "interrupted");
    this.endLiveTurn();
    this.waiting = [];
  }

  connected(): void {
    this.connection++;
  }

  forgetAsks(): void {
    for (const message of this.waiting) delete message.asked;
    for (const item of this.feed) {
      if (item.kind === "turn-failure" && item.retry !== undefined) delete item.retry.asked;
    }
  }

  askRetry(turnId: string): void {
    const failure = this.failureOf(turnId);
    if (failure?.retry !== undefined) failure.retry.asked = true;
  }

  showsFailure(turnId: string): boolean {
    return this.failureOf(turnId) !== undefined;
  }

  askOfWaiting(id: string, asked: NonNullable<WaitingMessage["asked"]>): void {
    const message = this.waiting.find((waiting) => waiting.id === id);
    if (message !== undefined) message.asked = asked;
  }

  askStop(): void {
    if (this.activeTurnId !== null) this.observed.askStop(this.activeTurnId);
  }

  markReconnectGap(): void {
    this.settledTurnId = null;
    const turnId = this.activeTurnId;
    if (turnId === null) return;
    this.observed.gap(turnId, countTurnCalls(this.feed.slice(this.turnStart ?? 0), turnId));
  }

  private joinTurn(turnId: string): void {
    if (this.activeTurnId !== null) return;
    if (this.observed.get(turnId)?.endedAt != null) return;
    this.activeTurnId = this.observed.join(turnId, this.joinedTurnStart());
    this.captureBaseline(turnId);
    this.turnStart = this.feed.length;
    this.isProcessing = true;
  }

  private attempted(turnId: string): void {
    this.observed.retry(turnId, false);
  }

  private receiveUserMessage(msg: UserMessageFrame): void {
    const waited = this.waiting.some((message) => message.id === msg.id);
    if (waited) this.waiting = this.waiting.filter((message) => message.id !== msg.id);
    if (!waited && this.replayClock === null && this.sentIds.has(msg.id)) {
      this.confirmSentAttachments(msg);
      return;
    }
    const joins = this.activeTurnId === msg.turn_id;
    const timestamp = localTimestampAt(this.now());
    if (!joins) {
      this.maybePushDayDivider(timestamp);
      this.turnStart ??= this.feed.length;
    }
    if (msg.agent_sender !== undefined) {
      const { from, category, body } = namedAgentMessage(msg.agent_sender, msg.content);
      this.feed.push({
        id: nextFeedId(),
        kind: "agent-message",
        from,
        category,
        content: body,
        runId: msg.agent_sender.run_id ?? null,
        timestamp,
        messageId: msg.id,
        turnId: msg.turn_id,
        ...(joins ? { midTurn: true } : {}),
      });
      return;
    }
    this.feed.push({
      id: nextFeedId(),
      kind: "user",
      content: msg.content,
      timestamp,
      messageId: msg.id,
      ...(msg.images === undefined || msg.images.length === 0 ? {} : { images: msg.images }),
      ...(msg.attachments === undefined || msg.attachments.length === 0
        ? {}
        : { attachments: msg.attachments }),
      ...(msg.sender === undefined ? {} : { sender: msg.sender }),
      turnId: msg.turn_id,
      ...(joins ? { midTurn: true } : {}),
    });
  }

  private addWaiting(message: QueuedMessageFrame): void {
    if (this.waiting.some((waiting) => waiting.id === message.id)) return;
    this.waiting.push({
      id: message.id,
      content: message.content,
      ...(message.images === undefined || message.images.length === 0
        ? {}
        : { images: message.images }),
      ...(message.attachments === undefined || message.attachments.length === 0
        ? {}
        : { attachments: message.attachments }),
      ...(message.sender === undefined ? {} : { sender: message.sender }),
      endpoint: message.endpoint,
      connection: this.connection,
    });
  }

  private reconcileWaiting(queued: readonly QueuedMessageFrame[]): void {
    const listed = queued.map((message) => message.id);
    this.waiting = this.waiting.filter(
      (message) => listed.includes(message.id) || message.connection >= this.connection,
    );
    for (const message of queued) this.addWaiting(message);
  }

  private recordFailure(turnId: string, message: string, details: string | undefined): void {
    this.streams.finish(turnId, "interrupted");
    const asked = this.feed.findLast(
      (item): item is UserFeedItem =>
        item.kind === "user" &&
        (item.turnId === turnId || item.messageId === turnId) &&
        item.midTurn !== true &&
        item.sender === undefined,
    );
    this.feed.push({
      id: nextFeedId(),
      kind: "turn-failure",
      turnId,
      message,
      ...(details === undefined || details === "" ? {} : { details }),
      ...(asked === undefined ? {} : { retry: {} }),
    });
  }

  private failureOf(turnId: string): TurnFailureFeedItem | undefined {
    return this.feed.findLast(
      (item): item is TurnFailureFeedItem => item.kind === "turn-failure" && item.turnId === turnId,
    );
  }

  private removeFailures(turnId: string): boolean {
    let removed = false;
    for (let at = this.feed.length - 1; at >= 0; at--) {
      const item = this.feed[at];
      if (item?.kind === "turn-failure" && item.turnId === turnId) {
        this.feed.splice(at, 1);
        removed = true;
      }
    }
    return removed;
  }

  private closeToolGroup(turnId: string): void {
    const last = this.feed.at(-1);
    if (last?.kind === "tool-group" && last.turnId === turnId) last.closed = true;
  }

  private refuseRetry(turnId: string, reason: string): void {
    const failure = this.failureOf(turnId);
    if (failure !== undefined) failure.retry = { refused: reason };
  }

  private confirmSentAttachments(msg: UserMessageFrame): void {
    const hasFiles = msg.attachments !== undefined && msg.attachments.length > 0;
    const hasImages = msg.images !== undefined && msg.images.length > 0;
    if (!hasFiles && !hasImages) return;
    const shown = this.feed.findLast(
      (item): item is UserFeedItem => item.kind === "user" && item.messageId === msg.id,
    );
    if (shown === undefined) return;
    if (hasFiles) shown.attachments = msg.attachments;
    if (hasImages) shown.images = msg.images;
  }

  private closeTurn(turnId: string, ending?: TurnEnding): void {
    const how = this.observed.end(turnId, ending);
    this.streams.finish(turnId, how);
    settlePendingCalls(this.pendingToolCalls, how === "finished" ? "done" : "stopped");
  }

  private ofLiveTurn(): { turnId?: string } {
    return this.activeTurnId === null ? {} : { turnId: this.activeTurnId };
  }

  private reachedLiveTurn(): { turnId?: string; midTurn?: true } {
    return this.activeTurnId === null ? {} : { turnId: this.activeTurnId, midTurn: true };
  }

  private captureBaseline(turnId: string): void {
    if (this.liveBaseline?.turnId === turnId) return;
    this.liveBaseline = { turnId, recorded: this.historyTurnCounts.get(turnId) ?? 0 };
  }

  private historyRecords(messages: RecentMessage[], turnId: string): boolean {
    const before = this.liveBaseline?.turnId === turnId ? this.liveBaseline.recorded : 0;
    return recordedMessages(messages, turnId) > before;
  }

  private countHistoryTurns(messages: RecentMessage[]): void {
    this.historyTurnCounts = countByTurn(messages);
  }

  private tagTurnStart(turnId: string): void {
    if (this.turnStart === null) return;
    const atStart = this.feed[this.turnStart];
    const item =
      atStart?.kind === "user"
        ? atStart
        : this.feed.findLast(
            (candidate): candidate is UserFeedItem =>
              candidate.kind === "user" &&
              (candidate.turnId === turnId || candidate.messageId === turnId) &&
              candidate.midTurn !== true,
          );
    if (item !== undefined) item.turn = { turnId, changed: null };
  }

  private resolvePendingHeads(turn: "shown" | "hidden"): void {
    const heads = [...this.pendingHeads].sort((a, b) => b.index - a.index);
    this.pendingHeads = [];
    for (const head of heads) {
      if (head.recent) this.recentCarriedTurn = turn;
      if (turn === "hidden") continue;
      const items = convertHistory(head.messages, {
        mode: "main",
        carriedTurn: "shown",
        timestamps: head.recent,
      }).items;
      this.feed.splice(head.index, 0, ...items);
      if (head.index < this.recentStart) this.recentStart += items.length;
      if (this.turnStart !== null && head.index <= this.turnStart) {
        this.turnStart += items.length;
      }
    }
  }

  private placeRecorded(
    recorded: FeedItem[],
    liveStart: number,
    historyMessages: RecentMessage[],
  ): void {
    const turnId = this.liveTurnId();
    if (
      this.turnStart !== null &&
      turnId !== null &&
      this.historyRecords(historyMessages, turnId)
    ) {
      this.feed.splice(liveStart, this.feed.length - liveStart, ...recorded);
      this.settleFromHistory(turnId);
      return;
    }
    this.feed.splice(liveStart, 0, ...recorded);
    if (this.turnStart !== null) this.turnStart += recorded.length;
  }

  private applyTurnSnapshot(turn: TurnInProgress | null): void {
    if (turn === null || turn.reply_to === this.settledTurnId) return;
    const echoed = turn.frames.flatMap(({ frame }) =>
      frame.type === "user_message" ? [frame.id] : [],
    );
    const stopAsked = this.observed.get(turn.reply_to)?.stopAsked === true;
    const sentTurnId = this.sentTurnId;
    const kept = this.dropLiveTurn(echoed);
    const serverNow = Date.parse(turn.now);
    const skew = Number.isNaN(serverNow) ? 0 : Date.now() - serverNow;
    const onPageClock = (at: string): number => {
      const sent = Date.parse(at);
      return Number.isNaN(sent) ? Date.now() : sent + skew;
    };
    try {
      for (const { at, frame } of turn.frames) {
        this.replayClock = onPageClock(at);
        this.handleMessage(frame);
      }
    } finally {
      this.replayClock = null;
    }
    this.streams.flush();
    if (this.observed.get(turn.reply_to) === undefined) {
      this.observed.start(turn.reply_to, onPageClock(turn.started_at));
    }
    if (stopAsked) this.observed.askStop(turn.reply_to);
    if (sentTurnId !== null && !echoed.includes(sentTurnId)) this.sentTurnId = sentTurnId;
    for (const item of kept) this.feed.push(item);
  }

  private dropLiveTurn(echoed: readonly string[]): FeedItem[] {
    const start = this.turnStart;
    const dropped = this.activeTurnId;
    this.endLiveTurn();
    if (dropped !== null) this.observed.forget(dropped);
    if (start === null) return [];
    return this.feed.splice(start).filter((item) => !redrawnBySnapshot(item, echoed));
  }

  private endLiveTurn(): void {
    this.turnStart = null;
    this.isProcessing = false;
    this.activeTurnId = null;
    this.sentTurnId = null;
    this.liveBaseline = null;
    this.streams.clear();
    this.pendingToolCalls.clear();
  }

  private liveTurnId(): string | null {
    return this.activeTurnId ?? this.sentTurnId;
  }

  private settleFromHistory(turnId: string): void {
    this.endLiveTurn();
    this.observed.forget(turnId);
    this.settledTurnId = turnId;
  }

  private isOfSettledTurn(msg: ServerMessage): boolean {
    const settled = this.settledTurnId;
    if (settled === null) return false;
    if (msg.type === "turn_started") {
      if (msg.reply_to === settled) return true;
      this.settledTurnId = null;
      return false;
    }
    if (msg.type === "turn_ended") {
      if (msg.reply_to !== settled) return false;
      this.settledTurnId = null;
      return true;
    }
    if (msg.type === "user_message") return msg.turn_id === settled;
    return (
      (msg.type === "response" ||
        msg.type === "turn_usage" ||
        msg.type === "file_attachment" ||
        msg.type === "tool_call" ||
        msg.type === "tool_result" ||
        msg.type === "broadcast_response" ||
        msg.type === "text_delta" ||
        msg.type === "thinking_delta" ||
        msg.type === "thinking" ||
        msg.type === "stream_restart") &&
      msg.reply_to === settled
    );
  }

  private syncDayKey(segment: RecentHistorySegment): void {
    const last = segment.messages[segment.messages.length - 1];
    if (last?.timestamp) this.lastLiveDayKey = localDay(last.timestamp);
  }

  private dayDividerFor(iso: string): DividerFeedItem | null {
    const key = localDay(iso);
    const crossed = this.lastLiveDayKey !== null && key !== this.lastLiveDayKey;
    this.lastLiveDayKey = key;
    return crossed ? dayDivider(iso) : null;
  }

  private maybePushDayDivider(iso: string): void {
    const divider = this.dayDividerFor(iso);
    if (divider) this.feed.push(divider);
  }
}
