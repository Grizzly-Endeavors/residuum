import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "../../test/component";
import Welcome from "./Welcome.svelte";
import type { SetupWizardState } from "../../lib/types";

function wizard(overrides: Partial<SetupWizardState> = {}): SetupWizardState {
  return { userName: "", agentName: "assistant", timezone: "", ...overrides } as SetupWizardState;
}

describe("Welcome step", () => {
  it("asks for your name and the first agent's name", () => {
    render(Welcome, { wizardState: wizard(), onNext: () => {} });
    expect(screen.getByLabelText("Your Name")).toBeTruthy();
    expect(screen.getByLabelText("Agent Name")).toHaveValue("assistant");
  });

  it("lets you continue without giving your name", async () => {
    const onNext = vi.fn();
    render(Welcome, { wizardState: wizard(), onNext });
    await fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(onNext).toHaveBeenCalled();
  });

  it.each([
    ["", "Give your agent a name."],
    ["Scout", "Use only lowercase letters, digits, and hyphens."],
    ["-scout", "The name can't start or end with a hyphen."],
    ["scout-", "The name can't start or end with a hyphen."],
    ["a".repeat(25), "Use 24 characters or fewer."],
    ["team", '"team" is reserved. Pick a different name.'],
  ])("blocks the agent name %j and says why", (agentName, message) => {
    render(Welcome, { wizardState: wizard({ agentName }), onNext: () => {} });
    expect(screen.getByRole("alert")).toHaveTextContent(message);
    expect(screen.getByRole("button", { name: "Next" })).toBeDisabled();
  });

  it("accepts a well-formed agent name", () => {
    render(Welcome, { wizardState: wizard({ agentName: "night-owl-2" }), onNext: () => {} });
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("button", { name: "Next" })).toBeEnabled();
  });
});
