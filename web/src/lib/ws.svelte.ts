// ── WebSocket coordinator (Svelte 5 runes) ──────────────────────────
//
// Thin glue layer that wires WsTransport to the main chat's FeedStore and
// the agent sessions store. It is bound to one agent at a time: switching
// agents tears the connection down, replaces every agent-scoped store, and
// opens a new connection, so nothing from one agent shows under another.
// The connection is open only while the hub says the agent runs: a stopped
// or failed agent's chat shows its history, read from its files, and nothing
// tries to reach it until it starts.

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

/** A chat message the agent refused: what was written, and the files staged for it. */
export interface RefusedSend {
  content: string;
  attachments: StagedAttachment[];
}

class WsCoordinator {
  /** The bound agent: the one this connection and its stores belong to, or `null` before one is chosen. */
  agent = $state<string | null>(null);
  transport = new WsTransport({
    url: () => agentWsUrl(this.agent ?? ""),
    keepalive: true,
  });
  /** The main chat's feed for the bound agent. Replaced on an agent switch. */
  store = $state<FeedStore>(this.createFeed(null));
  /** Why the main chat's history couldn't be loaded, in plain words, until a load succeeds. */
  historyError = $state<string | null>(null);
  /**
   * The bound agent's sessions. Replaced on an agent switch: a store never
   * outlives its agent, so anything still in flight for the old one (a fetch,
   * a stop) lands on a store nobody reads.
   */
  sessions = $state<SessionsStore>(new SessionsStore({ agent: null }));
  private hasConnected = false;
  /** The agent started while bound, so its chat may be behind once the connection opens. */
  private catchUpOnConnect = false;
  private frameListeners = new Set<(msg: ServerMessage, agent: string | null) => void>();
  /** Withdrawals of waiting messages awaiting the agent's answer, by message id. */
  private withdrawals = new Map<
    string,
    { resolve: (withdrawn: boolean) => void; reject: (err: Error) => void }
  >();
  /**
   * The bound agent's workspace watches. Whatever follows changes on this
   * socket registers here, and the registry keeps the socket's one watch set
   * the union of theirs.
   */
  readonly watches = new WatchRegistry({
    send: (prefixes) => {
      this.transport.send({ type: "watch_workspace", prefixes });
    },
    normalize: normalizeWatchPrefix,
    refusal: (prefix) =>
      `can't watch "${prefix}": watch paths are relative to the workspace, like "team/wiki", and can't contain ".."`,
  });
  /** Whether this connection already told the user live updates are off. */
  private liveUpdatesOffShown = false;
  /**
   * Chat messages this page sent that nothing has yet said the agent took in
   * (its `user_message`, `message_queued` or `turn_started`), by id: what
   * each held, to hand back should a `message_refused` name it.
   */
  private unconfirmedSends = new Map<string, RefusedSend>();
  private refusalListeners = new Set<(refused: RefusedSend) => void>();

  constructor() {
    onViewedAgentChange((name) => {
      this.useAgent(name);
    });
    // First among the frame observers, so a watch owner has acted on a change
    // before the other observers hear of it.
    this.frameListeners.add((msg) => {
      this.watches.handleFrame(msg);
    });

    // Wire transport events: route system events to the notification
    // surface, then hand the message to the feed store for any chat-state
    // side effects (e.g. an error clears the reply in progress).
    this.transport.onMessage = (msg) => {
      for (const listener of this.frameListeners) {
        try {
          listener(msg, this.agent);
        } catch (err) {
          // eslint-disable-next-line no-console -- a failing observer must not stop the chat from handling the frame; the console is its only channel
          console.error("frame listener failed", err);
        }
      }
      // Session activity has its own store. It must never reach the main
      // feed, whose `error` handling would clear the main turn's state.
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
        // The failure card says it too, beside the button it replaces; the
        // toast is what a screen reader hears. Nothing of a send was
        // refused, so the composer is left alone.
        notifications.surface("error", msg.message);
      } else if (msg.type === "workspace_watch_unavailable") {
        if (!this.liveUpdatesOffShown) notifications.surface("error", msg.message);
        this.liveUpdatesOffShown = true;
      } else if (msg.type === "notice") {
        const notice = noticeFrameNotice(msg);
        if (notice !== null) notifications.surface(notice.kind, notice.message, notice.details);
      } else if (msg.type === "reloading") {
        const notice = reloadingNotice();
        if (notice !== null) notifications.surface(notice.kind, notice.message);
        // Gateway is reloading config from disk — anything we cached about
        // server-side state may be stale.
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
      // Before anything goes out on it, so a message sent from here on counts as this connection's.
      this.store.connected();
      // First on every connection: the activity line is built from the tool
      // frames, which the agent sends only to a connection that asks.
      this.transport.send({ type: "set_verbose", enabled: true });
      // Then the turn in flight so far, which the live frames alone can't
      // show: what streamed before this page connected, or while it was away.
      this.transport.send({ type: "resync_turn" });
      // A turn still in flight carried on while the page was away.
      if (this.hasConnected) this.store.markReconnectGap();
      // Load the sessions listing, or catch up on frames missed while
      // disconnected.
      this.sessions.resync();
      // After a reconnect, or once the agent has started, the main chat may
      // have missed messages too (a session's relayed result, main's reply).
      if (this.hasConnected || this.catchUpOnConnect) void this.reconcileMainHistory();
      this.hasConnected = true;
      this.catchUpOnConnect = false;
      // The conversation's size as the agent has it now, before its next model call.
      void this.loadUsage();
      // A new connection watches nothing until told, so the watch set goes
      // out again before any owner hears of the reconnect.
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

  // ── Following the agent's state ───────────────────────────────────

  /**
   * Whether the bound agent's connection should be open: the hub lists it
   * running, or hasn't listed the agents yet.
   */
  private connectionWanted(): boolean {
    if (this.agent === null) return false;
    const state = hub.agent(this.agent)?.state;
    return state === undefined ? !hub.loaded : state === "running";
  }

  /** Open the connection once the agent runs, and close it once it doesn't. */
  private followAgentState(wanted: boolean): void {
    if (this.agent === null) return;
    if (wanted) {
      if (this.transport.status !== "disconnected") return;
      this.catchUpOnConnect = this.store.historyLoaded;
      this.transport.reconnectNow();
      return;
    }
    const wasOpen = this.transport.status === "connected";
    // Also cancels a reconnect that the agent's own shutdown scheduled.
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

  /** A frame that answers a withdrawal: taken back, no longer waiting, or read. */
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

  /** A frame that says the agent took in a message this page sent. */
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

  /**
   * The agent refused the message `id` this page sent: what it held goes back
   * to a listener's composer, or into the agent's draft when nothing listens,
   * so it waits there for the chat to open. Its files upload again, since the
   * hub's copies may be gone (the feed takes the message off by itself).
   */
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

  /** Every withdrawal still awaiting an answer will get none: it fails with `reason`. */
  private failWithdrawals(reason: string): void {
    const pending = [...this.withdrawals.values()];
    this.withdrawals.clear();
    for (const { reject } of pending) reject(new Error(reason));
  }

  // ── Agent binding ─────────────────────────────────────────────────

  /**
   * The main chat's feed for one agent. A turn the page joins already
   * running is timed from when the hub says the agent became busy.
   */
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

  /**
   * Bind to `name`, the viewed agent: close the bound agent's connection,
   * discard its state, and open the new agent's. `null` unbinds. Calling it
   * with the agent already bound does nothing.
   */
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
    // Read whatever the agent's state, so a stopped agent shows its last figures.
    void this.loadUsage();
  }

  // ── Main chat history ─────────────────────────────────────────────

  /**
   * Load the main chat's recent history, replacing the feed, then the
   * newest episode so the chat is never empty after the observer compresses
   * history and the "compressed history" marker shows from the start.
   */
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
    // A message sent before the history arrived keeps its turn on screen.
    store.reloadHistory(recent);
    await this.loadOlderHistory();
  }

  /**
   * Read the main chat's Recent segment, and read it again whenever a turn
   * ended while it was on its way: the read may have come before history
   * recorded that turn, and applying it would take the turn off the feed.
   * A read that follows the page hearing the turn end holds it, since history
   * records a turn before its end goes out. It needs no cap: each further read
   * needs another turn to end during the one before, and main turns run one
   * at a time.
   */
  private async fetchRecentHistory(agent: string, store: FeedStore): Promise<RecentHistorySegment> {
    let ended;
    let recent;
    do {
      ended = store.turnsEndedLive;
      recent = await fetchChatHistory(agent);
    } while (store === this.store && store.turnsEndedLive !== ended);
    return recent;
  }

  /**
   * Prepend the next older episode to the main chat. The single path for
   * every caller, so overlapping requests can't load an episode twice.
   * Returns whether an episode was added.
   */
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

  /** Catch the main chat up on messages recorded while disconnected. */
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

  /**
   * Read the conversation's size: the cumulative token totals the agent
   * keeps, which `turn_usage` frames update from here on. A failure is kept
   * on the store, where the conversation-size view shows it with a retry.
   */
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

  /** The user's messages waiting for the connection to come back. */
  get queuedMessages(): number {
    return this.transport.pendingOf("send_message");
  }

  /**
   * Observe every frame the server sends, alongside the stores that handle
   * them. Returns a function that stops observing.
   */
  onFrame(listener: (msg: ServerMessage, agent: string | null) => void): () => void {
    this.frameListeners.add(listener);
    return () => this.frameListeners.delete(listener);
  }

  // ── Delegated methods ─────────────────────────────────────────────

  /** Close the connection and unbind from the bound agent. */
  disconnect(): void {
    this.useAgent(null);
  }

  send(msg: ClientMessage): void {
    this.transport.send(msg);
  }

  /**
   * Send a chat message with the files staged for it, which go as their
   * upload ids. Should the agent refuse it (`message_refused`), the
   * `onRefusedSend` listeners get it back, files and all, or the agent's draft
   * does when nothing listens.
   */
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

  /**
   * Have the agent run the failed turn `turnId` again on the message it
   * already holds: no new message goes, so no bubble is added and nothing is
   * uploaded twice. The rerun's start takes the failure's place in the feed;
   * should the agent refuse (`retry_refused`), the failure says why.
   */
  retryTurn(turnId: string): void {
    this.store.askRetry(turnId);
    this.transport.send({ type: "retry_turn", id: turnId });
  }

  /**
   * Hear of each chat message this page sent that the agent refused, with
   * what it held, to put back in a composer that is showing; its files are
   * already uploading again. While anything listens, the agent's draft is
   * left alone. Returns a function that stops listening.
   */
  onRefusedSend(listener: (refused: RefusedSend) => void): () => void {
    this.refusalListeners.add(listener);
    return () => this.refusalListeners.delete(listener);
  }

  /**
   * Have the waiting message `id` sent now: the turn in flight stops, and the
   * message starts the next one.
   */
  sendWaitingNow(id: string): void {
    this.store.askOfWaiting(id, "send-now");
    this.store.askStop();
    this.transport.send({ type: "send_queued_now", id });
  }

  /**
   * Take back the waiting message `id` before the agent reads it. Resolves to
   * whether it was taken back: `false` when the agent had already read it.
   * Fails when the connection is down, or drops before the agent answers.
   */
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

  /**
   * Stop the turn currently in flight, if any. A no-op when nothing is
   * running — the server ignores a stop with no matching turn.
   */
  stop(): void {
    const replyTo = this.store.activeTurnId;
    if (!replyTo) return;
    this.store.askStop();
    this.transport.send({ type: "cancel", reply_to: replyTo });
  }
}

export const ws = new WsCoordinator();
