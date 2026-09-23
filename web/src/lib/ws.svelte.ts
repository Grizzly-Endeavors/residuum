// ── WebSocket coordinator (Svelte 5 runes) ──────────────────────────
//
// Thin glue layer that wires WsTransport to the main chat's FeedStore and
// the agent sessions store.

import { WsTransport } from "./transport.svelte";
import { FeedStore } from "./feed.svelte";
import { SessionsStore, isSessionFrame } from "./sessions.svelte";
import { notifications } from "./notifications.svelte";
import { invalidate } from "./cache";
import { userErrorMessage } from "./errors";
import { WorkspaceWatchSync } from "./workspace-watch";
import {
  fetchChatHistory,
  fetchChatSegment,
  CACHE_KEY_STATUS,
  CACHE_KEY_TIMEZONE,
  CACHE_KEY_MCP_CATALOG,
  CACHE_KEY_CONFIG_RAW,
  CACHE_KEY_PROVIDERS_RAW,
  CACHE_KEY_MCP_RAW,
} from "./api";
import type { ClientMessage, ImageAttachment, ServerMessage } from "./types";

class WsCoordinator {
  transport = new WsTransport();
  store = new FeedStore();
  sessions = new SessionsStore({
    send: (msg) => {
      this.transport.send(msg);
    },
    pushToMain: (from, runId, content, category) => {
      this.store.pushAgentMessage(from, runId, content, category);
    },
  });
  private msgCounter = 0;
  private hasConnected = false;
  private frameListeners = new Set<(msg: ServerMessage) => void>();
  private connectionListeners = new Set<(connected: boolean) => void>();
  /** The open artifact's watched workspace prefixes, re-sent on reconnect. */
  private workspaceWatch = new WorkspaceWatchSync((msg) => {
    this.transport.send(msg);
  });
  /** Whether this connection already told the user live updates are off. */
  private liveUpdatesOffShown = false;

  verbose = $state(false);

  constructor() {
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
          listener(msg);
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
        notifications.surface("error", msg.message);
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
        invalidate(CACHE_KEY_STATUS);
        invalidate(CACHE_KEY_TIMEZONE);
        invalidate(CACHE_KEY_MCP_CATALOG);
        invalidate(CACHE_KEY_CONFIG_RAW);
        invalidate(CACHE_KEY_PROVIDERS_RAW);
        invalidate(CACHE_KEY_MCP_RAW);
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
      // A new connection watches nothing until told. The watch set goes out
      // before listeners hear of the reconnect, so an artifact that reloads
      // on it can't miss changes made in between.
      this.liveUpdatesOffShown = false;
      this.workspaceWatch.connected();
      this.notifyConnection(true);
    };

    this.transport.onDisconnected = () => {
      this.notifyConnection(false);
    };
  }

  // ── Main chat history ─────────────────────────────────────────────

  /**
   * Load the main chat's recent history, replacing the feed, then the
   * newest episode so the chat is never empty after the observer compresses
   * history and the "compressed history" marker shows from the start.
   */
  async loadMainHistory(): Promise<void> {
    let recent;
    try {
      recent = await fetchChatHistory();
    } catch (err) {
      notifications.surface(
        "error",
        userErrorMessage(err, { action: "Couldn't load the chat history." }),
      );
      return;
    }
    this.store.loadHistory(recent);
    await this.loadOlderHistory();
  }

  /**
   * Prepend the next older episode to the main chat. The single path for
   * every caller, so overlapping requests can't load an episode twice.
   * Returns whether an episode was added.
   */
  async loadOlderHistory(): Promise<boolean> {
    const store = this.store;
    const cursor = store.oldestEpisodeCursor;
    if (!store.hasMoreHistory || store.isLoadingOlder || !cursor) return false;
    store.isLoadingOlder = true;
    try {
      store.prependEpisode(await fetchChatSegment(cursor));
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
    if (!this.store.historyLoaded) return;
    let recent;
    try {
      recent = await fetchChatHistory();
    } catch (err) {
      notifications.surface(
        "error",
        userErrorMessage(err, {
          action: "Couldn't check for messages missed while disconnected.",
        }),
      );
      return;
    }
    if (this.store.reconcileRecent(recent)) return;
    this.store.reloadHistory(recent);
    await this.loadOlderHistory();
  }

  /**
   * Observe every frame the server sends, alongside the stores that handle
   * them. Returns a function that stops observing.
   */
  onFrame(listener: (msg: ServerMessage) => void): () => void {
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

  /**
   * Watch these workspace path prefixes on this connection (the open
   * artifact's), replacing any before. `[]` stops watching.
   */
  watchWorkspace(prefixes: readonly string[]): void {
    this.workspaceWatch.set(prefixes);
  }

  // ── Delegated methods ─────────────────────────────────────────────

  connect(): void {
    this.transport.connect();
  }

  disconnect(): void {
    this.transport.disconnect();
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
      ...(images?.length ? { images } : {}),
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
