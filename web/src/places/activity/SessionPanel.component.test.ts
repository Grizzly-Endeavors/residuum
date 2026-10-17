import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { jsonResponse, mockFetch, render, screen, settle, stubPointer } from "../../test/component";
import { actionRegistry } from "../../lib/action-registry.svelte";
import { snapshot } from "../../test/hub-frames";
import SessionPanelHarness from "../../test/ui/SessionPanelHarness.svelte";
import { hub } from "../../lib/hub.svelte";
import type { AgentSummary } from "../../lib/hub-types";
import { SessionRun } from "../../lib/session-run.svelte";
import type { SessionSummary } from "../../lib/types";

class NoObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

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

function summary(overrides: Partial<SessionSummary> = {}): SessionSummary {
  return {
    address: "spawned-research-3f9a",
    run_id: "run-1",
    category: "spawned",
    source_label: "agent:researcher",
    state: "idle",
    spawner: "main",
    depth: 2,
    purpose: "Compare fallback strategies",
    started_at: "2026-03-14T11:00:00Z",
    completed_at: null,
    episode_id: null,
    interrupted: false,
    usage: { input_tokens: 0, output_tokens: 0, context_tokens: null, tool_calls: 0 },
    outcome: null,
    error: null,
    error_details: null,
    overlap: null,
    model: null,
    resolved_model: null,
    ...overrides,
  };
}

const relay = { connected: false, send: () => {}, onFrame: () => () => {} };

/** The session routes the panel called, other than reading the transcript. */
let calls: string[];

async function show(session: SessionSummary): Promise<SessionRun> {
  mockFetch((url, init) => {
    if (url.endsWith("/transcript")) return jsonResponse({ session, messages: [] });
    calls.push(`${init?.method ?? "GET"} ${url}`);
    return jsonResponse({ outcome: "live" });
  });
  const run = new SessionRun("atlas", session.run_id, { summary: session }, relay);
  render(SessionPanelHarness, { run });
  await run.load();
  await settle();
  return run;
}

/** Whether the device reports a coarse pointer, as a touch screen does. */
let coarsePointer = false;
let unregister: () => void = () => {};

beforeEach(() => {
  calls = [];
  coarsePointer = false;
  stubPointer(() => coarsePointer);
  Element.prototype.scrollIntoView = vi.fn();
  vi.stubGlobal("IntersectionObserver", NoObserver);
  vi.stubGlobal("ResizeObserver", NoObserver);
  hub.handleFrame(snapshot([agent("atlas"), agent("drifter", "stopped")]));
  // A chat action, which acts on the main conversation and so isn't a session's.
  unregister = actionRegistry.register("test", () => [
    {
      id: "chat:observe",
      group: "Actions",
      label: "Summarize older messages now",
      icon: "layers",
      command: "observe",
      run: vi.fn(),
    },
  ]);
});

afterEach(() => {
  unregister();
});

describe("the session panel", () => {
  it("names the run, how it started and how it's doing, with its details", async () => {
    await show(summary());
    expect(screen.getByRole("heading", { name: "Compare fallback strategies" })).toBeVisible();
    expect(screen.getByText("Started by atlas")).toBeVisible();
    expect(screen.getByText(/^Idle, /)).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Details" }));
    expect(screen.getByText("atlas's conversation")).toBeVisible();
    expect(screen.getByText("2 levels below the conversation")).toBeVisible();
    expect(screen.getByText("0 tokens in, 0 out, 0 tool calls")).toBeVisible();
    expect(screen.getByText("run-1")).toBeVisible();
    expect(screen.getByRole("button", { name: "Stop" })).toBeEnabled();
  });

  it("names the model the run ran on in its details", async () => {
    await show(summary({ model: "small", resolved_model: "anthropic/claude-haiku-4-5" }));
    await userEvent.click(screen.getByRole("button", { name: "Details" }));
    expect(screen.getByText("Model", { selector: "dt" }).nextElementSibling).toHaveTextContent(
      "anthropic/claude-haiku-4-5, from the small tier",
    );
  });

  it("says the owner started a run, and when it forked the conversation", async () => {
    await show(
      summary({
        address: "spawned-multitask-1a2b",
        source_label: "owner:multitask",
        spawner: null,
        depth: 1,
      }),
    );
    expect(screen.getByText("Started by you")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Details" }));
    expect(screen.getByText("You, forking atlas's conversation with /multitask")).toBeVisible();
  });

  it("says plainly that the owner started a clean run", async () => {
    await show(summary({ source_label: "owner:session", spawner: null, depth: 1 }));
    await userEvent.click(screen.getByRole("button", { name: "Details" }));
    expect(screen.getByText("Started by", { selector: "dt" }).nextElementSibling).toHaveTextContent(
      /^You$/,
    );
  });

  it("shows how a finished run failed, and offers to start it again", async () => {
    await show(
      summary({
        state: "completed",
        outcome: "failed",
        error: "the site timed out",
        error_details: "GET https://example.com: timed out after 30s",
      }),
    );
    expect(screen.getByText("the site timed out")).toBeVisible();
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    expect(screen.getByRole("combobox", { name: "Message this session" })).toHaveAttribute(
      "placeholder",
      "Message it to start it again…",
    );
    expect(screen.getByText("No messages in this run yet.")).toBeVisible();
  });

  it("sends with Enter, keeping Shift+Enter for a new line", async () => {
    const run = await show(summary());
    const box = screen.getByRole("combobox", { name: "Message this session" });
    await userEvent.type(box, "Weigh safety{Shift>}{Enter}{/Shift}over speed{Enter}");
    await settle();
    expect(run.waiting).toMatchObject([{ content: "Weigh safety\nover speed" }]);
    expect(run.draft).toBe("");
  });

  it("shows a sent message waiting for the session to read it, with Dismiss for it", async () => {
    const run = await show(summary());
    run.draft = "Check the archive too";
    await run.send();
    await settle();
    const waiting = screen.getByRole("region", { name: "Waiting for this session to read" });
    expect(waiting).toHaveTextContent("Check the archive too");
    expect(screen.queryByRole("button", { name: "Send now" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Multitask" })).toBeNull();

    const dismiss = screen.getByRole("button", { name: "Dismiss" });
    expect(dismiss).toHaveAccessibleDescription("Check the archive too");
    await userEvent.click(dismiss);
    await settle();
    expect(run.waiting).toEqual([]);
    expect(screen.queryByRole("region", { name: "Waiting for this session to read" })).toBeNull();
  });

  it("asks for the agent to be started before it can be messaged or stopped", async () => {
    hub.handleFrame(snapshot([agent("atlas", "stopped")]));
    await show(summary());
    expect(screen.getByRole("combobox", { name: "Message this session" })).toBeDisabled();
    expect(screen.getByText("Start atlas first.")).toBeVisible();
    expect(screen.getByRole("button", { name: "Stop" })).toBeDisabled();
  });

  it("floats the message box over the transcript's foot", async () => {
    await show(summary());
    const box = screen.getByRole("combobox", { name: "Message this session" });
    expect(box.closest(".feed-dock")).not.toBeNull();
  });
});

describe("the session's message box on a touch screen", () => {
  it("makes Enter a new line, and only Send sends", async () => {
    coarsePointer = true;
    const run = await show(summary());
    const box = screen.getByRole("combobox", { name: "Message this session" });
    expect(box).toHaveAttribute("enterkeyhint", "enter");
    await userEvent.type(box, "Weigh safety{Enter}over speed");
    expect(box).toHaveValue("Weigh safety\nover speed");
    expect(calls).toEqual([]);

    await userEvent.click(screen.getByRole("button", { name: "Send to this session" }));
    await settle();
    expect(calls).toEqual(["POST /api/agents/atlas/sessions/spawned-research-3f9a/messages"]);
    expect(run.draft).toBe("");
    // The press leaves the keyboard in the box.
    expect(box).toHaveFocus();
  });

  it("asks for no Enter key hint where there is a keyboard", async () => {
    await show(summary());
    expect(screen.getByRole("combobox", { name: "Message this session" })).not.toHaveAttribute(
      "enterkeyhint",
    );
  });
});

describe("the session's / menu", () => {
  it("lists only the session's own actions, and Enter runs one", async () => {
    const run = await show(summary());
    const box = screen.getByRole("combobox", { name: "Message this session" });
    expect(box).toHaveAttribute("aria-expanded", "false");
    await userEvent.type(box, "/");
    const menu = screen.getByRole("listbox", { name: "Session actions" });
    expect(box).toHaveAttribute("aria-expanded", "true");
    expect(box).toHaveAttribute("aria-controls", menu.id);
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual([
      expect.stringContaining("Stop this session"),
    ]);

    await userEvent.keyboard("{Enter}");
    await settle();
    expect(calls).toEqual(["POST /api/agents/atlas/sessions/spawned-research-3f9a/stop"]);
    expect(run.stopping).toBe(true);
    expect(box).toHaveValue("");
  });

  it("opens from its button, and says why Stop can't run on a finished run", async () => {
    await show(summary({ state: "completed", outcome: "completed" }));
    await userEvent.click(screen.getByRole("button", { name: "Session actions" }));
    const stop = screen.getByRole("option", { name: /Stop this session/ });
    expect(stop).toHaveTextContent("This session isn't running");
    await userEvent.click(stop);
    expect(calls).toEqual([]);
  });

  it("runs a typed /stop sent with the button, and sends any other / line as a message", async () => {
    coarsePointer = true;
    await show(summary());
    const box = screen.getByRole("combobox", { name: "Message this session" });
    const send = screen.getByRole("button", { name: "Send to this session" });

    await userEvent.type(box, "/observe the logs");
    await userEvent.click(send);
    await settle();
    expect(calls).toEqual(["POST /api/agents/atlas/sessions/spawned-research-3f9a/messages"]);

    await userEvent.type(box, "/stop");
    await userEvent.click(send);
    await settle();
    expect(calls).toEqual([
      "POST /api/agents/atlas/sessions/spawned-research-3f9a/messages",
      "POST /api/agents/atlas/sessions/spawned-research-3f9a/stop",
    ]);
    expect(box).toHaveValue("");
  });
});
