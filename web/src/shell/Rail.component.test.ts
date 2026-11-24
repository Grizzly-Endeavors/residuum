import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { tick } from "svelte";
import userEvent from "@testing-library/user-event";
import { render, screen, stubWebSocket } from "../test/component";
import { activityFrame, snapshot } from "../test/hub-frames";
import { actionRegistry } from "../lib/action-registry.svelte";
import { hub } from "../lib/hub.svelte";
import { router } from "../lib/router.svelte";
import { HOME, locationAt } from "../lib/routes";
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

const row = (name: RegExp | string): HTMLElement =>
  screen.getByRole("link", {
    name: (accessible) => {
      const read = accessible.replace(/\s+,/g, ",");
      return typeof name === "string" ? read === name : name.test(read);
    },
  });

const caret = (agentName: string): HTMLElement =>
  screen.getByRole("button", { name: `${agentName} places` });

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
  router.location = locationAt(HOME);
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
    const navigate = vi.spyOn(router, "open");
    render(Rail, { accordion: new RailAccordion(), actions: actions() });

    await user.click(row(/^atlas/));
    expect(navigate).toHaveBeenLastCalledWith(locationAt({ kind: "chat", agent: "atlas" }));
    expect(caret("atlas")).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("link", { name: "Activity" })).toHaveAttribute(
      "href",
      "/agent/atlas/activity",
    );
    expect(screen.getByRole("link", { name: "Schedule" })).toHaveAttribute(
      "href",
      "/agent/atlas/schedule",
    );
    expect(screen.getByRole("link", { name: "Files" })).toHaveAttribute(
      "href",
      "/agent/atlas/files",
    );

    await user.click(row(/^atlas/));
    expect(caret("atlas")).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("link", { name: "Files" })).toBeVisible();

    await user.click(row(/^scout/));
    expect(navigate).toHaveBeenLastCalledWith(locationAt({ kind: "chat", agent: "scout" }));
    expect(caret("atlas")).toHaveAttribute("aria-expanded", "false");
    expect(caret("scout")).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("link", { name: "Activity" })).toHaveAttribute(
      "href",
      "/agent/scout/activity",
    );
  });

  it("folds and unfolds an agent's places from its caret without navigating", async () => {
    const user = userEvent.setup();
    const navigate = vi.spyOn(router, "open");
    render(Rail, { accordion: new RailAccordion(), actions: actions() });

    await user.click(caret("atlas"));
    expect(caret("atlas")).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("link", { name: "Activity" })).toBeVisible();

    await user.click(caret("atlas"));
    expect(caret("atlas")).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("link", { name: "Activity" })).toBeNull();
    expect(navigate).not.toHaveBeenCalled();
  });

  it("opens Activity, Schedule and Files as pages, or beside the agent's Chat from their panel buttons", async () => {
    const user = userEvent.setup();
    const openPlace = vi.spyOn(router, "open").mockResolvedValue(true);
    const accordion = new RailAccordion();
    accordion.expand("atlas");
    render(Rail, { accordion, actions: actions() });

    await user.click(screen.getByRole("link", { name: "Schedule" }));
    expect(openPlace).toHaveBeenLastCalledWith(locationAt({ kind: "schedule", agent: "atlas" }));

    await user.click(screen.getByRole("button", { name: "Open Files beside chat" }));
    expect(openPlace).toHaveBeenLastCalledWith({
      place: { kind: "chat", agent: "atlas" },
      panel: { kind: "files", agent: "atlas" },
      settings: null,
    });
    expect(screen.getByRole("button", { name: "Open Activity beside chat" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Open Schedule beside chat" })).toBeTruthy();
  });

  it("marks a place current when its page is open or it shows beside the agent's Chat", async () => {
    const accordion = new RailAccordion();
    accordion.expand("atlas");
    router.location = locationAt({ kind: "activity", agent: "atlas" });
    render(Rail, { accordion, actions: actions() });
    const current = (): string[] =>
      ["Activity", "Schedule", "Files"].filter(
        (name) => screen.getByRole("link", { name }).getAttribute("aria-current") === "page",
      );
    expect(current()).toEqual(["Activity"]);

    router.location = {
      place: { kind: "chat", agent: "atlas" },
      panel: { kind: "files", agent: "atlas" },
      settings: null,
    };
    await tick();
    expect(current()).toEqual(["Files"]);
    expect(screen.getByRole("link", { name: "Files" }).parentElement).toHaveAttribute(
      "data-current",
    );

    router.location = locationAt({ kind: "chat", agent: "atlas" });
    await tick();
    expect(current()).toEqual([]);
  });

  it("marks the agent's name current while its Chat is the page, with or without a panel beside it", async () => {
    const accordion = new RailAccordion();
    accordion.expand("atlas");
    router.location = locationAt({ kind: "chat", agent: "atlas" });
    render(Rail, { accordion, actions: actions() });
    expect(row(/^atlas/)).toHaveAttribute("aria-current", "page");
    expect(row(/^scout/)).not.toHaveAttribute("aria-current");

    router.location = {
      place: { kind: "chat", agent: "atlas" },
      panel: { kind: "files", agent: "atlas" },
      settings: null,
    };
    await tick();
    expect(row(/^atlas/)).toHaveAttribute("aria-current", "page");

    router.location = locationAt({ kind: "activity", agent: "atlas" });
    await tick();
    expect(row(/^atlas/)).not.toHaveAttribute("aria-current");
  });

  it("leaves the beside-chat buttons out of the phone drawer and out of the arrow-key rows", async () => {
    const user = userEvent.setup();
    const accordion = new RailAccordion();
    accordion.expand("atlas");
    const { unmount } = render(Rail, { accordion, actions: actions() });

    screen.getByRole("link", { name: "Activity" }).focus();
    await user.keyboard("{ArrowDown}");
    expect(document.activeElement).toBe(screen.getByRole("link", { name: "Schedule" }));
    screen.getByRole("link", { name: "Activity" }).focus();
    await user.tab();
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Open Activity beside chat" }),
    );
    unmount();

    render(Rail, { accordion, actions: actions(), onclose: vi.fn() });
    expect(screen.getByRole("link", { name: "Activity" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /beside chat$/ })).toBeNull();
  });

  it("moves between the rows that show with the arrow keys, Home and End", async () => {
    const user = userEvent.setup();
    const accordion = new RailAccordion();
    accordion.toggle("atlas");
    render(Rail, { accordion, actions: actions() });

    row(/^atlas/).focus();
    await user.keyboard("{ArrowDown}");
    expect(document.activeElement).toBe(screen.getByRole("link", { name: "Activity" }));
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

  it("opens an agent's places as pages from the command palette", () => {
    const openPlace = vi.spyOn(router, "open").mockResolvedValue(true);
    const unregister = registerAppActions(actions());
    const run = (id: string): void => {
      const action = actionRegistry.all.find((candidate) => candidate.id === id);
      expect(action).toBeDefined();
      action?.run();
    };

    run("place:atlas:files");
    expect(openPlace).toHaveBeenLastCalledWith(locationAt({ kind: "files", agent: "atlas" }));
    run("place:atlas:activity");
    expect(openPlace).toHaveBeenLastCalledWith(locationAt({ kind: "activity", agent: "atlas" }));
    run("place:atlas:chat");
    expect(openPlace).toHaveBeenLastCalledWith(locationAt({ kind: "chat", agent: "atlas" }));
    unregister();
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
