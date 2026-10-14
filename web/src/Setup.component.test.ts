import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { jsonResponse, mockFetch, render, screen, settle } from "./test/component";
import Setup from "./Setup.svelte";
import { waitFor } from "./test/wait";

const DRAFT_KEY = "residuum-setup-draft";

/** Tavily as a draft stores it: added, with its key stripped. */
const TAVILY_WITHOUT_KEY = {
  name: "tavily",
  command: "npx",
  args: ["tavily"],
  env: { TAVILY_API_KEY: "" },
  secretEnvKeys: ["TAVILY_API_KEY"],
};

describe("Setup wizard", () => {
  beforeEach(() => {
    localStorage.clear();
    mockFetch((url) => {
      if (url.includes("/system/timezone")) return jsonResponse({ timezone: "Europe/Oslo" });
      if (url.includes("/mcp-catalog")) {
        return jsonResponse([
          {
            name: "tavily",
            description: "Web search",
            command: "npx",
            args: ["tavily"],
            env: {},
            category: "tools",
            requires_input: [{ field: "env.TAVILY_API_KEY", label: "Tavily API key" }],
            install_hint: "",
          },
        ]);
      }
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

    // waitFor moves the faked clock by its interval on every check, which lets the draft save run.
    await waitFor(() => {
      expect(JSON.parse(localStorage.getItem(DRAFT_KEY) ?? "null")).toMatchObject({ step: 1 });
    });
    expect(localStorage.getItem(DRAFT_KEY)).not.toContain("sk-secret");
  });

  it("saves the draft without a tool server key typed into it", async () => {
    vi.useFakeTimers();
    const user = userEvent.setup({ advanceTimers: (ms) => vi.advanceTimersByTime(ms) });
    render(Setup, { onComplete: () => {} });
    await settle();

    // Welcome -> Providers -> Roles -> Tool servers
    await user.click(screen.getByRole("button", { name: "Next" }));
    await user.click(screen.getByRole("button", { name: "Next" }));
    await user.click(screen.getByRole("button", { name: "Next" }));
    await waitFor(() => screen.getByRole("button", { name: "Add tavily" }));

    await user.click(screen.getByRole("button", { name: "Add tavily" }));
    await user.type(screen.getByLabelText("Tavily API key"), "tvly-secret");
    await user.click(screen.getByRole("button", { name: "Add tavily" }));

    // An earlier draft from this step may already be stored; wait for the one with the server.
    const servers = await waitFor(() => {
      const draft = JSON.parse(localStorage.getItem(DRAFT_KEY) ?? "null") as {
        wizardState: { mcpServers: unknown[] };
      } | null;
      expect(draft?.wizardState.mcpServers).toHaveLength(1);
      return draft?.wizardState.mcpServers;
    });
    expect(localStorage.getItem(DRAFT_KEY)).not.toContain("tvly-secret");
    expect(servers).toEqual([
      {
        name: "tavily",
        command: "npx",
        args: ["tavily"],
        env: { TAVILY_API_KEY: "" },
        secretEnvKeys: ["TAVILY_API_KEY"],
      },
    ]);
  });

  it("asks for a restored provider's and tool server's keys again", async () => {
    localStorage.setItem(
      DRAFT_KEY,
      JSON.stringify({
        step: 1,
        wizardState: {
          selectedProviders: ["anthropic"],
          mcpServers: [TAVILY_WITHOUT_KEY],
        },
      }),
    );
    const user = userEvent.setup();
    render(Setup, { onComplete: () => {} });
    await settle();

    expect(screen.getByText(/so enter this one again, or set the ANTHROPIC_API_KEY/)).toBeTruthy();
    await user.type(screen.getByLabelText("Anthropic API key"), "sk-again");
    expect(screen.queryByText(/so enter this one again/)).toBeNull();

    // Providers -> Roles -> Tool servers
    await user.click(screen.getByRole("button", { name: "Next" }));
    await user.click(screen.getByRole("button", { name: "Next" }));
    await waitFor(() => screen.getByRole("button", { name: "Enter the key for tavily" }));
    expect(screen.getByText("Needs its key")).toBeTruthy();
    expect(screen.getByText(/enter this server's key again/)).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Enter the key for tavily" }));
    await user.type(screen.getByLabelText("Tavily API key"), "tvly-again");
    await user.click(screen.getByRole("button", { name: "Save key" }));
    expect(screen.queryByText("Needs its key")).toBeNull();
    expect(screen.getByText("Added")).toBeTruthy();
  });

  it("lists on the Save step what a restored draft would save without a key", async () => {
    localStorage.setItem(
      DRAFT_KEY,
      JSON.stringify({
        step: 5,
        wizardState: {
          selectedProviders: ["anthropic", "ollama"],
          mcpServers: [TAVILY_WITHOUT_KEY],
        },
      }),
    );
    render(Setup, { onComplete: () => {} });
    await settle();

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Save and start");
    expect(
      screen.getByText(/Anthropic, tavily have no key and will be saved without one/),
    ).toBeTruthy();
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
