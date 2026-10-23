import { afterEach, describe, expect, it, vi } from "vitest";
import { type AppAction } from "./action-registry.svelte";
import { runCommandLine } from "./command-line";
import { notifications } from "./notifications.svelte";
import { waitFor } from "../test/wait";

function inboxAction(overrides: Partial<AppAction> = {}): AppAction {
  return {
    id: "chat:inbox",
    group: "Actions",
    label: "New message to atlas",
    icon: "inbox",
    command: "inbox",
    takesText: true,
    run: vi.fn(),
    ...overrides,
  };
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("runCommandLine", () => {
  it("runs an action that can run with no text, even when its text form is refused", async () => {
    const action = inboxAction({ disabledWithText: "Start atlas first" });
    expect(runCommandLine([action], "/inbox")).toBe("ran");
    await waitFor(() => {
      expect(action.run).toHaveBeenCalledWith("");
    });
  });

  it("refuses a line with text when the action can't run with text, and keeps the line", () => {
    const surface = vi.spyOn(notifications, "surface").mockImplementation(() => {});
    const action = inboxAction({ disabledWithText: "Start atlas first" });
    expect(runCommandLine([action], "/inbox water the plants")).toBe("refused");
    expect(action.run).not.toHaveBeenCalled();
    expect(surface).toHaveBeenCalledWith("error", "Couldn't run /inbox: Start atlas first.");
  });

  it("refuses a bare command when the action is disabled outright", () => {
    vi.spyOn(notifications, "surface").mockImplementation(() => {});
    const action = inboxAction({ disabled: "Open an agent first" });
    expect(runCommandLine([action], "/inbox")).toBe("refused");
    expect(action.run).not.toHaveBeenCalled();
  });

  it("runs an action with text when it is enabled", async () => {
    const action = inboxAction();
    expect(runCommandLine([action], "/inbox water the plants")).toBe("ran");
    await waitFor(() => {
      expect(action.run).toHaveBeenCalledWith("water the plants");
    });
  });
});
