// ── WebSocket coordinator (Svelte 5 runes) ──────────────────────────
//
// Thin glue layer that wires WsTransport and FeedStore together.

import { WsTransport } from "./transport.svelte";
import { FeedStore } from "./feed.svelte";
import { notifications } from "./notifications.svelte";
import type {
  ClientMessage,
  RecentHistorySegment,
  EpisodeHistorySegment,
  ImageAttachment,
} from "./types";

class WsCoordinator {
  transport = new WsTransport();
  store = new FeedStore();
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
      if (msg.type === "error") {
        notifications.surface("error", msg.message);
      } else if (msg.type === "notice") {
        notifications.surface("notice", msg.message);
      } else if (msg.type === "reloading") {
        notifications.surface("system", "Gateway is reloading…");
      }
      this.store.handleMessage(msg);
    };

    this.transport.onConnected = () => {
      if (this.verbose) {
        this.transport.send({ type: "set_verbose", enabled: true });
      }
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
