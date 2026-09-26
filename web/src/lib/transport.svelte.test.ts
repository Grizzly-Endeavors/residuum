import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { WsTransport } from "./transport.svelte";
import type { ClientMessage } from "./types";

/** A minimal fake WebSocket the test controls directly: no real network. */
class FakeWebSocket {
  static OPEN = 1;
  static CONNECTING = 0;
  static CLOSED = 3;

  readyState = FakeWebSocket.CONNECTING;
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;

  send(data: string): void {
    this.sent.push(data);
  }

  close(): void {
    this.readyState = FakeWebSocket.CLOSED;
  }

  /** Test helper: simulate the socket finishing its handshake. */
  simulateOpen(): void {
    this.readyState = FakeWebSocket.OPEN;
    this.onopen?.();
  }
}

describe("WsTransport queuing while disconnected", () => {
  let lastSocket: FakeWebSocket;

  beforeEach(() => {
    // A constructor function that returns an explicit object bypasses the
    // newly-created `this` entirely, so no `this`-aliasing is needed to
    // capture the instance for the test to control.
    function FakeWebSocketConstructor(): FakeWebSocket {
      const socket = new FakeWebSocket();
      lastSocket = socket;
      return socket;
    }
    FakeWebSocketConstructor.OPEN = FakeWebSocket.OPEN;
    FakeWebSocketConstructor.CONNECTING = FakeWebSocket.CONNECTING;
    FakeWebSocketConstructor.CLOSED = FakeWebSocket.CLOSED;
    vi.stubGlobal("WebSocket", FakeWebSocketConstructor as unknown as typeof WebSocket);
    // jsdom-free environment: transport reads location.host/protocol.
    vi.stubGlobal("location", { protocol: "http:", host: "localhost" });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("queues a message sent while disconnected instead of dropping it", () => {
    const transport = new WsTransport();
    transport.connect();
    // Socket hasn't opened yet — still "connecting", not "connected".
    const msg: ClientMessage = { type: "send_message", id: "web-1", content: "hello" };

    transport.send(msg);

    expect(lastSocket.sent).toHaveLength(0);
    expect(transport.pendingCount).toBe(1);
  });

  it("flushes queued messages in order once the socket reopens", () => {
    const transport = new WsTransport();
    transport.connect();
    const first: ClientMessage = { type: "send_message", id: "web-1", content: "first" };
    const second: ClientMessage = { type: "send_message", id: "web-2", content: "second" };
    transport.send(first);
    transport.send(second);
    expect(transport.pendingCount).toBe(2);

    lastSocket.simulateOpen();

    expect(transport.pendingCount).toBe(0);
    expect(lastSocket.sent).toEqual([JSON.stringify(first), JSON.stringify(second)]);
  });

  it("never queues a ping — it would just be moot by the next reconnect", () => {
    const transport = new WsTransport();
    transport.connect();
    transport.send({ type: "ping" });
    expect(transport.pendingCount).toBe(0);
  });

  it("sends immediately once connected, without touching the queue", () => {
    const transport = new WsTransport();
    transport.connect();
    lastSocket.simulateOpen();
    const msg: ClientMessage = { type: "send_message", id: "web-1", content: "hi" };

    transport.send(msg);

    expect(lastSocket.sent).toEqual([JSON.stringify(msg)]);
    expect(transport.pendingCount).toBe(0);
  });
});
