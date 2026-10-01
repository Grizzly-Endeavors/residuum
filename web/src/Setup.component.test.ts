import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { jsonResponse, mockFetch, render, screen, settle } from "./test/component";
import Setup from "./Setup.svelte";

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
    await vi.waitFor(() => expect(screen.getByLabelText("Time zone")).toHaveValue("Europe/Oslo"));
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
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: (ms) => vi.advanceTimersByTime(ms) });
    render(Setup, { onComplete: () => {} });
    await settle();

    await user.click(screen.getByRole("button", { name: "Next" }));
    await user.type(screen.getByLabelText("Anthropic API key"), "sk-secret");
    await vi.advanceTimersByTimeAsync(600);

    const draft = localStorage.getItem(DRAFT_KEY) ?? "";
    expect(JSON.parse(draft)).toMatchObject({ step: 1 });
    expect(draft).not.toContain("sk-secret");
  });
});
