// ── WebSocket coordinator (Svelte 5 runes) ──────────────────────────
//
// Thin glue layer that wires WsTransport and FeedStore together.
// The external API (`ws.status`, `ws.feed`, `ws.send()`, etc.) is unchanged.

import { WsTransport } from "./transport.svelte";
import { FeedStore } from "./feed.svelte";
import type {
  ClientMessage,
  FeedItem,
  RecentHistorySegment,
  EpisodeHistorySegment,
  ImageAttachment,
} from "./types";

class WsCoordinator {
  private transport = new WsTransport();
  private store = new FeedStore();
  private msgCounter = 0;

  verbose = $state(false);

  constructor() {
    try {
      this.verbose = localStorage.getItem("residuum-verbose") === "true";
    } catch {
      // localStorage unavailable
    }

    // Wire transport events to feed store
    this.transport.onMessage = (msg) => {
      this.store.handleMessage(msg);
    };

    this.transport.onConnected = () => {
      if (this.verbose) {
        this.transport.send({ type: "set_verbose", enabled: true });
      }
    };
  }

  // ── Delegated state (read-only from outside) ──────────────────────

  get status() {
    return this.transport.status;
  }

  get feed() {
    return this.store.feed;
  }

  get isProcessing() {
    return this.store.isProcessing;
  }

  get oldestEpisodeCursor() {
    return this.store.oldestEpisodeCursor;
  }

  get hasMoreHistory() {
    return this.store.hasMoreHistory;
  }

  get isLoadingOlder() {
    return this.store.isLoadingOlder;
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

  appendFeedItem(item: FeedItem): void {
    this.store.appendFeedItem(item);
  }
}

export const ws = new WsCoordinator();
