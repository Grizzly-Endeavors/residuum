import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { hub } from "../../lib/hub.svelte";
import type { AgentSummary } from "../../lib/hub-types";
import { setViewedAgent } from "../../lib/viewed-agent";
import { ws } from "../../lib/ws.svelte";
import { registerAppActions } from "../../shell/app-actions.svelte";
import type { ShellActions } from "../../shell/shell-actions";
import {
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  stubWebSocket,
} from "../../test/component";
import { snapshot } from "../../test/hub-frames";
import ConversationSizeHarness from "../../test/ui/ConversationSizeHarness.svelte";

const TOTALS = {
  input_tokens: 412_880,
  output_tokens: 9_214,
  context_tokens: 18_402,
  tool_calls: 37,
};

function agent(name: string, state: AgentSummary["state"]): AgentSummary {
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

let unregister: () => void = () => {};
let usageFails = false;

beforeEach(() => {
  usageFails = false;
  stubWebSocket();
  vi.spyOn(console, "error").mockImplementation(() => {});
  mockFetch((url) => {
    if (url.endsWith("/usage")) return usageFails ? jsonResponse({}, 500) : jsonResponse(TOTALS);
    return jsonResponse({}, 404);
  });
  hub.handleFrame(snapshot([agent("atlas", "running"), agent("drifter", "stopped")]));
  const shell = {
    openSearch: vi.fn(),
    openSettings: vi.fn(),
    openShortcuts: vi.fn(),
    openNotifications: vi.fn(),
    openFeedback: vi.fn(),
    createAgent: vi.fn(),
    addInboxNote: vi.fn(),
  } satisfies ShellActions;
  unregister = registerAppActions(shell);
});

afterEach(() => {
  unregister();
  setViewedAgent(null);
});

describe("the conversation size", () => {
  it("says how big the conversation is in words, with the token counts behind Details", async () => {
    setViewedAgent("atlas");
    render(ConversationSizeHarness);
    await screen.findByText("Tools used");

    expect(screen.getByRole("heading", { name: "Conversation size" })).toBeInTheDocument();
    expect(screen.getByRole("complementary")).toHaveTextContent(
      "About 14,000 words go to the model each time atlas replies",
    );
    expect(
      screen.getByText("Sent to the model in total, counting repeats").nextElementSibling,
    ).toHaveTextContent("About 310,000 words");
    expect(screen.getByText("Tools used").nextElementSibling).toHaveTextContent("37 times");
    expect(screen.queryByText("This reply so far")).toBeNull();

    const details = screen.getByRole("button", { name: "Token counts" });
    expect(screen.getByText("412,880")).not.toBeVisible();
    await userEvent.click(details);
    expect(screen.getByText("412,880")).toBeVisible();
    expect(screen.getByText("18,402")).toBeVisible();
  });

  it("adds what the reply in progress has written and done", async () => {
    setViewedAgent("atlas");
    render(ConversationSizeHarness);
    await screen.findByText("Tools used");
    ws.store.handleMessage({
      type: "turn_started",
      reply_to: "t1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    ws.store.handleMessage({
      type: "turn_usage",
      reply_to: "t1",
      output_tokens: 400,
      has_usage: true,
      tool_calls: 1,
      session_totals: null,
    });
    await settle();
    expect(screen.getByText("This reply so far").nextElementSibling).toHaveTextContent(
      "About 300 words written, tools used once",
    );
  });

  it("shows a stopped agent's last figures, and says why it can't summarize now", async () => {
    setViewedAgent("drifter");
    render(ConversationSizeHarness);
    await screen.findByText("Tools used");
    expect(screen.getByRole("complementary")).toHaveTextContent(
      "drifter isn't running, so these are the figures from when it last replied.",
    );
    expect(screen.getByRole("button", { name: "Summarize older messages now" })).toBeDisabled();
    expect(screen.getByText("Start drifter first.")).toBeInTheDocument();
  });

  it("says when the figures can't be read, and reads them again on Try again", async () => {
    usageFails = true;
    setViewedAgent("atlas");
    render(ConversationSizeHarness);
    await settle();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Couldn't read the size of the conversation with atlas.",
    );

    usageFails = false;
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    await settle();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByText("Tools used")).toBeInTheDocument();
  });
});
