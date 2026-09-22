// ── WebSocket coordinator (Svelte 5 runes) ──────────────────────────
//
// Thin glue layer that wires WsTransport to the main chat's FeedStore and
// the agent sessions store.

import { WsTransport } from "./transport.svelte";
import { FeedStore } from "./feed.svelte";
import { SessionsStore, isSessionFrame } from "./sessions.svelte";
import { notifications } from "./notifications.svelte";
import { invalidate } from "./cache";
import {
  CACHE_KEY_STATUS,
  CACHE_KEY_TIMEZONE,
  CACHE_KEY_MCP_CATALOG,
  CACHE_KEY_CONFIG_RAW,
  CACHE_KEY_PROVIDERS_RAW,
  CACHE_KEY_MCP_RAW,
} from "./api";
import type {
  ClientMessage,
  RecentHistorySegment,
  EpisodeHistorySegment,
  ImageAttachment,
} from "./types";

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
      // Session activity has its own store. It must never reach the main
      // feed, whose `error` handling would clear the main turn's state.
      if (isSessionFrame(msg)) {
        this.sessions.handleFrame(msg);
        return;
      }
      if (msg.type === "error") {
        notifications.surface("error", msg.message);
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
    };
  }

  setLoadingOlder(value: boolean): void {
    this.store.isLoadingOlder = value;
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

  loadHistory(segment: RecentHistorySegment): void {
    this.store.loadHistory(segment);
  }

  prependEpisode(segment: EpisodeHistorySegment): void {
    this.store.prependEpisode(segment);
  }
}

export const ws = new WsCoordinator();
