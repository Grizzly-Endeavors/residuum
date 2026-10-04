import { beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { jsonResponse, mockFetch, render, screen, settle } from "../../test/component";
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
    ...overrides,
  };
}

const relay = { connected: false, send: () => {}, onFrame: () => () => {} };

async function show(session: SessionSummary): Promise<SessionRun> {
  mockFetch((url) =>
    url.endsWith("/transcript")
      ? jsonResponse({ session, messages: [] })
      : jsonResponse({ outcome: "live" }),
  );
  const run = new SessionRun("atlas", session.run_id, { summary: session }, relay);
  render(SessionPanelHarness, { run });
  await run.load();
  await settle();
  return run;
}

beforeEach(() => {
  vi.stubGlobal("IntersectionObserver", NoObserver);
  vi.stubGlobal("ResizeObserver", NoObserver);
  hub.handleFrame(snapshot([agent("atlas"), agent("drifter", "stopped")]));
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
    expect(screen.getByRole("textbox", { name: "Message this session" })).toHaveAttribute(
      "placeholder",
      "Message it to start it again…",
    );
    expect(screen.getByText("No messages in this run yet.")).toBeVisible();
  });

  it("sends with Enter, keeping Shift+Enter for a new line", async () => {
    const run = await show(summary());
    const box = screen.getByRole("textbox", { name: "Message this session" });
    await userEvent.type(box, "Weigh safety{Shift>}{Enter}{/Shift}over speed{Enter}");
    await settle();
    expect(run.items.find((item) => item.kind === "user")).toMatchObject({
      content: "Weigh safety\nover speed",
    });
    expect(run.draft).toBe("");
  });

  it("asks for the agent to be started before it can be messaged or stopped", async () => {
    hub.handleFrame(snapshot([agent("atlas", "stopped")]));
    await show(summary());
    expect(screen.getByRole("textbox", { name: "Message this session" })).toBeDisabled();
    expect(screen.getByText("Start atlas first.")).toBeVisible();
    expect(screen.getByRole("button", { name: "Stop" })).toBeDisabled();
  });
});
