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
    const transport = new WsTransport({
      url: () => "ws://localhost/api/agents/scout/ws",
      keepalive: true,
    });
    transport.connect();
    // Socket hasn't opened yet — still "connecting", not "connected".
    const msg: ClientMessage = { type: "send_message", id: "web-1", content: "hello" };

    transport.send(msg);

    expect(lastSocket.sent).toHaveLength(0);
    expect(transport.pendingCount).toBe(1);
  });

  it("flushes queued messages in order once the socket reopens", () => {
    const transport = new WsTransport({
      url: () => "ws://localhost/api/agents/scout/ws",
      keepalive: true,
    });
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
    const transport = new WsTransport({
      url: () => "ws://localhost/api/agents/scout/ws",
      keepalive: true,
    });
    transport.connect();
    transport.send({ type: "ping" });
    expect(transport.pendingCount).toBe(0);
  });

  it("sends immediately once connected, without touching the queue", () => {
    const transport = new WsTransport({
      url: () => "ws://localhost/api/agents/scout/ws",
      keepalive: true,
    });
    transport.connect();
    lastSocket.simulateOpen();
    const msg: ClientMessage = { type: "send_message", id: "web-1", content: "hi" };

    transport.send(msg);

    expect(lastSocket.sent).toEqual([JSON.stringify(msg)]);
    expect(transport.pendingCount).toBe(0);
  });
});

describe("WsTransport losing and regaining the connection", () => {
  let sockets: FakeWebSocket[];

  beforeEach(() => {
    vi.useFakeTimers();
    sockets = [];
    function FakeWebSocketConstructor(): FakeWebSocket {
      const socket = new FakeWebSocket();
      sockets.push(socket);
      return socket;
    }
    FakeWebSocketConstructor.OPEN = FakeWebSocket.OPEN;
    vi.stubGlobal("WebSocket", FakeWebSocketConstructor as unknown as typeof WebSocket);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  const latest = (): FakeWebSocket => sockets[sockets.length - 1] as FakeWebSocket;

  it("stays lost through reconnect attempts until a socket opens", () => {
    const transport = new WsTransport({ url: () => "ws://localhost/api/hub/ws" });
    transport.connect();
    latest().simulateOpen();
    expect(transport.lost).toBe(false);

    latest().onclose?.();
    expect(transport.lost).toBe(true);
    vi.advanceTimersByTime(1000);
    expect(transport.status).toBe("connecting");
    expect(transport.lost).toBe(true);

    latest().simulateOpen();
    expect(transport.lost).toBe(false);
  });

  it("counts a first connection that never opens as lost", () => {
    const transport = new WsTransport({ url: () => "ws://localhost/api/hub/ws" });
    transport.connect();
    expect(transport.lost).toBe(false);
    latest().onclose?.();
    expect(transport.lost).toBe(true);
  });

  it("reconnects at once on request, instead of waiting out the backoff", () => {
    const transport = new WsTransport({ url: () => "ws://localhost/api/hub/ws" });
    transport.connect();
    latest().onclose?.();
    expect(sockets).toHaveLength(1);

    transport.reconnectNow();
    expect(sockets).toHaveLength(2);
    expect(transport.status).toBe("connecting");

    // The close's own retry was cancelled: no third socket appears.
    vi.advanceTimersByTime(20_000);
    expect(sockets).toHaveLength(2);
  });

  it("ignores a reconnect request while a socket is open or opening", () => {
    const transport = new WsTransport({ url: () => "ws://localhost/api/hub/ws" });
    transport.connect();
    transport.reconnectNow();
    latest().simulateOpen();
    transport.reconnectNow();
    expect(sockets).toHaveLength(1);
  });

  it("names a frame whose handler threw, rather than calling it unparseable", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const transport = new WsTransport<{ type: string }>({ url: () => "ws://localhost/api/hub/ws" });
    transport.onMessage = () => {
      throw new Error("handler bug");
    };
    transport.connect();
    latest().simulateOpen();

    latest().onmessage?.({ data: JSON.stringify({ type: "agent_state" }) });
    latest().onmessage?.({ data: "{not json" });

    expect(warn.mock.calls.map((call): unknown => call[0])).toEqual([
      "ws frame handler failed on agent_state",
      "unparseable ws frame",
    ]);
    warn.mockRestore();
  });
});
