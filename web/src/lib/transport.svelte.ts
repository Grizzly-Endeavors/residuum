import type { ClientMessage, ServerMessage, ConnectionStatus } from "./types";
import { FIRST_RECONNECT_MS } from "./reconnect";

export interface TransportOptions {
  url: () => string;
  keepalive?: boolean;
}

function reportFrameFailure(message: string, err: unknown): void {
  // eslint-disable-next-line no-console -- transport-layer failure has no user-visible channel; project rule mandates failure visibility
  console.warn(message, err);
}

export class WsTransport<S = ServerMessage, C extends { type: string } = ClientMessage> {
  status = $state<ConnectionStatus>("disconnected");
  lost = $state(false);

  onMessage: ((msg: S) => void) | null = null;

  onConnected: (() => void) | null = null;

  onDisconnected: (() => void) | null = null;

  private pending = $state.raw<C[]>([]);

  get pendingCount(): number {
    return this.pending.length;
  }

  pendingOf(type: C["type"]): number {
    return this.pending.filter((msg) => msg.type === type).length;
  }

  private ws: WebSocket | null = null;
  private reconnectDelay = FIRST_RECONNECT_MS;
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
      this.reconnectDelay = FIRST_RECONNECT_MS;
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

  reconnectNow(): void {
    if (this.status !== "disconnected") return;
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.reconnectDelay = FIRST_RECONNECT_MS;
    this.connect();
  }

  reset(): void {
    this.disconnect();
    this.pending = [];
    this.reconnectDelay = FIRST_RECONNECT_MS;
  }

  send(msg: C): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
      return;
    }
    if (msg.type !== "ping") this.pending = [...this.pending, msg];
  }

  private flushPending(): void {
    const queued = this.pending;
    this.pending = [];
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
