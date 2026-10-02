import { describe, expect, it } from "vitest";
import type { AgentActivity, AgentSummary } from "../lib/hub-types";
import { agentRowStatus, type AgentRowContext } from "./rail-model";

function agent(name: string, overrides: Partial<AgentSummary> = {}): AgentSummary {
  return {
    name,
    display_name: name,
    state: "running",
    last_error: null,
    autostart: true,
    role: null,
    a2a_visibility: "private",
    ...overrides,
  };
}

const IDLE: AgentActivity = { busy: false, busy_since: null, unread: 0 };
const BUSY: AgentActivity = { busy: true, busy_since: "2026-03-14T12:00:00Z", unread: 0 };

function context(overrides: Partial<AgentRowContext> = {}): AgentRowContext {
  return { activity: IDLE, stopping: false, viewed: false, onItsChat: false, ...overrides };
}

describe("agentRowStatus", () => {
  it("shows nothing after a running, idle agent", () => {
    expect(agentRowStatus(agent("atlas"), context())).toEqual({
      dot: "running",
      working: false,
      tail: { kind: "none" },
      spoken: "running",
    });
  });

  it("puts a word after an agent that isn't running", () => {
    expect(agentRowStatus(agent("drifter", { state: "stopped" }), context()).tail).toEqual({
      kind: "word",
      word: "Stopped",
      tone: "quiet",
    });
    expect(agentRowStatus(agent("brittle", { state: "failed" }), context()).tail).toEqual({
      kind: "word",
      word: "Failed",
      tone: "danger",
    });
    expect(agentRowStatus(agent("nova", { state: "starting" }), context()).tail).toEqual({
      kind: "word",
      word: "Starting",
      tone: "quiet",
    });
  });

  it("reads a running agent the hub is stopping as stopping, and never as working", () => {
    const status = agentRowStatus(agent("atlas"), context({ stopping: true, activity: BUSY }));
    expect(status.dot).toBe("stopping");
    expect(status.working).toBe(false);
    expect(status.tail).toEqual({ kind: "word", word: "Stopping", tone: "quiet" });
    expect(status.spoken).toBe("stopping");
  });

  it("says Working for a busy agent the user isn't viewing, and only pulses for the viewed one", () => {
    const elsewhere = agentRowStatus(agent("scout"), context({ activity: BUSY }));
    expect(elsewhere.working).toBe(true);
    expect(elsewhere.tail).toEqual({ kind: "word", word: "Working", tone: "accent" });
    expect(elsewhere.spoken).toBe("running, working");

    const viewed = agentRowStatus(agent("scout"), context({ activity: BUSY, viewed: true }));
    expect(viewed.working).toBe(true);
    expect(viewed.tail).toEqual({ kind: "none" });
  });

  it("lets unread replies take the place of any word", () => {
    const activity = { ...BUSY, unread: 120 };
    const status = agentRowStatus(agent("scout"), context({ activity }));
    expect(status.tail).toEqual({ kind: "unread", count: 120 });
    expect(status.spoken).toBe("running, working, 120 unread");

    const failed = agentRowStatus(
      agent("brittle", { state: "failed" }),
      context({ activity: { ...IDLE, unread: 2 } }),
    );
    expect(failed.tail).toEqual({ kind: "unread", count: 2 });
  });

  it("leaves out the unread count while the agent's chat is the place shown", () => {
    const activity = { ...IDLE, unread: 3 };
    const status = agentRowStatus(
      agent("scout"),
      context({ activity, viewed: true, onItsChat: true }),
    );
    expect(status.tail).toEqual({ kind: "none" });
    expect(status.spoken).toBe("running");
  });
});
