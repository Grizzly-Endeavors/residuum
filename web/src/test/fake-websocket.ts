/**
 * A `WebSocket` the test drives by hand: no network. Install it with
 * `FakeWebSocket.install()`; every socket the code under test opens is in
 * `FakeWebSocket.sockets`, oldest first.
 */
import { vi } from "vitest";

export class FakeWebSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSED = 3;

  static sockets: FakeWebSocket[] = [];

  /** Replace the global `WebSocket` and forget earlier sockets. */
  static install(): void {
    FakeWebSocket.sockets = [];
    vi.stubGlobal("WebSocket", FakeWebSocket);
  }

  /** The newest socket opened. */
  static get last(): FakeWebSocket {
    const socket = FakeWebSocket.sockets[FakeWebSocket.sockets.length - 1];
    if (!socket) throw new Error("no socket has been opened");
    return socket;
  }

  readyState = FakeWebSocket.CONNECTING;
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;

  constructor(readonly url: string) {
    FakeWebSocket.sockets.push(this);
  }

  send(data: string): void {
    this.sent.push(data);
  }

  close(): void {
    this.readyState = FakeWebSocket.CLOSED;
  }

  /** Simulate the handshake finishing. */
  simulateOpen(): void {
    this.readyState = FakeWebSocket.OPEN;
    this.onopen?.();
  }

  /** Simulate a frame from the server. Does nothing if the code detached its handler. */
  simulateMessage(frame: unknown): void {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }

  /** Simulate the server or network closing the socket. */
  simulateClose(): void {
    this.readyState = FakeWebSocket.CLOSED;
    this.onclose?.();
  }

  /** The frames the code sent, parsed. */
  sentFrames(): unknown[] {
    return this.sent.map((s) => JSON.parse(s) as unknown);
  }
}
