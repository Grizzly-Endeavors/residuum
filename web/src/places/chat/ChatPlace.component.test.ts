import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import {
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  stubWebSocket,
} from "../../test/component";
import { snapshot } from "../../test/hub-frames";
import { hub } from "../../lib/hub.svelte";
import type { AgentSummary } from "../../lib/hub-types";
import { router } from "../../lib/router.svelte";
import type { SessionSummary } from "../../lib/types";
import { setViewedAgent } from "../../lib/viewed-agent";
import { ws } from "../../lib/ws.svelte";
import { registerAppActions } from "../../shell/app-actions.svelte";
import ChatPlace from "./ChatPlace.svelte";

class NoObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

function agent(name: string, overrides: Partial<AgentSummary> = {}): AgentSummary {
  return {
    name,
    state: "running",
    last_error: null,
    autostart: true,
    role: null,
    a2a_visibility: "private",
    ...overrides,
  };
}

function liveRun(runId: string): SessionSummary {
  return {
    address: `spawned-${runId}`,
    run_id: runId,
    category: "spawned",
    source_label: "agent:researcher",
    state: "running",
    spawner: "main",
    depth: 1,
    purpose: "",
    started_at: "2026-03-14T11:00:00Z",
    completed_at: null,
    episode_id: null,
    interrupted: false,
    usage: { input_tokens: 0, output_tokens: 0, context_tokens: null, tool_calls: 0 },
    outcome: null,
    error: null,
    error_details: null,
    overlap: null,
  };
}

let unregister: () => void = () => {};

beforeEach(() => {
  vi.stubGlobal("IntersectionObserver", NoObserver);
  vi.stubGlobal("ResizeObserver", NoObserver);
  stubWebSocket();
  mockFetch(() => jsonResponse({}, 404));
  hub.handleFrame(
    snapshot([
      agent("atlas", { role: "Keeps the team wiki tidy" }),
      agent("drifter", { state: "stopped" }),
      agent("scout"),
    ]),
  );
  unregister = registerAppActions({
    openSearch: vi.fn(),
    openSettings: vi.fn(),
    openShortcuts: vi.fn(),
    openNotifications: vi.fn(),
    openFeedback: vi.fn(),
    createAgent: vi.fn(),
    addInboxNote: vi.fn(),
  });
});

afterEach(() => {
  unregister();
  setViewedAgent(null);
});

describe("the chat header", () => {
  it("names the agent with its role, and opens its settings", async () => {
    setViewedAgent("atlas");
    const openSettings = vi.spyOn(router, "openSettings").mockResolvedValue(true);
    render(ChatPlace, { agent: "atlas" });

    expect(screen.getByRole("heading", { level: 1, name: "atlas" })).toBeInTheDocument();
    expect(screen.getByText("Keeps the team wiki tidy")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "atlas settings" }));
    expect(openSettings).toHaveBeenCalledWith({ scope: "atlas", section: null });
  });

  it("counts the agent's running sessions, and opens Activity from them", async () => {
    setViewedAgent("atlas");
    const openPlace = vi.spyOn(router, "openPlace").mockResolvedValue(true);
    render(ChatPlace, { agent: "atlas" });
    expect(screen.queryByRole("button", { name: /running/ })).toBeNull();

    ws.sessions.live = [liveRun("run-1"), liveRun("run-2")];
    await settle();
    await userEvent.click(screen.getByRole("button", { name: "2 running, open Activity" }));

    expect(openPlace).toHaveBeenCalledWith({ kind: "activity", agent: "atlas" });
  });

  it("offers the conversation's size, Restart and Stop for a running agent", async () => {
    setViewedAgent("atlas");
    render(ChatPlace, { agent: "atlas" });
    await userEvent.click(screen.getByRole("button", { name: "More for atlas" }));

    for (const name of ["Show conversation size", "Restart atlas", "Stop atlas"]) {
      expect(screen.getByRole("menuitem", { name: new RegExp(`^${name}`) })).not.toHaveAttribute(
        "aria-disabled",
      );
    }
  });

  it("says why a stopped agent can't be restarted or stopped", async () => {
    setViewedAgent("drifter");
    render(ChatPlace, { agent: "drifter" });
    await userEvent.click(screen.getByRole("button", { name: "More for drifter" }));

    for (const name of ["Restart drifter", "Stop drifter"]) {
      const item = screen.getByRole("menuitem", { name: new RegExp(`^${name}`) });
      expect(item).toHaveAttribute("aria-disabled", "true");
      expect(item).toHaveTextContent("drifter isn't running");
    }
    expect(screen.getByRole("menuitem", { name: /^Show conversation size/ })).not.toHaveAttribute(
      "aria-disabled",
    );
  });
});

describe("the conversation", () => {
  it("shows one empty state once an empty history has loaded", async () => {
    setViewedAgent("atlas");
    render(ChatPlace, { agent: "atlas" });
    expect(screen.queryByText("No messages yet")).toBeNull();

    ws.store.loadHistory({ kind: "recent", messages: [], next_cursor: null });
    await settle();
    expect(screen.getAllByRole("heading", { name: "No messages yet" })).toHaveLength(1);
  });

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
      context_tokens: null,
    };
    render(ChatPlace, { agent: "scout" });
    await settle();
    expect(screen.getByText(/4 tool calls/)).toBeInTheDocument();
    expect(screen.getByText(/7 tool calls/)).toBeInTheDocument();

    setViewedAgent("atlas");
    await settle();
    expect(screen.queryByText(/4 tool calls/)).toBeNull();
    expect(screen.queryByText(/7 tool calls/)).toBeNull();
  });
});
