import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, mockFetch, render, screen, stubWebSocket } from "../test/component";
import { fakeAgentConfig, type FakeAgentConfig } from "../test/fake-config";
import { agentConfigFile, configCoordinator } from "../lib/config-coordinator";
import { setViewedAgent } from "../lib/viewed-agent";
import ThinkingSelector from "./ThinkingSelector.svelte";

const PROVIDERS = `[models]
main = { model = "anthropic/claude-a", temperature = 0.5, thinking = "low" }
`;

let agent = "";
let server: FakeAgentConfig;
let count = 0;

beforeEach(() => {
  stubWebSocket();
  agent = `thinker-${++count}`;
  server = fakeAgentConfig(agent, { providers: PROVIDERS });
  mockFetch(server.handler);
  setViewedAgent(agent);
});

afterEach(() => {
  setViewedAgent(null);
});

const level = (name: string): HTMLElement => screen.getByTitle(`Thinking: ${name}`);

const patches = (): unknown[] =>
  server.requests
    .filter((r) => r.method === "PATCH")
    .map((r): unknown => JSON.parse(r.body ?? "{}"));

describe("ThinkingSelector", () => {
  it("marks the level providers.toml holds", async () => {
    render(ThinkingSelector);
    await vi.waitFor(() => {
      expect(level("low").classList.contains("active")).toBe(true);
    });
  });

  it("sets a level through a patch that keeps the model and temperature", async () => {
    render(ThinkingSelector);
    await vi.waitFor(() => {
      expect(level("low").classList.contains("active")).toBe(true);
    });

    await fireEvent.mouseDown(level("high"));

    await vi.waitFor(() => {
      expect(patches()).toEqual([
        {
          models: {
            main: { $inline: { model: "anthropic/claude-a", temperature: 0.5, thinking: "high" } },
          },
        },
      ]);
    });
    await vi.waitFor(() => {
      expect(level("high").classList.contains("active")).toBe(true);
    });
  });

  it("clears the level when the active one is clicked", async () => {
    render(ThinkingSelector);
    await vi.waitFor(() => {
      expect(level("low").classList.contains("active")).toBe(true);
    });

    await fireEvent.mouseDown(level("low"));

    await vi.waitFor(() => {
      expect(patches()).toEqual([
        { models: { main: { $inline: { model: "anthropic/claude-a", temperature: 0.5 } } } },
      ]);
    });
  });

  it("follows a settings save of the thinking level without a reload", async () => {
    render(ThinkingSelector);
    await vi.waitFor(() => {
      expect(level("low").classList.contains("active")).toBe(true);
    });

    await configCoordinator.save(agentConfigFile(agent, "providers"), {
      baseline: PROVIDERS,
      edit: {
        patch: {
          models: { main: { $inline: { model: "anthropic/claude-a", thinking: "medium" } } },
        },
      },
      choose: () => Promise.resolve("keep-mine"),
    });

    await vi.waitFor(() => {
      expect(level("medium").classList.contains("active")).toBe(true);
    });
    expect(level("low").classList.contains("active")).toBe(false);
  });

  it("follows a change made outside the page", async () => {
    render(ThinkingSelector);
    await vi.waitFor(() => {
      expect(level("low").classList.contains("active")).toBe(true);
    });

    server.files.providers = PROVIDERS.replace('"low"', '"high"');
    await configCoordinator.externalChange(agentConfigFile(agent, "providers"));

    await vi.waitFor(() => {
      expect(level("high").classList.contains("active")).toBe(true);
    });
  });
});
