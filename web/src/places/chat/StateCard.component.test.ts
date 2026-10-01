import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { jsonResponse, mockFetch, render, screen, settle } from "../../test/component";
import { snapshot } from "../../test/hub-frames";
import { failureLine } from "../../lib/agent-failure";
import { hub } from "../../lib/hub.svelte";
import type { AgentErrorKind, AgentSummary } from "../../lib/hub-types";
import { router } from "../../lib/router.svelte";
import type { ShellActions } from "../../shell/shell-actions";
import StateCard from "./StateCard.svelte";

function failed(kind: AgentErrorKind): AgentSummary {
  return {
    name: "brittle",
    state: "failed",
    last_error: {
      message: "brittle couldn't start: config error. Fix its settings, then start it again.",
      kind,
      reason: "config error: providers.toml: model 'gpt-9' is not offered by provider 'openai'",
      at: "2026-03-14T11:58:00Z",
    },
    autostart: true,
    role: null,
    a2a_visibility: "private",
  };
}

const DRIFTER: AgentSummary = {
  name: "drifter",
  state: "stopped",
  last_error: null,
  autostart: false,
  role: null,
  a2a_visibility: "private",
};

let shell: ShellActions;
let requests: string[];

/** The buttons' names in order, leaving out the Details disclosure. */
function actionNames(): string[] {
  return screen
    .getAllByRole("button")
    .map((button) => button.textContent.trim())
    .filter((name) => name !== "Details");
}

beforeEach(() => {
  requests = [];
  shell = {
    openSearch: vi.fn(),
    openSettings: vi.fn(),
    openShortcuts: vi.fn(),
    openNotifications: vi.fn(),
    openFeedback: vi.fn(),
    createAgent: vi.fn(),
    addInboxNote: vi.fn(),
  };
  hub.handleFrame(snapshot([failed("config"), DRIFTER]));
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("a failed agent's card", () => {
  it("says in plain words why it couldn't start, with the error behind Details", async () => {
    render(StateCard, { agent: failed("config"), shown: "failed", alone: false, actions: shell });

    expect(screen.getByRole("region", { name: "brittle couldn't start" })).toHaveTextContent(
      failureLine("config"),
    );
    const reason = screen.getByText(/model 'gpt-9' is not offered/);
    expect(reason).not.toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Details" }));
    expect(reason).toBeVisible();
  });

  it.each([
    ["config", ["Fix settings", "Restart"]],
    ["port_conflict", ["Open Connections", "Restart"]],
    ["crash", ["Restart", "Report a bug"]],
    ["other", ["Restart", "Report a bug"]],
  ] as const)("leads with the fix for a %s failure", (kind, names) => {
    render(StateCard, { agent: failed(kind), shown: "failed", alone: false, actions: shell });
    expect(actionNames()).toEqual(names);
  });

  it("opens the Settings section the first diagnostic names for Fix settings", async () => {
    mockFetch((url, init) => {
      requests.push(`${init?.method ?? "GET"} ${url}`);
      if (url.endsWith("/raw")) return new Response("[models]\nmain = 'openai/gpt-9'\n");
      if (url.includes("/providers/validate")) {
        return jsonResponse({
          valid: false,
          diagnostics: [
            {
              severity: "error",
              message: "not offered",
              location: { kind: "path", path: "models.main" },
            },
          ],
        });
      }
      return jsonResponse({ valid: true });
    });
    const openSettings = vi.spyOn(router, "openSettings").mockResolvedValue(true);
    render(StateCard, { agent: failed("config"), shown: "failed", alone: false, actions: shell });

    await userEvent.click(screen.getByRole("button", { name: "Fix settings" }));
    await vi.waitFor(() => {
      expect(openSettings).toHaveBeenCalledWith({ scope: "brittle", section: "model" });
    });
  });

  it("opens Connections for a port conflict, and the bug report for a crash", async () => {
    const openSettings = vi.spyOn(router, "openSettings").mockResolvedValue(true);
    const view = render(StateCard, {
      agent: failed("port_conflict"),
      shown: "failed",
      alone: false,
      actions: shell,
    });
    await userEvent.click(screen.getByRole("button", { name: "Open Connections" }));
    expect(openSettings).toHaveBeenCalledWith({ scope: "brittle", section: "connections" });
    view.unmount();

    render(StateCard, { agent: failed("crash"), shown: "failed", alone: false, actions: shell });
    await userEvent.click(screen.getByRole("button", { name: "Report a bug" }));
    expect(shell.openFeedback).toHaveBeenCalledWith("bug");
  });

  it("says so when a restart from it fails again", async () => {
    mockFetch((url, init) => {
      requests.push(`${init?.method ?? "GET"} ${url}`);
      return jsonResponse(failed("config"));
    });
    render(StateCard, { agent: failed("config"), shown: "failed", alone: false, actions: shell });
    expect(screen.queryByText(/still couldn't start/)).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: "Restart brittle" }));

    expect(await screen.findByText("It still couldn't start after the restart.")).toBeVisible();
    expect(requests).toEqual(["POST /api/hub/agents/brittle/restart"]);
  });
});

describe("a stopped agent's card", () => {
  it("starts the agent, and keeps the keyboard on the card while it does", async () => {
    mockFetch((url, init) => {
      requests.push(`${init?.method ?? "GET"} ${url}`);
      return jsonResponse({ ...DRIFTER, state: "running" });
    });
    render(StateCard, { agent: DRIFTER, shown: "stopped", alone: true, actions: shell });

    await userEvent.click(screen.getByRole("button", { name: "Start drifter" }));
    expect(screen.getByRole("heading", { name: "drifter is stopped" })).toHaveFocus();
    await settle();
    expect(requests).toEqual(["POST /api/hub/agents/drifter/start"]);
  });

  it("turns Start automatically on", async () => {
    mockFetch((url, init) => {
      const body = typeof init?.body === "string" ? init.body : "";
      requests.push(`${init?.method ?? "GET"} ${url} ${body}`);
      return jsonResponse({ ...DRIFTER, autostart: true });
    });
    render(StateCard, { agent: DRIFTER, shown: "stopped", alone: true, actions: shell });
    const toggle = screen.getByRole("switch", { name: "Start automatically" });
    expect(toggle).toHaveAttribute("aria-checked", "false");

    await userEvent.click(toggle);
    await settle();
    expect(requests).toEqual(['PATCH /api/hub/agents/drifter {"autostart":true}']);
  });
});

describe("a starting or stopping agent's card", () => {
  it.each([
    ["starting", "Starting drifter"],
    ["stopping", "Stopping drifter"],
  ] as const)("shows %s as progress, with nothing to press", (shown, title) => {
    render(StateCard, { agent: DRIFTER, shown, alone: false, actions: shell });
    expect(screen.getByRole("status")).toHaveTextContent(title);
    expect(screen.getByRole("status")).toHaveTextContent("This usually takes a few seconds.");
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });
});
