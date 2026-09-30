import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "../test/component";
import AgentSwitcher from "./AgentSwitcher.svelte";
import { hub } from "../lib/hub.svelte";
import { router } from "../lib/router.svelte";
import type { AgentSummary } from "../lib/hub-types";

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

beforeEach(() => {
  hub.handleFrame({
    type: "agents_snapshot",
    agents: [
      agent("atlas"),
      agent("brittle", {
        state: "failed",
        last_error: { message: "providers.toml is missing", at: "2026-09-29T10:00:00Z" },
      }),
      agent("drifter", { state: "stopped" }),
      agent("scout"),
      agent("warm", { state: "starting" }),
    ],
  });
  router.agent = "scout";
  router.team = null;
});

afterEach(() => {
  hub.handleFrame({ type: "agents_snapshot", agents: [] });
  router.agent = null;
});

describe("AgentSwitcher", () => {
  it("lists every agent as a button whose name carries its state", () => {
    render(AgentSwitcher);
    expect(screen.getByRole("button", { name: "atlas, running" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "drifter, stopped" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "warm, starting" })).toBeTruthy();
    expect(screen.getByRole("button", { name: /^brittle, failed/ })).toBeTruthy();
  });

  it("writes out the state of agents that are not running, not only a colored shape", () => {
    render(AgentSwitcher);
    expect(screen.getByText("stopped")).toBeTruthy();
    expect(screen.getByText("starting")).toBeTruthy();
    expect(screen.getByText("failed")).toBeTruthy();
  });

  it("says when an agent is working and how many messages are unread", () => {
    hub.handleFrame({ type: "agent_activity", name: "atlas", busy: true, unread: 3 });
    render(AgentSwitcher);
    expect(screen.getByRole("button", { name: "atlas, running, working, 3 unread" })).toBeTruthy();
    expect(screen.getByText("3")).toBeTruthy();
  });

  it("caps a large unread count in the badge but not in the spoken label", () => {
    hub.handleFrame({ type: "agent_activity", name: "atlas", busy: false, unread: 150 });
    render(AgentSwitcher);
    expect(screen.getByText("99+")).toBeTruthy();
    expect(screen.getByRole("button", { name: "atlas, running, 150 unread" })).toBeTruthy();
  });

  it("marks the current agent", () => {
    render(AgentSwitcher);
    expect(screen.getByRole("button", { name: "scout, running" })).toHaveAttribute(
      "aria-current",
      "true",
    );
    expect(screen.getByRole("button", { name: "atlas, running" })).not.toHaveAttribute(
      "aria-current",
    );
  });

  it("marks Team, not an agent, as current on team pages and hub settings", () => {
    router.team = "overview";
    const { unmount } = render(AgentSwitcher);
    expect(screen.getByRole("button", { name: "Team" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("button", { name: "scout, running" })).not.toHaveAttribute(
      "aria-current",
    );
    unmount();
    router.team = null;
    router.settings = { scope: "hub", section: "a2a" };
    render(AgentSwitcher);
    expect(screen.getByRole("button", { name: "Team" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("button", { name: "scout, running" })).not.toHaveAttribute(
      "aria-current",
    );
    router.settings = null;
  });

  it("navigates to the agent that is clicked", async () => {
    const open = vi.spyOn(router, "openAgent").mockImplementation(() => {});
    render(AgentSwitcher);
    await fireEvent.click(screen.getByRole("button", { name: "atlas, running" }));
    expect(open).toHaveBeenCalledWith("atlas");
  });

  it("opens the team view from its own button", async () => {
    const open = vi.spyOn(router, "openTeam").mockImplementation(() => {});
    render(AgentSwitcher);
    await fireEvent.click(screen.getByRole("button", { name: "Team" }));
    expect(open).toHaveBeenCalledWith("overview");
  });

  it("moves focus between agents with the arrow keys", async () => {
    render(AgentSwitcher);
    const atlas = screen.getByRole("button", { name: "atlas, running" });
    atlas.focus();
    await fireEvent.keyDown(atlas, { key: "ArrowRight" });
    expect(screen.getByRole("button", { name: /^brittle/ })).toHaveFocus();
    await fireEvent.keyDown(document.activeElement as Element, { key: "ArrowLeft" });
    expect(atlas).toHaveFocus();
    await fireEvent.keyDown(atlas, { key: "End" });
    expect(screen.getByRole("button", { name: "Team" })).toHaveFocus();
    await fireEvent.keyDown(document.activeElement as Element, { key: "Home" });
    expect(atlas).toHaveFocus();
  });

  it("shows a failed agent's last error on focus and hides it on blur", async () => {
    render(AgentSwitcher);
    const brittle = screen.getByRole("button", { name: /^brittle/ });
    expect(screen.queryByRole("tooltip")).toBeNull();
    brittle.focus();
    const tip = await screen.findByRole("tooltip");
    expect(tip).toHaveTextContent("providers.toml is missing");
    expect(brittle).toHaveAttribute("aria-describedby", tip.id);
    brittle.blur();
    await vi.waitFor(() => {
      expect(screen.queryByRole("tooltip")).toBeNull();
    });
  });

  it("shows a failed agent's last error on hover", async () => {
    render(AgentSwitcher);
    const brittle = screen.getByRole("button", { name: /^brittle/ });
    await fireEvent.pointerEnter(brittle);
    expect(await screen.findByRole("tooltip")).toHaveTextContent("providers.toml is missing");
    await fireEvent.pointerLeave(brittle);
    expect(screen.queryByRole("tooltip")).toBeNull();
  });

  it("dismisses the error with Escape", async () => {
    render(AgentSwitcher);
    const brittle = screen.getByRole("button", { name: /^brittle/ });
    brittle.focus();
    await screen.findByRole("tooltip");
    await fireEvent.keyDown(brittle, { key: "Escape" });
    expect(screen.queryByRole("tooltip")).toBeNull();
  });

  it("has no error popup for agents that have not failed", () => {
    render(AgentSwitcher);
    const atlas = screen.getByRole("button", { name: "atlas, running" });
    atlas.focus();
    expect(screen.queryByRole("tooltip")).toBeNull();
  });

  it("still names the agent in the URL before the hub has listed it", () => {
    router.agent = "newcomer";
    render(AgentSwitcher);
    expect(screen.getByRole("button", { name: /^newcomer/ })).toHaveAttribute(
      "aria-current",
      "true",
    );
  });
});
