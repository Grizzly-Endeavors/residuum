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
import type { ClientMessage, ImageAttachment, ServerMessage } from "./types";

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
  /** The bound agent's sessions. Replaced on an agent switch. */
  sessions = $state<SessionsStore>(this.createSessions(null, this.store));
  private msgCounter = 0;
  private hasConnected = false;
  /** The agent started while bound, so its chat may be behind once the connection opens. */
  private catchUpOnConnect = false;
  private frameListeners = new Set<(msg: ServerMessage, agent: string | null) => void>();
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
      if (msg.type === "error") {
        notifications.surface("error", msg.message, msg.details ?? undefined);
      } else if (msg.type === "workspace_watch_unavailable") {
        if (!this.liveUpdatesOffShown) notifications.surface("error", msg.message);
        this.liveUpdatesOffShown = true;
      } else if (msg.type === "notice") {
        const notice = noticeFrameNotice(msg.message);
        if (notice !== null) notifications.surface(notice.kind, notice.message, notice.details);
      } else if (msg.type === "reloading") {
        const notice = reloadingNotice();
        if (notice !== null) notifications.surface(notice.kind, notice.message);
        // Gateway is reloading config from disk — anything we cached about
        // server-side state may be stale. Episode history is immutable and
        // intentionally stays cached.
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
      // First on every connection: the activity line is built from the tool
      // frames, which the agent sends only to a connection that asks.
      this.transport.send({ type: "set_verbose", enabled: true });
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
  }

  // ── Agent binding ─────────────────────────────────────────────────

  /**
   * The main chat's feed for one agent. A turn the page joins already
   * running is timed from when the hub says the agent became busy.
   */
  private createFeed(agent: string | null): FeedStore {
    return new FeedStore(() => {
      if (agent === null) return null;
      const since = hub.activityOf(agent).busy_since;
      const at = since === null ? Number.NaN : Date.parse(since);
      return Number.isNaN(at) ? null : at;
    });
  }

  /**
   * Build the sessions store for one agent's feed. A store never outlives its
   * agent: anything still in flight for the old one (a fetch, a stop) lands
   * on a store nobody reads.
   */
  private createSessions(agent: string | null, store: FeedStore): SessionsStore {
    return new SessionsStore({
      agent,
      pushToMain: (from, runId, content, category) => {
        store.pushAgentMessage(from, runId, content, category);
      },
    });
  }

  /**
   * Bind to `name`, the viewed agent: close the bound agent's connection,
   * discard its state, and open the new agent's. `null` unbinds. Calling it
   * with the agent already bound does nothing.
   */
  useAgent(name: string | null): void {
    if (name === this.agent) return;
    this.transport.reset();
    const store = this.createFeed(name);
    this.store = store;
    this.historyError = null;
    this.sessions = this.createSessions(name, store);
    this.hasConnected = false;
    this.catchUpOnConnect = false;
    this.liveUpdatesOffShown = false;
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
      recent = await fetchChatHistory(agent);
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
      recent = await fetchChatHistory(agent);
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

  sendChat(content: string, images?: ImageAttachment[]): void {
    this.msgCounter++;
    const id = `web-${this.msgCounter}`;
    const msg: ClientMessage = {
      type: "send_message",
      id,
      content,
      ...(images !== undefined && images.length > 0 ? { images } : {}),
    };
    this.transport.send(msg);
    this.store.pushUserMessage(content, images, id);
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
