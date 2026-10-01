// ── WebSocket coordinator (Svelte 5 runes) ──────────────────────────
//
// Thin glue layer that wires WsTransport to the main chat's FeedStore and
// the agent sessions store. It is bound to one agent at a time: switching
// agents tears the connection down, replaces every agent-scoped store, and
// opens a new connection, so nothing from one agent shows under another.

import { WsTransport } from "./transport.svelte";
import { agentWsUrl } from "./paths";
import { onViewedAgentChange } from "./viewed-agent";
import { scheduled } from "./scheduled.svelte";
import { FeedStore } from "./feed.svelte";
import { SessionsStore, isSessionFrame } from "./sessions.svelte";
import { notifications } from "./notifications.svelte";
import { invalidate } from "./cache";
import { userErrorMessage } from "./errors";
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
  store = $state<FeedStore>(new FeedStore());
  /** The bound agent's sessions. Replaced on an agent switch. */
  sessions = $state<SessionsStore>(this.createSessions(null, this.store));
  private msgCounter = 0;
  private hasConnected = false;
  private frameListeners = new Set<(msg: ServerMessage, agent: string | null) => void>();
  private connectionListeners = new Set<(connected: boolean) => void>();
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

  verbose = $state(false);

  constructor() {
    onViewedAgentChange((name) => {
      this.useAgent(name);
    });
    // First among the frame observers, so a watch owner has acted on a change
    // before the other observers hear of it.
    this.frameListeners.add((msg) => {
      this.watches.handleFrame(msg);
    });
    try {
      this.verbose = localStorage.getItem("residuum-verbose") === "true";
    } catch {
      // localStorage unavailable
    }

    // Wire transport events: route system events to the notification
    // surface, then hand the message to the feed store for any chat-state
    // side effects (e.g. clearing the thinking indicator on errors).
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
        notifications.surface("notice", msg.message);
      } else if (msg.type === "reloading") {
        notifications.surface("system", "Gateway is reloading…");
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
      if (this.verbose) {
        this.transport.send({ type: "set_verbose", enabled: true });
      }
      // Load the sessions listing, or catch up on frames missed while
      // disconnected.
      this.sessions.resync();
      // After a reconnect, the main chat may have missed messages too (a
      // session's relayed result, main's reply).
      if (this.hasConnected) void this.reconcileMainHistory();
      this.hasConnected = true;
      // Seed the chat footer so it renders correctly before the next model
      // call, rather than starting blank on every connect.
      void this.loadUsageTotals();
      // A new connection watches nothing until told. The watch set goes out
      // before listeners hear of the reconnect, so an artifact that reloads
      // on it can't miss changes made in between.
      this.liveUpdatesOffShown = false;
      this.watches.connected();
      this.notifyConnection(true);
    };

    this.transport.onDisconnected = () => {
      this.watches.disconnected();
      this.store.clearPostTurnActivity();
      this.notifyConnection(false);
    };
  }

  // ── Agent binding ─────────────────────────────────────────────────

  /**
   * Build the sessions store for one agent's feed. A store never outlives its
   * agent: anything still in flight for the old one (a fetch, a queued
   * command) lands on a store nobody reads, or is dropped.
   */
  private createSessions(agent: string | null, store: FeedStore): SessionsStore {
    const sessions: SessionsStore = new SessionsStore({
      agent,
      send: (msg) => {
        if (this.sessions === sessions) this.transport.send(msg);
      },
      pushToMain: (from, runId, content, category) => {
        store.pushAgentMessage(from, runId, content, category);
      },
    });
    return sessions;
  }

  /**
   * Bind to `name`, the viewed agent: close the bound agent's connection,
   * discard its state, and open the new agent's. `null` unbinds. Calling it
   * with the agent already bound does nothing.
   */
  useAgent(name: string | null): void {
    if (name === this.agent) return;
    this.transport.reset();
    const store = new FeedStore();
    this.store = store;
    this.sessions = this.createSessions(name, store);
    this.hasConnected = false;
    this.liveUpdatesOffShown = false;
    this.watches.bind(name);
    scheduled.reset(name);
    this.agent = name;
    if (name === null) return;
    this.transport.connect();
    void this.loadMainHistory();
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
    let recent;
    try {
      recent = await fetchChatHistory(agent);
    } catch (err) {
      if (store !== this.store) return;
      notifications.surface(
        "error",
        userErrorMessage(err, { action: "Couldn't load the chat history." }),
      );
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
   * Seed the chat footer's cumulative totals on connect/reconnect. Fails
   * quietly — this is a quiet, non-critical status line, not something
   * worth a toast over; the footer just stays blank until the next
   * `turn_usage` frame arrives.
   */
  private async loadUsageTotals(): Promise<void> {
    const agent = this.agent;
    if (agent === null) return;
    const store = this.store;
    try {
      store.setInitialUsage(await fetchUsageTotals(agent));
    } catch {
      // quiet degradation, by design — see doc comment above
    }
  }

  /**
   * Observe every frame the server sends, alongside the stores that handle
   * them. Returns a function that stops observing.
   */
  onFrame(listener: (msg: ServerMessage, agent: string | null) => void): () => void {
    this.frameListeners.add(listener);
    return () => this.frameListeners.delete(listener);
  }

  /**
   * Observe the socket connecting and disconnecting. Returns a function that
   * stops observing.
   */
  onConnectionChange(listener: (connected: boolean) => void): () => void {
    this.connectionListeners.add(listener);
    return () => this.connectionListeners.delete(listener);
  }

  private notifyConnection(connected: boolean): void {
    for (const listener of this.connectionListeners) listener(connected);
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
    this.store.pushUserMessage(content, images);
  }

  /**
   * Stop the turn currently in flight, if any. A no-op when nothing is
   * running — the server ignores a stop with no matching turn.
   */
  stop(): void {
    const replyTo = this.store.activeTurnId;
    if (!replyTo) return;
    this.transport.send({ type: "cancel", reply_to: replyTo });
  }

  setVerbose(enabled: boolean): void {
    this.verbose = enabled;
    try {
      localStorage.setItem("residuum-verbose", String(enabled));
    } catch {
      // localStorage unavailable
    }
    this.transport.send({ type: "set_verbose", enabled });
  }
}

export const ws = new WsCoordinator();
