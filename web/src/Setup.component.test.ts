import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { jsonResponse, mockFetch, render, screen, settle } from "./test/component";
import Setup from "./Setup.svelte";
import { waitFor } from "./test/wait";

const DRAFT_KEY = "residuum-setup-draft";

describe("Setup wizard", () => {
  beforeEach(() => {
    localStorage.clear();
    mockFetch((url) => {
      if (url.includes("/system/timezone")) return jsonResponse({ timezone: "Europe/Oslo" });
      if (url.includes("/mcp-catalog")) return jsonResponse([]);
      if (url.includes("/providers/models")) return jsonResponse({ models: [] });
      throw new Error(`unexpected fetch ${url}`);
    });
  });

  afterEach(() => {
    localStorage.clear();
  });

  it("starts on Welcome and marks it as the current step", async () => {
    render(Setup, { onComplete: () => {} });
    await settle();

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Welcome to Residuum");
    const steps = screen.getByRole("list", { name: "Setup steps" });
    expect(steps.querySelector("[aria-current='step']")).toHaveTextContent("Welcome");
    await waitFor(() => expect(screen.getByLabelText("Time zone")).toHaveValue("Europe/Oslo"));
  });

  it("moves focus to the next step's heading", async () => {
    const user = userEvent.setup();
    render(Setup, { onComplete: () => {} });
    await settle();

    await user.click(screen.getByRole("button", { name: "Next" }));
    const heading = screen.getByRole("heading", { level: 1 });
    expect(heading).toHaveTextContent("Add model providers");
    expect(heading).toHaveFocus();
  });

  it("restores a saved draft at its step, without its keys, and says so", async () => {
    localStorage.setItem(
      DRAFT_KEY,
      JSON.stringify({ step: 1, wizardState: { agentName: "scout", timezone: "Asia/Tokyo" } }),
    );
    render(Setup, { onComplete: () => {} });
    await settle();

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Add model providers");
    expect(screen.getByText(/Keys and tokens aren't kept in the draft/)).toBeTruthy();
    expect(screen.getByLabelText("Anthropic API key")).toHaveValue("");

    await userEvent.setup().click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByText(/Keys and tokens aren't kept/)).toBeNull();
  });

  it("saves the draft without the API keys typed into it", async () => {
    // The clock moves only when the test moves it, so the draft saves once,
    // after the last keystroke, and holds everything typed.
    vi.useFakeTimers();
    const user = userEvent.setup({ advanceTimers: (ms) => vi.advanceTimersByTime(ms) });
    render(Setup, { onComplete: () => {} });
    await settle();

    await user.click(screen.getByRole("button", { name: "Next" }));
    await user.type(screen.getByLabelText("Anthropic API key"), "sk-secret");
    expect(localStorage.getItem(DRAFT_KEY)).toBeNull();

    // vi.waitFor moves the faked clock by its interval on every check.
    await waitFor(() => {
      expect(JSON.parse(localStorage.getItem(DRAFT_KEY) ?? "null")).toMatchObject({ step: 1 });
    });
    expect(localStorage.getItem(DRAFT_KEY)).not.toContain("sk-secret");
  });

  it("drops a draft save still waiting when the wizard closes", async () => {
    vi.useFakeTimers();
    const user = userEvent.setup({ advanceTimers: (ms) => vi.advanceTimersByTime(ms) });
    const view = render(Setup, { onComplete: () => {} });
    await settle();

    await user.click(screen.getByRole("button", { name: "Next" }));
    view.unmount();
    await vi.runOnlyPendingTimersAsync();

    expect(localStorage.getItem(DRAFT_KEY)).toBeNull();
  });
});
