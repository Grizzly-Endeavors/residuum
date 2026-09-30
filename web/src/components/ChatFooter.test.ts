import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mockFetch, render, screen, stubWebSocket } from "../test/component";
import { fakeAgentConfig, type FakeAgentConfig } from "../test/fake-config";
import { agentConfigFile, configCoordinator } from "../lib/config-coordinator";
import type { SessionUsageTotals } from "../lib/types";
import { setViewedAgent } from "../lib/viewed-agent";
import ChatFooter from "./ChatFooter.svelte";

function usage(overrides: Partial<SessionUsageTotals> = {}): SessionUsageTotals {
  return {
    input_tokens: 100,
    output_tokens: 20,
    context_tokens: null,
    tool_calls: 0,
    ...overrides,
  };
}

describe("ChatFooter", () => {
  it("shows the cumulative tool-call count next to tokens", () => {
    render(ChatFooter, { usage: usage({ tool_calls: 12 }), model: null });
    expect(screen.getByText("12 tool calls")).toBeTruthy();
  });

  it("omits the tool-call segment when the session has run none yet", () => {
    render(ChatFooter, { usage: usage({ tool_calls: 0 }), model: null });
    expect(screen.queryByText(/tool call/)).toBeNull();
  });

  it("uses the singular form for exactly one tool call", () => {
    render(ChatFooter, { usage: usage({ tool_calls: 1 }), model: null });
    expect(screen.getByText("1 tool call")).toBeTruthy();
  });
});

describe("ChatFooter's model label", () => {
  const PROVIDERS = '[models]\nmain = "anthropic/claude-a"\n';
  let agent = "";
  let server: FakeAgentConfig;
  let count = 0;

  beforeEach(() => {
    stubWebSocket();
    agent = `footer-${++count}`;
    server = fakeAgentConfig(agent, { providers: PROVIDERS });
    mockFetch(server.handler);
    setViewedAgent(agent);
  });

  afterEach(() => {
    setViewedAgent(null);
  });

  it("shows the main model from providers.toml when no model is given", async () => {
    render(ChatFooter, { usage: null });
    expect(await screen.findByText("claude-a")).toBeTruthy();
  });

  it("follows a settings save of the main model without a reload", async () => {
    render(ChatFooter, { usage: null });
    await screen.findByText("claude-a");

    await configCoordinator.save(agentConfigFile(agent, "providers"), {
      baseline: PROVIDERS,
      edit: { patch: { models: { main: "anthropic/claude-b" } } },
      choose: () => Promise.resolve("keep-mine"),
    });

    expect(await screen.findByText("claude-b")).toBeTruthy();
    expect(screen.queryByText("claude-a")).toBeNull();
  });

  it("follows a change made outside the page", async () => {
    render(ChatFooter, { usage: null });
    await screen.findByText("claude-a");

    server.files.providers = PROVIDERS.replace("claude-a", "claude-c");
    await configCoordinator.externalChange(agentConfigFile(agent, "providers"));

    expect(await screen.findByText("claude-c")).toBeTruthy();
  });

  it("drops the label when the main model is removed", async () => {
    render(ChatFooter, { usage: null });
    await screen.findByText("claude-a");

    server.files.providers = "";
    await configCoordinator.externalChange(agentConfigFile(agent, "providers"));

    await vi.waitFor(() => {
      expect(screen.queryByText("claude-a")).toBeNull();
    });
  });

  it("shows the label it is given and never reads the file", async () => {
    render(ChatFooter, { usage: null, model: "given-model" });
    expect(screen.getByText("given-model")).toBeTruthy();

    await configCoordinator.externalChange(agentConfigFile(agent, "providers"));

    expect(screen.getByText("given-model")).toBeTruthy();
    // Only the coordinator's own read of the file.
    expect(server.requests.filter((r) => r.url.endsWith("/providers/raw"))).toHaveLength(1);
  });
});
