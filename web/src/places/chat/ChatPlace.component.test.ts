import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import {
  fireEvent,
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  stubWebSocket,
} from "../../test/component";
import { snapshot } from "../../test/hub-frames";
import { actionRegistry } from "../../lib/action-registry.svelte";
import { saveDraft, saveDraftImages } from "../../lib/composer-drafts";
import { hub } from "../../lib/hub.svelte";
import { notifications } from "../../lib/notifications.svelte";
import type { AgentSummary } from "../../lib/hub-types";
import { router } from "../../lib/router.svelte";
import type { SessionSummary } from "../../lib/types";
import { setViewedAgent } from "../../lib/viewed-agent";
import { ws } from "../../lib/ws.svelte";
import { registerAppActions } from "../../shell/app-actions.svelte";
import type { ShellActions } from "../../shell/shell-actions";
import ChatPlace from "./ChatPlace.svelte";

class NoObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

function agent(name: string, overrides: Partial<AgentSummary> = {}): AgentSummary {
  return {
    name,
    display_name: name,
    state: "running",
    last_error: null,
    autostart: true,
    role: null,
    a2a_visibility: "private",
    teams_configured: false,
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
let shell: ShellActions;

beforeEach(() => {
  vi.stubGlobal("IntersectionObserver", NoObserver);
  vi.stubGlobal("ResizeObserver", NoObserver);
  // The composer's model control is a popover, not the phone's sheet.
  vi.stubGlobal("matchMedia", (media: string) => ({
    matches: false,
    media,
    addEventListener: () => {},
    removeEventListener: () => {},
  }));
  stubWebSocket();
  mockFetch(() => jsonResponse({}, 404));
  hub.handleFrame(
    snapshot([
      agent("atlas", { role: "Keeps the team wiki tidy" }),
      agent("drifter", { state: "stopped" }),
      agent("scout"),
    ]),
  );
  shell = {
    openSearch: vi.fn(),
    openSettings: vi.fn(),
    openShortcuts: vi.fn(),
    openNotifications: vi.fn(),
    openFeedback: vi.fn(),
    createAgent: vi.fn(),
    addInboxNote: vi.fn(),
  };
  unregister = registerAppActions(shell);
});

afterEach(() => {
  unregister();
  setViewedAgent(null);
});

describe("the chat header", () => {
  it("names the agent with its role, and opens its settings", async () => {
    setViewedAgent("atlas");
    const openSettings = vi.spyOn(router, "openSettings").mockResolvedValue(true);
    render(ChatPlace, { agent: "atlas", actions: shell });

    expect(screen.getByRole("heading", { level: 1, name: "atlas" })).toBeInTheDocument();
    expect(screen.getByText("Keeps the team wiki tidy")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "atlas settings" }));
    expect(openSettings).toHaveBeenCalledWith({ scope: "atlas", section: null });
  });

  it("counts the agent's running sessions, and opens Activity from them", async () => {
    setViewedAgent("atlas");
    const openPlace = vi.spyOn(router, "openPlace").mockResolvedValue(true);
    render(ChatPlace, { agent: "atlas", actions: shell });
    expect(screen.queryByRole("button", { name: /running/ })).toBeNull();

    ws.sessions.live = [liveRun("run-1"), liveRun("run-2")];
    await settle();
    await userEvent.click(screen.getByRole("button", { name: "2 running, open Activity" }));

    expect(openPlace).toHaveBeenCalledWith({ kind: "activity", agent: "atlas" });
  });

  it("offers the conversation's size, Restart and Stop for a running agent", async () => {
    setViewedAgent("atlas");
    render(ChatPlace, { agent: "atlas", actions: shell });
    await userEvent.click(screen.getByRole("button", { name: "More for atlas" }));

    for (const name of ["Show conversation size", "Restart atlas", "Stop atlas"]) {
      expect(screen.getByRole("menuitem", { name: new RegExp(`^${name}`) })).not.toHaveAttribute(
        "aria-disabled",
      );
    }
  });

  it("says why a stopped agent can't be restarted or stopped", async () => {
    setViewedAgent("drifter");
    render(ChatPlace, { agent: "drifter", actions: shell });
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
    render(ChatPlace, { agent: "atlas", actions: shell });
    expect(screen.queryByText("No messages yet")).toBeNull();

    ws.store.loadHistory({ kind: "recent", messages: [], next_cursor: null });
    await settle();
    expect(screen.getAllByRole("heading", { name: "No messages yet" })).toHaveLength(1);
  });

  it("shows a stopped agent's card in place of the composer, and no empty state", async () => {
    setViewedAgent("drifter");
    render(ChatPlace, { agent: "drifter", actions: shell });
    ws.store.loadHistory({ kind: "recent", messages: [], next_cursor: null });
    await settle();

    expect(screen.getByRole("region", { name: "drifter is stopped" })).toBeInTheDocument();
    expect(screen.queryByText("No messages yet")).toBeNull();
    expect(screen.queryByRole("combobox")).toBeNull();
    expect(screen.queryByText(/Reconnecting/)).toBeNull();
  });

  it("keeps the past conversation above the card", async () => {
    setViewedAgent("drifter");
    render(ChatPlace, { agent: "drifter", actions: shell });
    ws.store.loadHistory({
      kind: "recent",
      messages: [
        {
          role: "assistant",
          content: "The wiki index is tidy.",
          timestamp: "2026-03-14T11:00",
          visibility: "user",
        },
      ],
      next_cursor: null,
    });
    await settle();

    const conversation = screen.getByRole("region", { name: "Conversation with drifter" });
    expect(conversation).toHaveTextContent("The wiki index is tidy.");
    expect(screen.getByRole("region", { name: "drifter is stopped" })).toBeInTheDocument();
  });

  it("gives the composer back once the agent runs", async () => {
    setViewedAgent("drifter");
    render(ChatPlace, { agent: "drifter", actions: shell });
    await settle();
    expect(screen.queryByRole("combobox")).toBeNull();

    hub.handleFrame({ type: "agent_state", agent: agent("drifter", { state: "running" }) });
    await settle();
    expect(screen.queryByRole("region", { name: "drifter is stopped" })).toBeNull();
    expect(screen.getByRole("combobox")).toBeInTheDocument();
  });

  it("shows the live turn of the agent that is open, not the one that was", async () => {
    setViewedAgent("scout");
    ws.store.handleMessage({ type: "turn_started", reply_to: "t1" });
    ws.store.handleMessage({
      type: "tool_call",
      id: "c1",
      name: "memory_search",
      arguments: { query: "release notes" },
      server: null,
    });
    render(ChatPlace, { agent: "scout", actions: shell });
    await settle();
    expect(screen.getByText("Working")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /^Searching memory for “release notes”/ }),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "Stop reply" })).toBeInTheDocument();

    setViewedAgent("atlas");
    await settle();
    expect(screen.queryByText("Working")).toBeNull();
  });

  it("says what the agent is doing with its memory after a reply, until it's done", async () => {
    setViewedAgent("atlas");
    render(ChatPlace, { agent: "atlas", actions: shell });
    ws.store.handleMessage({ type: "post_turn_activity", kind: "memory", active: true });
    await settle();
    const status = screen.getByText("Noting what matters from this conversation");
    expect(status.closest("[role=status]")).not.toBeNull();

    ws.store.handleMessage({ type: "post_turn_activity", kind: "subconscious", active: true });
    await settle();
    expect(
      screen.getByText("Noting what matters from this conversation and reviewing the last reply"),
    ).toBeInTheDocument();

    ws.store.clearPostTurnActivity();
    await settle();
    expect(screen.queryByText(/Noting what matters/)).toBeNull();
  });
});

describe("stopping the reply", () => {
  function startTurn(): void {
    setViewedAgent("atlas");
    ws.store.handleMessage({ type: "turn_started", reply_to: "t1" });
  }

  it("stops it from the activity line, which says it is stopping", async () => {
    startTurn();
    const stop = vi.spyOn(ws, "stop");
    render(ChatPlace, { agent: "atlas", actions: shell });
    await settle();

    await userEvent.click(screen.getByRole("button", { name: "Stop the reply" }));
    expect(stop).toHaveBeenCalledOnce();
    expect(ws.store.observed.get("t1")?.stopAsked).toBe(true);
    expect(screen.getByRole("button", { name: "Stop the reply" })).toHaveTextContent("Stopping…");
  });

  it("stops it with a second Esc while the composer has focus", async () => {
    startTurn();
    const stop = vi.spyOn(ws, "stop");
    render(ChatPlace, { agent: "atlas", actions: shell });
    await settle();

    screen.getByRole("combobox").focus();
    await userEvent.keyboard("{Escape}");
    expect(stop).not.toHaveBeenCalled();
    expect(screen.getByText("Press Esc again to stop")).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    expect(stop).toHaveBeenCalledOnce();
    expect(screen.queryByText("Press Esc again to stop")).toBeNull();
  });

  it("leaves Esc to an open overlay, and does nothing between turns", async () => {
    setViewedAgent("atlas");
    const stop = vi.spyOn(ws, "stop");
    render(ChatPlace, { agent: "atlas", actions: shell });
    await settle();
    screen.getByRole("combobox").focus();
    await userEvent.keyboard("{Escape}");
    expect(stop).not.toHaveBeenCalled();

    ws.store.handleMessage({ type: "turn_started", reply_to: "t1" });
    await userEvent.click(screen.getByRole("button", { name: "More for atlas" }));
    expect(screen.getByRole("menu", { name: "More for atlas" })).toBeInTheDocument();
    screen.getByRole("combobox").focus();
    await userEvent.keyboard("{Escape}");
    expect(stop).not.toHaveBeenCalled();
    expect(screen.queryByRole("menu", { name: "More for atlas" })).toBeNull();
    expect(screen.queryByText("Press Esc again to stop")).toBeNull();
  });
});

describe("sending a line", () => {
  async function open(): Promise<HTMLElement> {
    Element.prototype.scrollIntoView = vi.fn();
    setViewedAgent("atlas");
    render(ChatPlace, { agent: "atlas", actions: shell });
    await settle();
    return screen.getByRole("combobox", { name: "Message atlas" });
  }

  afterEach(() => {
    saveDraft("atlas", "");
  });

  it("sends a pasted path as a message, though it starts with a slash", async () => {
    const box = await open();
    const sendChat = vi.spyOn(ws, "sendChat").mockImplementation(() => {});
    const surface = vi.spyOn(notifications, "surface");
    surface.mockClear();

    await userEvent.click(box);
    await userEvent.paste("/home/bear/logs/app.log has the error");
    await userEvent.keyboard("{Enter}");

    expect(sendChat).toHaveBeenCalledWith("/home/bear/logs/app.log has the error", undefined);
    expect(surface).not.toHaveBeenCalledWith("error", expect.stringContaining("run /"));
    expect(box).toHaveValue("");
  });

  it("sends a slash word that names no action as a message", async () => {
    const box = await open();
    const sendChat = vi.spyOn(ws, "sendChat").mockImplementation(() => {});

    await userEvent.type(box, "/nope at all{Enter}");

    expect(sendChat).toHaveBeenCalledWith("/nope at all", undefined);
  });

  it("runs the action a line names, with the rest as its text", async () => {
    const box = await open();
    const sendChat = vi.spyOn(ws, "sendChat").mockImplementation(() => {});
    const run = vi.spyOn(actionRegistry, "run").mockResolvedValue(true);

    await userEvent.type(box, "/inbox water the plants{Enter}");

    expect(sendChat).not.toHaveBeenCalled();
    expect(run).toHaveBeenCalledOnce();
    expect(run.mock.calls[0]?.[0].command).toBe("inbox");
    expect(run.mock.calls[0]?.[1]).toBe("water the plants");
    expect(box).toHaveValue("");
  });

  it("says why an action can't run now, and leaves the line in the box", async () => {
    const box = await open();
    const sendChat = vi.spyOn(ws, "sendChat").mockImplementation(() => {});
    const surface = vi.spyOn(notifications, "surface");

    await userEvent.type(box, "/stop now{Enter}");

    expect(sendChat).not.toHaveBeenCalled();
    expect(surface).toHaveBeenCalledWith("error", expect.stringMatching(/^Couldn't run \/stop: /));
    expect(box).toHaveValue("/stop now");
  });
});

describe("loading the history", () => {
  afterEach(() => {
    ws.historyError = null;
  });

  it("stands in for the conversation with a skeleton until it has loaded", async () => {
    setViewedAgent("atlas");
    render(ChatPlace, { agent: "atlas", actions: shell });
    await settle();
    expect(screen.getByText("Loading the conversation")).toBeInTheDocument();
    expect(screen.queryByText("No messages yet")).toBeNull();

    ws.store.loadHistory({ kind: "recent", messages: [], next_cursor: null });
    await settle();
    expect(screen.queryByText("Loading the conversation")).toBeNull();
  });

  it("says why it didn't load, in the conversation, and offers Retry", async () => {
    setViewedAgent("atlas");
    ws.historyError = "Residuum ran into a problem on its end.";
    const load = vi.spyOn(ws, "loadMainHistory").mockResolvedValue();
    render(ChatPlace, { agent: "atlas", actions: shell });
    await settle();

    const conversation = screen.getByRole("region", { name: "Conversation with atlas" });
    expect(conversation).toHaveTextContent("Couldn't load the conversation");
    expect(conversation).toHaveTextContent("Residuum ran into a problem on its end.");
    expect(screen.queryByText("Loading the conversation")).toBeNull();
    expect(screen.queryByText("No messages yet")).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(load).toHaveBeenCalledOnce();
  });
});

describe("dropping images", () => {
  const png = (name: string): File => new File(["png"], name, { type: "image/png" });
  const carrying = (...files: File[]): { dataTransfer: Partial<DataTransfer> } => ({
    dataTransfer: { types: ["Files"], files: files as unknown as FileList },
  });

  afterEach(() => {
    saveDraftImages("atlas", []);
  });

  it("takes images dropped anywhere on the place, with an overlay while they are over it", async () => {
    Element.prototype.scrollIntoView = vi.fn();
    setViewedAgent("atlas");
    render(ChatPlace, { agent: "atlas", actions: shell });
    await settle();
    const header = screen.getByRole("heading", { level: 1, name: "atlas" });
    expect(screen.queryByText("Drop images to attach")).toBeNull();

    // Over the header, which is nowhere near the composer.
    const taken = !(await fireEvent.dragOver(header, carrying(png("a.png"))));
    expect(taken).toBe(true);
    await settle();
    expect(screen.getByText("Drop images to attach")).toBeInTheDocument();

    await fireEvent.drop(header, carrying(png("a.png")));
    await settle();
    expect(screen.queryByText("Drop images to attach")).toBeNull();
    expect(await screen.findByRole("img", { name: "Image 1" })).toBeInTheDocument();
  });

  it("drops the overlay when the files leave the place", async () => {
    setViewedAgent("atlas");
    render(ChatPlace, { agent: "atlas", actions: shell });
    await settle();
    const header = screen.getByRole("heading", { level: 1, name: "atlas" });
    await fireEvent.dragOver(header, carrying(png("a.png")));
    await settle();
    expect(screen.getByText("Drop images to attach")).toBeInTheDocument();

    await fireEvent.dragLeave(header, { relatedTarget: null });
    await settle();
    expect(screen.queryByText("Drop images to attach")).toBeNull();
  });

  it("ignores a drag that isn't carrying files", async () => {
    setViewedAgent("atlas");
    render(ChatPlace, { agent: "atlas", actions: shell });
    await settle();
    const header = screen.getByRole("heading", { level: 1, name: "atlas" });
    const taken = !(await fireEvent.dragOver(header, { dataTransfer: { types: ["text/plain"] } }));
    expect(taken).toBe(false);
    expect(screen.queryByText("Drop images to attach")).toBeNull();
  });

  it("takes nothing while the agent has no composer, and leaves the drop for the app to refuse", async () => {
    setViewedAgent("drifter");
    render(ChatPlace, { agent: "drifter", actions: shell });
    await settle();
    const header = screen.getByRole("heading", { level: 1, name: "drifter" });
    const taken = !(await fireEvent.dragOver(header, carrying(png("a.png"))));
    expect(taken).toBe(false);
    expect(screen.queryByText("Drop images to attach")).toBeNull();
  });
});
