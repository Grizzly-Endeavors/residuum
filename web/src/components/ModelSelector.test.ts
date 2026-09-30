import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  fireEvent,
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  stubWebSocket,
} from "../test/component";
import { fakeAgentConfig, type FakeAgentConfig } from "../test/fake-config";
import { agentConfigFile, configCoordinator } from "../lib/config-coordinator";
import { toast } from "../lib/toast.svelte";
import { setViewedAgent } from "../lib/viewed-agent";
import ModelSelector from "./ModelSelector.svelte";

const PROVIDERS = `[providers.anthropic]
type = "anthropic"
api_key = "secret:anthropic"

[models]
main = "anthropic/claude-a"
`;

let agent = "";
let server: FakeAgentConfig;
let count = 0;

beforeEach(() => {
  stubWebSocket();
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
  agent = `scout-${++count}`;
  server = fakeAgentConfig(agent, { providers: PROVIDERS });
  mockFetch(server.handler);
  setViewedAgent(agent);
});

afterEach(() => {
  setViewedAgent(null);
});

const patches = (): { url: string; body: unknown }[] =>
  server.requests
    .filter((r) => r.method === "PATCH")
    .map((r) => ({ url: r.url, body: JSON.parse(r.body ?? "{}") as unknown }));

async function pick(name: string): Promise<void> {
  await fireEvent.click(screen.getByTitle("Switch model"));
  await fireEvent.mouseDown(await screen.findByRole("option", { name }));
}

describe("ModelSelector", () => {
  it("shows the main model from providers.toml", async () => {
    render(ModelSelector);
    expect(await screen.findByText("claude-a")).toBeTruthy();
  });

  it("switches the main model through a patch of models.main, keeping the provider", async () => {
    render(ModelSelector);
    await screen.findByText("claude-a");

    await pick("Claude B");

    await vi.waitFor(() => {
      expect(patches()).toEqual([
        {
          url: `/api/agents/${agent}/providers/patch`,
          body: { models: { main: "anthropic/claude-b" } },
        },
      ]);
    });
    expect(await screen.findByText("claude-b")).toBeTruthy();
  });

  it("keeps the thinking level when it switches the model", async () => {
    server.files.providers = PROVIDERS.replace(
      'main = "anthropic/claude-a"',
      'main = { model = "anthropic/claude-a", thinking = "high" }',
    );
    render(ModelSelector);
    await screen.findByText("claude-a");

    await pick("Claude B");

    await vi.waitFor(() => {
      expect(patches().map((p) => p.body)).toEqual([
        { models: { main: { $inline: { model: "anthropic/claude-b", thinking: "high" } } } },
      ]);
    });
  });

  it("reads the file again before it writes, so it builds on what is there now", async () => {
    render(ModelSelector);
    await screen.findByText("claude-a");
    // Something else set a thinking level since the chip loaded.
    server.files.providers = PROVIDERS.replace(
      'main = "anthropic/claude-a"',
      'main = { model = "anthropic/claude-a", thinking = "low" }',
    );

    await pick("Claude B");

    await vi.waitFor(() => {
      expect(patches().map((p) => p.body)).toEqual([
        { models: { main: { $inline: { model: "anthropic/claude-b", thinking: "low" } } } },
      ]);
    });
  });

  it("follows a settings save of the main model without a reload", async () => {
    render(ModelSelector);
    await screen.findByText("claude-a");

    await configCoordinator.save(agentConfigFile(agent, "providers"), {
      baseline: PROVIDERS,
      edit: { patch: { models: { main: "anthropic/claude-b" } } },
      choose: () => Promise.resolve("keep-mine"),
    });

    expect(await screen.findByText("claude-b")).toBeTruthy();
  });

  it("follows a change made outside the page", async () => {
    render(ModelSelector);
    await screen.findByText("claude-a");

    server.files.providers = PROVIDERS.replace("claude-a", "claude-b");
    await configCoordinator.externalChange(agentConfigFile(agent, "providers"));

    expect(await screen.findByText("claude-b")).toBeTruthy();
  });

  it("follows a history restore of providers.toml", async () => {
    mockFetch((url, init) => {
      if (url.endsWith("/checkpoints/cp1/restore")) {
        server.files.providers = PROVIDERS.replace("claude-a", "claude-b");
        return jsonResponse({ checkpoint_id: "cp2", restored_paths: ["providers.toml"] });
      }
      return server.handler(url, init);
    });
    render(ModelSelector);
    await screen.findByText("claude-a");

    await configCoordinator.restore(agent, "cp1", "agent_config", "providers.toml");

    expect(await screen.findByText("claude-b")).toBeTruthy();
  });

  it("says so when the save is refused, and keeps showing the current model", async () => {
    mockFetch((url, init) => {
      if (init?.method === "PATCH") return jsonResponse({ valid: false, error: "no such model" });
      return server.handler(url, init);
    });
    render(ModelSelector);
    await screen.findByText("claude-a");

    await pick("Claude B");
    await settle();

    await vi.waitFor(() => {
      expect([...toast.toasts.values()].map((t) => t.message)).toEqual([
        expect.stringContaining("Couldn't switch the model"),
      ]);
    });
    expect(screen.getByText("claude-a")).toBeTruthy();
  });

  it("stops following the file once it is gone", async () => {
    const view = render(ModelSelector);
    await screen.findByText("claude-a");
    view.unmount();
    server.requests.length = 0;

    await configCoordinator.externalChange(agentConfigFile(agent, "providers"));

    // Only the coordinator's own read: nothing reloads the chip.
    expect(server.requests.map((r) => r.url)).toEqual([`/api/agents/${agent}/providers/raw`]);
  });
});
