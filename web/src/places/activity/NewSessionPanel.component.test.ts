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
import NewSessionPanelHarness from "../../test/ui/NewSessionPanelHarness.svelte";
import { waitFor } from "../../test/wait";
import { hub } from "../../lib/hub.svelte";
import type { AgentSummary } from "../../lib/hub-types";
import { router } from "../../lib/router.svelte";
import type { SessionSummary } from "../../lib/types";
import { setViewedAgent } from "../../lib/viewed-agent";
import { ws } from "../../lib/ws.svelte";
import { NewSession } from "./new-session.svelte";

function agent(name: string, state: AgentSummary["state"] = "running"): AgentSummary {
  return {
    name,
    display_name: name,
    state,
    last_error: null,
    autostart: true,
    role: null,
    a2a_visibility: "private",
    teams_configured: false,
  };
}

function started(address: string): SessionSummary {
  return {
    address,
    run_id: "run-owner-1",
    category: "spawned",
    source_label: "owner:session",
    state: "forking",
    spawner: null,
    depth: 1,
    purpose: "Plan the garden",
    started_at: "2026-03-14T12:00:00Z",
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

let starts: unknown[];
let answer: () => Response;

beforeEach(() => {
  stubWebSocket();
  starts = [];
  answer = () => jsonResponse({ address: "spawned-session-1a2b" }, 202);
  mockFetch((url, init) => {
    if (url === "/api/agents/atlas/sessions" && init?.method === "POST") {
      if (typeof init.body === "string") starts.push(JSON.parse(init.body));
      return answer();
    }
    return jsonResponse({ live: [], completed: [], next_cursor: null });
  });
  hub.handleFrame(snapshot([agent("atlas"), agent("drifter", "stopped")]));
  setViewedAgent("atlas");
});

afterEach(() => {
  setViewedAgent(null);
});

function show(name = "atlas"): NewSession {
  const session = new NewSession(name);
  render(NewSessionPanelHarness, { session });
  return session;
}

describe("the new session panel", () => {
  it("says what a new session is, and runs it on the medium model unless asked", async () => {
    show();
    expect(screen.getByRole("heading", { name: "New session" })).toBeVisible();
    expect(
      screen.getByText(/^Starts a clean session on atlas\. It doesn't see your conversation/),
    ).toBeVisible();
    expect(screen.getByRole("radio", { name: "Medium" })).toBeChecked();

    await userEvent.type(
      screen.getByRole("textbox", { name: "Task for the new session" }),
      "Plan the garden{Enter}",
    );
    await waitFor(() => {
      expect(starts).toEqual([{ prompt: "Plan the garden", model: "medium" }]);
    });
  });

  it("starts on the chosen model, then hands over to the run once it shows up", async () => {
    const replacePanel = vi.spyOn(router, "replacePanel").mockResolvedValue(true);
    const session = show();
    await userEvent.click(screen.getByRole("radio", { name: "Large" }));
    await userEvent.type(
      screen.getByRole("textbox", { name: "Task for the new session" }),
      "Plan the garden",
    );
    await userEvent.click(screen.getByRole("button", { name: "Start the session" }));

    expect(await screen.findByText("Starting")).toBeVisible();
    expect(starts).toEqual([{ prompt: "Plan the garden", model: "large" }]);
    expect(session.prompt).toBe("");
    expect(replacePanel).not.toHaveBeenCalled();

    ws.sessions.handleFrame({ type: "session_started", session: started("spawned-session-1a2b") });
    await waitFor(() => {
      expect(replacePanel).toHaveBeenCalledWith({
        kind: "session",
        agent: "atlas",
        runId: "run-owner-1",
      });
    });
  });

  it("keeps the task and says why when the session couldn't start", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    answer = () => jsonResponse({ error: "invalid model tier" }, 400);
    const session = show();
    await userEvent.type(
      screen.getByRole("textbox", { name: "Task for the new session" }),
      "Plan the garden{Enter}",
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Couldn't start the session. Invalid model tier.",
    );
    expect(session.prompt).toBe("Plan the garden");
    expect(screen.getByRole("textbox", { name: "Task for the new session" })).toHaveValue(
      "Plan the garden",
    );
    expect(screen.queryByText("Starting")).toBeNull();
  });

  it("asks for the agent to be started first", async () => {
    hub.handleFrame(snapshot([agent("atlas", "stopped")]));
    show();
    await settle();
    expect(screen.getByRole("textbox", { name: "Task for the new session" })).toBeDisabled();
    expect(screen.getByText("Start atlas first.")).toBeVisible();
    expect(starts).toEqual([]);
  });
});
