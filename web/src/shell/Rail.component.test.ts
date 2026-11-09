import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { tick } from "svelte";
import userEvent from "@testing-library/user-event";
import { render, screen, stubWebSocket } from "../test/component";
import { activityFrame, snapshot } from "../test/hub-frames";
import { hub } from "../lib/hub.svelte";
import { router } from "../lib/router.svelte";
import type { AgentSummary } from "../lib/hub-types";
import { RailAccordion } from "./accordion.svelte";
import { registerAppActions } from "./app-actions.svelte";
import Rail from "./Rail.svelte";
import type { ShellActions } from "./shell-actions";
import { waitFor } from "../test/wait";

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

function actions(): ShellActions {
  return {
    openSearch: vi.fn(),
    openSettings: vi.fn(),
    openShortcuts: vi.fn(),
    openNotifications: vi.fn(),
    openFeedback: vi.fn(),
    createAgent: vi.fn(),
  };
}

/** An agent row by the start of its name, or by its whole name as read: "atlas, running". */
const row = (name: RegExp | string): HTMLElement =>
  screen.getByRole("link", {
    name: (accessible) => {
      const read = accessible.replace(/\s+,/g, ",");
      return typeof name === "string" ? read === name : name.test(read);
    },
  });

const caret = (agentName: string): HTMLElement =>
  screen.getByRole("button", { name: new RegExp(`^(Expand|Collapse) ${agentName}$`) });

beforeEach(() => {
  stubWebSocket();
  hub.handleFrame(
    snapshot([agent("atlas"), agent("brittle", { state: "failed" }), agent("scout")], {
      activity: { scout: { busy: true, busy_since: null, unread: 4 } },
    }),
  );
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("Rail", () => {
  it("names each agent's state, and counts what needs the user beside Home", () => {
    render(Rail, { accordion: new RailAccordion(), actions: actions() });
    expect(row("atlas, running")).toBeTruthy();
    expect(row("brittle, failed")).toBeTruthy();
    expect(row("scout, running, working, 4 unread")).toBeTruthy();
    expect(screen.getByRole("link", { name: "Home 1 thing needs you" })).toBeTruthy();
  });

  it("opens an agent's Chat and expands its places when its name is clicked, without collapsing on a repeat click", async () => {
    const user = userEvent.setup();
    const navigate = vi.spyOn(router, "openPlace");
    render(Rail, { accordion: new RailAccordion(), actions: actions() });

    await user.click(row(/^atlas/));
    expect(navigate).toHaveBeenLastCalledWith({ kind: "chat", agent: "atlas" });
    expect(caret("atlas")).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("link", { name: "Activity" })).toHaveAttribute(
      "href",
      "/agent/atlas?panel=activity:atlas",
    );
    expect(screen.getByRole("link", { name: "Schedule" })).toHaveAttribute(
      "href",
      "/agent/atlas?panel=schedule:atlas",
    );
    expect(screen.getByRole("link", { name: "Files" })).toHaveAttribute(
      "href",
      "/agent/atlas?panel=files:atlas",
    );

    // Clicking the already-expanded agent's name again never collapses its places.
    await user.click(row(/^atlas/));
    expect(caret("atlas")).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("link", { name: "Files" })).toBeVisible();

    await user.click(row(/^scout/));
    expect(navigate).toHaveBeenLastCalledWith({ kind: "chat", agent: "scout" });
    expect(caret("atlas")).toHaveAttribute("aria-expanded", "false");
    expect(caret("scout")).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("link", { name: "Activity" })).toHaveAttribute(
      "href",
      "/agent/scout?panel=activity:scout",
    );
  });

  it("folds and unfolds an agent's places from its caret without navigating", async () => {
    const user = userEvent.setup();
    const navigate = vi.spyOn(router, "openPlace");
    render(Rail, { accordion: new RailAccordion(), actions: actions() });

    await user.click(caret("atlas"));
    expect(caret("atlas")).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("link", { name: "Activity" })).toBeVisible();

    await user.click(caret("atlas"));
    expect(caret("atlas")).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("link", { name: "Activity" })).toBeNull();
    expect(navigate).not.toHaveBeenCalled();
  });

  it("opens Activity, Schedule and Files in the context panel beside Chat", async () => {
    const user = userEvent.setup();
    const openPanel = vi.spyOn(router, "openPanel").mockResolvedValue(true);
    const openPlace = vi.spyOn(router, "openPlace").mockResolvedValue(true);
    const accordion = new RailAccordion();
    accordion.expand("atlas");
    router.location = { place: { kind: "chat", agent: "atlas" }, panel: null, settings: null };
    render(Rail, { accordion, actions: actions() });

    await user.click(screen.getByRole("link", { name: "Files" }));
    expect(openPanel).toHaveBeenCalledWith({ kind: "files", agent: "atlas" });
    expect(openPlace).not.toHaveBeenCalled();

    // From another place, clicking Files opens the agent's Chat with the Files panel.
    router.location = { place: { kind: "home" }, panel: null, settings: null };
    await tick();
    await user.click(screen.getByRole("link", { name: "Schedule" }));
    expect(openPlace).toHaveBeenCalledWith(
      { kind: "chat", agent: "atlas" },
      { panel: { kind: "schedule", agent: "atlas" } },
    );
  });

  it("moves between the rows that show with the arrow keys, Home and End", async () => {
    const user = userEvent.setup();
    const accordion = new RailAccordion();
    accordion.toggle("atlas");
    render(Rail, { accordion, actions: actions() });

    row(/^atlas/).focus();
    await user.keyboard("{ArrowDown}");
    expect(document.activeElement).toBe(screen.getByRole("link", { name: "Chat" }));
    await user.keyboard("{ArrowUp}{ArrowUp}");
    expect(document.activeElement).toBe(screen.getByRole("link", { name: /^Inbox/ }));
    await user.keyboard("{End}");
    expect(document.activeElement).toBe(screen.getByRole("link", { name: "Shared files" }));
    await user.keyboard("{Home}");
    expect(document.activeElement).toBe(screen.getByRole("link", { name: /^Home/ }));
  });

  it("follows the hub's frames: a reply raises the badge, and a stop shows Stopping", async () => {
    render(Rail, { accordion: new RailAccordion(), actions: actions() });
    hub.handleFrame(activityFrame("atlas", false, 2));
    hub.handleFrame({ type: "agent_stopping", name: "atlas" });
    await tick();
    expect(row("atlas, stopping, 2 unread")).toBeTruthy();
  });

  it("asks the shell for search, Settings, the help menu's items and a new agent", async () => {
    const user = userEvent.setup();
    const shell = actions();
    const unregister = registerAppActions(shell);
    render(Rail, { accordion: new RailAccordion(), actions: shell });

    await user.click(screen.getByRole("button", { name: /^Search or jump to/ }));
    expect(shell.openSearch).toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Create an agent" }));
    expect(shell.createAgent).toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Settings" }));
    expect(shell.openSettings).toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Help" }));
    await user.click(await screen.findByRole("menuitem", { name: "Recent notifications" }));
    await waitFor(() => {
      expect(shell.openNotifications).toHaveBeenCalled();
    });
    await user.click(screen.getByRole("button", { name: "Help" }));
    await user.click(await screen.findByRole("menuitem", { name: "Report a bug" }));
    await waitFor(() => {
      expect(shell.openFeedback).toHaveBeenCalledWith("bug");
    });
    unregister();
  });
});
