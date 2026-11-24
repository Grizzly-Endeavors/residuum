import { untrack } from "svelte";
import { hub } from "./hub.svelte";
import { WsTransport } from "./transport.svelte";
import { agentWsUrl } from "./paths";
import { onViewedAgentChange } from "./viewed-agent";
import { scheduled } from "./scheduled.svelte";
import { FeedStore } from "./feed.svelte";
import { SessionsStore, isSessionFrame } from "./sessions.svelte";
import { notifications } from "./notifications.svelte";
import { noticeFrameNotice, reloadingNotice } from "./reload-notices";
import { invalidate } from "./cache";
import { newMessageId } from "./message-id";
import { restoreDraft } from "./composer-drafts";
import { userErrorMessage, userErrorReason } from "./errors";
import { normalizeWatchPrefix } from "./workspace-watch";
import { WatchRegistry } from "./watch-registry";
import {
  fetchChatHistory,
  fetchChatSegment,
  fetchUsageTotals,
  cacheKeyStatus,
  CACHE_KEY_TIMEZONE,
  CACHE_KEY_MCP_CATALOG,
  cacheKeyConfigRaw,
  cacheKeyProvidersRaw,
  cacheKeyMcpRaw,
} from "./api";
import type { StagedAttachment } from "./staged-attachments.svelte";
import type { ClientMessage, RecentHistorySegment, ServerMessage } from "./types";

export interface RefusedSend {
  content: string;
  attachments: StagedAttachment[];
}

class WsCoordinator {
  agent = $state<string | null>(null);
  transport = new WsTransport({
    url: () => agentWsUrl(this.agent ?? ""),
    keepalive: true,
  });
  store = $state<FeedStore>(this.createFeed(null));
  historyError = $state<string | null>(null);
  sessions = $state<SessionsStore>(new SessionsStore({ agent: null }));
  private hasConnected = false;
  private catchUpOnConnect = false;
  private frameListeners = new Set<(msg: ServerMessage, agent: string | null) => void>();
  private withdrawals = new Map<
    string,
    { resolve: (withdrawn: boolean) => void; reject: (err: Error) => void }
  >();
  readonly watches = new WatchRegistry({
    send: (prefixes) => {
      this.transport.send({ type: "watch_workspace", prefixes });
    },
    normalize: normalizeWatchPrefix,
    refusal: (prefix) =>
      `can't watch "${prefix}": watch paths are relative to the workspace, like "team/wiki", and can't contain ".."`,
  });
  private liveUpdatesOffShown = false;
  private unconfirmedSends = new Map<string, RefusedSend>();
  private refusalListeners = new Set<(refused: RefusedSend) => void>();

  constructor() {
    onViewedAgentChange((name) => {
      this.useAgent(name);
    });
    this.frameListeners.add((msg) => {
      this.watches.handleFrame(msg);
    });

    this.transport.onMessage = (msg) => {
      for (const listener of this.frameListeners) {
        try {
          listener(msg, this.agent);
        } catch (err) {
          // eslint-disable-next-line no-console -- a failing observer must not stop the chat from handling the frame; the console is its only channel
          console.error("frame listener failed", err);
        }
      }
      if (isSessionFrame(msg)) {
        this.sessions.handleFrame(msg);
        return;
      }
      this.settleWithdrawal(msg);
      this.confirmSend(msg);
      if (msg.type === "error") {
        notifications.surface("error", msg.message, msg.details ?? undefined);
      } else if (msg.type === "message_refused") {
        notifications.surface("error", msg.message);
        this.refuseSend(msg.id);
      } else if (msg.type === "retry_refused") {
        if (this.store.showsFailure(msg.id)) notifications.surface("error", msg.message);
      } else if (msg.type === "workspace_watch_unavailable") {
        if (!this.liveUpdatesOffShown) notifications.surface("error", msg.message);
        this.liveUpdatesOffShown = true;
      } else if (msg.type === "notice") {
        const notice = noticeFrameNotice(msg);
        if (notice !== null) notifications.surface(notice.kind, notice.message, notice.details);
      } else if (msg.type === "reloading") {
        const notice = reloadingNotice();
        if (notice !== null) notifications.surface(notice.kind, notice.message);
        invalidate(CACHE_KEY_TIMEZONE);
        invalidate(CACHE_KEY_MCP_CATALOG);
        if (this.agent !== null) {
          invalidate(cacheKeyStatus(this.agent));
          invalidate(cacheKeyConfigRaw(this.agent));
          invalidate(cacheKeyProvidersRaw(this.agent));
          invalidate(cacheKeyMcpRaw(this.agent));
        }
      }
      this.store.handleMessage(msg);
    };

    this.transport.onConnected = () => {
      this.store.connected();
      this.transport.send({ type: "set_verbose", enabled: true });
      this.transport.send({ type: "resync_turn" });
      if (this.hasConnected) this.store.markReconnectGap();
      this.sessions.resync();
      if (this.hasConnected || this.catchUpOnConnect) void this.reconcileMainHistory();
      this.hasConnected = true;
      this.catchUpOnConnect = false;
      void this.loadUsage();
      this.liveUpdatesOffShown = false;
      this.watches.connected();
    };

    this.transport.onDisconnected = () => {
      this.connectionClosed();
    };

    $effect.root(() => {
      $effect(() => {
        const wanted = this.connectionWanted();
        untrack(() => {
          this.followAgentState(wanted);
        });
      });
    });
  }

  private connectionWanted(): boolean {
    if (this.agent === null) return false;
    const state = hub.agent(this.agent)?.state;
    return state === undefined ? !hub.loaded : state === "running";
  }

  private followAgentState(wanted: boolean): void {
    if (this.agent === null) return;
    if (wanted) {
      if (this.transport.status !== "disconnected") return;
      this.catchUpOnConnect = this.store.historyLoaded;
      this.transport.reconnectNow();
      return;
    }
    const wasOpen = this.transport.status === "connected";
    this.transport.disconnect();
    if (wasOpen) this.connectionClosed();
    this.store.abandonLiveTurn();
  }

  private connectionClosed(): void {
    this.watches.disconnected();
    this.store.clearPostTurnActivity();
    this.store.forgetAsks();
    this.failWithdrawals("The connection to the agent dropped before it answered.");
  }

  private settleWithdrawal(msg: ServerMessage): void {
    if (
      msg.type !== "message_withdrawn" &&
      msg.type !== "queued_message_unavailable" &&
      msg.type !== "user_message"
    ) {
      return;
    }
    const pending = this.withdrawals.get(msg.id);
    if (pending === undefined) return;
    this.withdrawals.delete(msg.id);
    pending.resolve(msg.type === "message_withdrawn");
  }

  private confirmSend(msg: ServerMessage): void {
    if (this.unconfirmedSends.size === 0) return;
    if (msg.type === "user_message") this.unconfirmedSends.delete(msg.id);
    else if (msg.type === "message_queued") this.unconfirmedSends.delete(msg.message.id);
    else if (msg.type === "turn_started") this.unconfirmedSends.delete(msg.reply_to);
    else if (msg.type === "turn_snapshot") {
      for (const queued of msg.queued) this.unconfirmedSends.delete(queued.id);
      if (msg.turn !== null) this.unconfirmedSends.delete(msg.turn.reply_to);
    }
  }

  private refuseSend(id: string): void {
    const refused = this.unconfirmedSends.get(id);
    if (refused === undefined) return;
    this.unconfirmedSends.delete(id);
    for (const attachment of refused.attachments) void attachment.uploadAgain();
    if (this.refusalListeners.size > 0) {
      for (const listener of this.refusalListeners) listener(refused);
    } else if (this.agent !== null) {
      restoreDraft(this.agent, refused.content, refused.attachments);
    }
  }

  private failWithdrawals(reason: string): void {
    const pending = [...this.withdrawals.values()];
    this.withdrawals.clear();
    for (const { reject } of pending) reject(new Error(reason));
  }

  private createFeed(agent: string | null): FeedStore {
    return new FeedStore(
      () => {
        if (agent === null) return null;
        const since = hub.activityOf(agent).busy_since;
        const at = since === null ? Number.NaN : Date.parse(since);
        return Number.isNaN(at) ? null : at;
      },
      () => (agent === null ? "The agent" : hub.shownName(agent)),
    );
  }

  useAgent(name: string | null): void {
    this.failWithdrawals("The chat moved to another agent before it answered.");
    if (name === this.agent) return;
    this.transport.reset();
    const store = this.createFeed(name);
    this.store = store;
    this.historyError = null;
    this.sessions = new SessionsStore({ agent: name });
    this.hasConnected = false;
    this.catchUpOnConnect = false;
    this.liveUpdatesOffShown = false;
    this.unconfirmedSends.clear();
    this.watches.bind(name);
    scheduled.reset(name);
    this.agent = name;
    if (name === null) return;
    if (this.connectionWanted()) this.transport.connect();
    void this.loadMainHistory();
    void this.loadUsage();
  }

  async loadMainHistory(): Promise<void> {
    const agent = this.agent;
    if (agent === null) return;
    const store = this.store;
    this.historyError = null;
    let recent;
    try {
      recent = await this.fetchRecentHistory(agent, store);
    } catch (err) {
      if (store !== this.store) return;
      this.historyError = userErrorReason(err, { action: "Couldn't load the chat history." });
      return;
    }
    if (store !== this.store) return;
    store.reloadHistory(recent);
    await this.loadOlderHistory();
  }

  private async fetchRecentHistory(agent: string, store: FeedStore): Promise<RecentHistorySegment> {
    let ended;
    let recent;
    do {
      ended = store.turnsEndedLive;
      recent = await fetchChatHistory(agent);
    } while (store === this.store && store.turnsEndedLive !== ended);
    return recent;
  }

  async loadOlderHistory(): Promise<boolean> {
    const agent = this.agent;
    const store = this.store;
    const cursor = store.oldestEpisodeCursor;
    if (agent === null || !store.hasMoreHistory || store.isLoadingOlder || !cursor) return false;
    store.isLoadingOlder = true;
    try {
      store.prependEpisode(await fetchChatSegment(agent, cursor));
      return true;
    } catch (err) {
      notifications.surface(
        "error",
        userErrorMessage(err, {
          action: "Couldn't load earlier messages.",
          notFound: "That part of the history is no longer available.",
        }),
      );
      return false;
    } finally {
      store.isLoadingOlder = false;
    }
  }

  private async reconcileMainHistory(): Promise<void> {
    const agent = this.agent;
    const store = this.store;
    if (agent === null || !store.historyLoaded) return;
    let recent;
    try {
      recent = await this.fetchRecentHistory(agent, store);
    } catch (err) {
      if (store !== this.store) return;
      notifications.surface(
        "error",
        userErrorMessage(err, {
          action: "Couldn't check for messages missed while disconnected.",
        }),
      );
      return;
    }
    if (store !== this.store) return;
    if (store.reconcileRecent(recent)) return;
    store.reloadHistory(recent);
    await this.loadOlderHistory();
  }

  async loadUsage(): Promise<void> {
    const agent = this.agent;
    if (agent === null) return;
    const store = this.store;
    try {
      store.setSessionUsage(await fetchUsageTotals(agent));
    } catch (err) {
      if (store !== this.store) return;
      store.usageProblem = userErrorMessage(err, {
        action: `Couldn't read the size of the conversation with ${agent}.`,
      });
    }
  }

  get queuedMessages(): number {
    return this.transport.pendingOf("send_message");
  }

  onFrame(listener: (msg: ServerMessage, agent: string | null) => void): () => void {
    this.frameListeners.add(listener);
    return () => this.frameListeners.delete(listener);
  }

  disconnect(): void {
    this.useAgent(null);
  }

  send(msg: ClientMessage): void {
    this.transport.send(msg);
  }

  sendChat(content: string, attachments: readonly StagedAttachment[] = []): void {
    const id = newMessageId();
    const msg: ClientMessage = {
      type: "send_message",
      id,
      content,
      ...(attachments.length > 0
        ? { attachments: attachments.map((attachment) => attachment.uploadId ?? "") }
        : {}),
    };
    const delivered = this.transport.status === "connected";
    this.unconfirmedSends.set(id, { content, attachments: [...attachments] });
    this.transport.send(msg);
    this.store.pushUserMessage(
      content,
      attachments.map((attachment) => ({
        item_id: "",
        filename: attachment.filename,
        size: attachment.size,
        content_type: attachment.contentType || "application/octet-stream",
        path: "",
      })),
      id,
      delivered,
    );
  }

  retryTurn(turnId: string): void {
    this.store.askRetry(turnId);
    this.transport.send({ type: "retry_turn", id: turnId });
  }

  onRefusedSend(listener: (refused: RefusedSend) => void): () => void {
    this.refusalListeners.add(listener);
    return () => this.refusalListeners.delete(listener);
  }

  sendWaitingNow(id: string): void {
    this.store.askOfWaiting(id, "send-now");
    this.store.askStop();
    this.transport.send({ type: "send_queued_now", id });
  }

  withdrawWaiting(id: string): Promise<boolean> {
    if (this.transport.status !== "connected") {
      return Promise.reject(new Error("The agent isn't connected right now."));
    }
    this.store.askOfWaiting(id, "withdraw");
    return new Promise((resolve, reject) => {
      this.withdrawals.set(id, { resolve, reject });
      this.transport.send({ type: "withdraw_queued", id });
    });
  }

  stop(): void {
    const replyTo = this.store.activeTurnId;
    if (!replyTo) return;
    this.store.askStop();
    this.transport.send({ type: "cancel", reply_to: replyTo });
  }
}

export const ws = new WsCoordinator();
