import { describe, expect, it, vi } from "vitest";
import type { AppAction } from "./action-registry.svelte";
import { sessionActions, type SessionActionContext } from "./session-actions";

function context(overrides: Partial<SessionActionContext> = {}): SessionActionContext {
  return {
    agent: "atlas",
    agentReady: true,
    stoppable: true,
    stopping: false,
    stop: vi.fn(),
    ...overrides,
  };
}

function stopAction(ctx: SessionActionContext): AppAction {
  const action = sessionActions(ctx).find((candidate) => candidate.command === "stop");
  if (action === undefined) throw new Error("no /stop");
  return action;
}

describe("a session's actions", () => {
  it("offers /stop, which stops the run", () => {
    const ctx = context();
    const stop = stopAction(ctx);
    expect(stop.label).toBe("Stop this session");
    expect(stop.disabled).toBeUndefined();
    stop.run();
    expect(ctx.stop).toHaveBeenCalledOnce();
  });

  it("says why /stop can't run", () => {
    expect(stopAction(context({ agentReady: false })).disabled).toBe("Start atlas first");
    expect(stopAction(context({ stoppable: false })).disabled).toBe("This session isn't running");
    expect(stopAction(context({ stopping: true })).disabled).toBe("It's already stopping");
  });
});
