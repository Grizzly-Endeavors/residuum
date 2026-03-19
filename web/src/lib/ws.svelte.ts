// ── WebSocket coordinator (Svelte 5 runes) ──────────────────────────
//
// Thin glue layer that wires WsTransport and FeedStore together.
// The external API (`ws.status`, `ws.feed`, `ws.send()`, etc.) is unchanged.

import { WsTransport } from "./transport.svelte";
import { FeedStore } from "./feed.svelte";
import type { ClientMessage, FeedItem, RecentMessage, ImageAttachment } from "./types";

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
    const msg: ClientMessage = images?.length
      ? { type: "send_message", id, content, images }
      : { type: "send_message", id, content };
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

  loadHistory(messages: RecentMessage[]): void {
    this.store.loadHistory(messages);
  }

  appendFeedItem(item: FeedItem): void {
    this.store.appendFeedItem(item);
  }
}

export const ws = new WsCoordinator();
