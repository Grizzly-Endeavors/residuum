// ── WebSocket transport layer (Svelte 5 runes) ──────────────────────

import type { ClientMessage, ServerMessage, ConnectionStatus } from "./types";

export interface TransportOptions {
  /** The socket URL, read on every (re)connect so it can follow the bound agent. */
  url: () => string;
  /** Send a `ping` every 30 seconds. Off for sockets that take no client frames. */
  keepalive?: boolean;
}

/**
 * Log a frame that couldn't be read, or that broke the code handling it.
 * Either way the frame is lost; the next one is handled as usual.
 */
function reportFrameFailure(message: string, err: unknown): void {
  // eslint-disable-next-line no-console -- transport-layer failure has no user-visible channel; project rule mandates failure visibility
  console.warn(message, err);
}

/** Low-level WebSocket transport with reconnect and keepalive. */
export class WsTransport<S = ServerMessage, C extends { type: string } = ClientMessage> {
  status = $state<ConnectionStatus>("disconnected");
  /**
   * The socket closed, or failed to open, and hasn't opened since. Unlike
   * `status`, it stays set through each reconnect attempt, so a notice built
   * on it doesn't flicker while the transport retries.
   */
  lost = $state(false);

  /** Called when a parsed ServerMessage arrives. */
  onMessage: ((msg: S) => void) | null = null;

  /** Called after the socket connects (before any messages). */
  onConnected: (() => void) | null = null;

  /** Called when an open socket closes, before a reconnect is scheduled. */
  onDisconnected: (() => void) | null = null;

  /** Messages queued while disconnected, flushed in order once reconnected. */
  private pending: C[] = [];

  /** How many messages are queued waiting for reconnect (reactive). */
  pendingCount = $state(0);

  private ws: WebSocket | null = null;
  private reconnectDelay = 1000;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private pingTimer: ReturnType<typeof setInterval> | null = null;

  constructor(private readonly options: TransportOptions) {}

  connect(): void {
    this.status = "connecting";
    const socket = new WebSocket(this.options.url());
    this.ws = socket;

    socket.onopen = () => {
      if (this.ws !== socket) return;
      this.status = "connected";
      this.lost = false;
      this.reconnectDelay = 1000;
      this.onConnected?.();
      this.startPing();
      this.flushPending();
    };

    socket.onmessage = (e) => {
      if (this.ws !== socket) return;
      let msg: S;
      try {
        msg = JSON.parse(String(e.data)) as S;
      } catch (err) {
        reportFrameFailure("unparseable ws frame", err);
        return;
      }
      try {
        this.onMessage?.(msg);
      } catch (err) {
        const { type } = msg as { type?: unknown };
        reportFrameFailure(`ws frame handler failed on ${String(type)}`, err);
      }
    };

    socket.onclose = () => {
      if (this.ws !== socket) return;
      const wasConnected = this.status === "connected";
      this.status = "disconnected";
      this.lost = true;
      this.stopPing();
      if (wasConnected) this.onDisconnected?.();
      this.scheduleReconnect();
    };
  }

  disconnect(): void {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.stopPing();
    if (this.ws) {
      this.ws.onopen = null;
      this.ws.onmessage = null;
      this.ws.onclose = null;
      this.ws.close();
      this.ws = null;
    }
    this.status = "disconnected";
    this.lost = false;
  }

  /**
   * Try to connect now instead of waiting out the reconnect delay, and start
   * the backoff over. Does nothing while a connection is open or opening.
   */
  reconnectNow(): void {
    if (this.status !== "disconnected") return;
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.reconnectDelay = 1000;
    this.connect();
  }

  /**
   * Disconnect and drop everything queued for the old connection, for when
   * the socket is about to point somewhere else: a queued message must never
   * be delivered to a different agent than the one it was written for.
   */
  reset(): void {
    this.disconnect();
    this.pending = [];
    this.pendingCount = 0;
    this.reconnectDelay = 1000;
  }

  /**
   * Send a message, or queue it while disconnected/reconnecting so it isn't
   * silently dropped — flushed in order once the socket reopens. A `ping`
   * is never worth queuing (the next real reconnect makes it moot).
   */
  send(msg: C): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
      return;
    }
    if (msg.type !== "ping") {
      this.pending.push(msg);
      this.pendingCount = this.pending.length;
    }
  }

  // ── Private ──────────────────────────────────────────────────────────

  private flushPending(): void {
    const queued = this.pending;
    this.pending = [];
    this.pendingCount = 0;
    for (const msg of queued) {
      this.send(msg);
    }
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer) return;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, this.reconnectDelay);
    this.reconnectDelay = Math.min(this.reconnectDelay * 1.5, 15000);
  }

  private startPing(): void {
    this.stopPing();
    if (!this.options.keepalive) return;
    this.pingTimer = setInterval(() => {
      if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify({ type: "ping" }));
    }, 30000);
  }

  private stopPing(): void {
    if (this.pingTimer) {
      clearInterval(this.pingTimer);
      this.pingTimer = null;
    }
  }
}
