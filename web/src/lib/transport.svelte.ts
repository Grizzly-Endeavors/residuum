// ── WebSocket transport layer (Svelte 5 runes) ──────────────────────

import type { ClientMessage, ServerMessage, ConnectionStatus } from "./types";

/** Low-level WebSocket transport with reconnect and keepalive. */
export class WsTransport {
  status = $state<ConnectionStatus>("disconnected");

  /** Called when a parsed ServerMessage arrives. */
  onMessage: ((msg: ServerMessage) => void) | null = null;

  /** Called after the socket connects (before any messages). */
  onConnected: (() => void) | null = null;

  private ws: WebSocket | null = null;
  private reconnectDelay = 1000;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private pingTimer: ReturnType<typeof setInterval> | null = null;

  connect(): void {
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    const url = `${proto}//${location.host}/ws`;

    this.status = "connecting";
    this.ws = new WebSocket(url);

    this.ws.onopen = () => {
      this.status = "connected";
      this.reconnectDelay = 1000;
      this.onConnected?.();
      this.startPing();
    };

    this.ws.onmessage = (e) => {
      try {
        const msg: ServerMessage = JSON.parse(e.data);
        this.onMessage?.(msg);
      } catch (err) {
        // eslint-disable-next-line no-console -- transport-layer failure has no user-visible channel; project rule mandates failure visibility
        console.warn("unparseable ws frame", err);
      }
    };

    this.ws.onclose = () => {
      this.status = "disconnected";
      this.stopPing();
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
      this.ws.onclose = null;
      this.ws.close();
      this.ws = null;
    }
    this.status = "disconnected";
  }

  send(msg: ClientMessage): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }

  // ── Private ──────────────────────────────────────────────────────────

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
    this.pingTimer = setInterval(() => {
      this.send({ type: "ping" });
    }, 30000);
  }

  private stopPing(): void {
    if (this.pingTimer) {
      clearInterval(this.pingTimer);
      this.pingTimer = null;
    }
  }
}
