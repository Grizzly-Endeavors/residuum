import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { jsonResponse, mockFetch, render, screen, settle, stubWebSocket } from "./test/component";
import Chat from "./Chat.svelte";
import { ws } from "./lib/ws.svelte";
import { setViewedAgent } from "./lib/viewed-agent";

beforeEach(() => {
  class NoObserver {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }
  vi.stubGlobal("IntersectionObserver", NoObserver);
  vi.stubGlobal("ResizeObserver", NoObserver);
  stubWebSocket();
  mockFetch(() => jsonResponse({}, 404));
});

afterEach(() => {
  setViewedAgent(null);
});

describe("chat counters across an agent switch", () => {
  it("shows the counters of the agent that is open, not the one that was", async () => {
    setViewedAgent("scout");
    ws.store.isProcessing = true;
    ws.store.turnStartedAt = Date.now();
    ws.store.turnHasUsage = true;
    ws.store.turnOutputTokens = 120;
    ws.store.turnToolCalls = 4;
    ws.store.sessionUsage = {
      input_tokens: 900,
      output_tokens: 300,
      tool_calls: 7,
    } as never;
    render(Chat);
    await settle();
    expect(screen.getByText(/4 tool calls/)).toBeTruthy();
    expect(screen.getByText(/7 tool calls/)).toBeTruthy();

    setViewedAgent("atlas");
    await settle();
    expect(screen.queryByText(/4 tool calls/)).toBeNull();
    expect(screen.queryByText(/7 tool calls/)).toBeNull();

    ws.store.isProcessing = true;
    ws.store.turnStartedAt = Date.now();
    ws.store.turnHasUsage = true;
    ws.store.turnOutputTokens = 30;
    ws.store.turnToolCalls = 2;
    await settle();
    expect(screen.getByText(/2 tool calls/)).toBeTruthy();
    expect(screen.queryByText(/4 tool calls/)).toBeNull();

    setViewedAgent("scout");
    await settle();
    expect(screen.queryByText(/2 tool calls/)).toBeNull();
  });
});
