import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  advance,
  fireEvent,
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  stubWebSocket,
} from "./test/component";
import { fakeAgentConfig, type FakeAgentConfig } from "./test/fake-config";
import Settings from "./Settings.svelte";
import { ALL_SCOPE, type SectionId } from "./lib/settings-sections";
import { agentConfigFile, configCoordinator } from "./lib/config-coordinator";
import { toast } from "./lib/toast.svelte";

const CONFIG = "timeout_secs = 30\nmax_tokens = 100\n";

let agent = "";
let server: FakeAgentConfig;
let count = 0;

function mount(
  scope: "agent" | "hub" = "agent",
  section: SectionId = "runtime",
): ReturnType<typeof render<typeof Settings>> {
  return render(Settings, {
    scope: scope === "agent" ? agent : ALL_SCOPE,
    section,
    onSelectSection: () => {},
    onClose: () => {},
  });
}

const timeout = (): HTMLInputElement => screen.getByLabelText("Timeout (seconds)");
const maxTokens = (): HTMLInputElement => screen.getByLabelText("Max Tokens");
const toasts = (): string[] => [...toast.toasts.values()].map((t) => t.message);
const writes = (): string[] =>
  server.requests
    .filter((r) => r.method === "PUT" || r.method === "PATCH")
    .map((r) => `${r.method} ${r.url}`);

async function type(input: HTMLInputElement, value: string): Promise<void> {
  await fireEvent.input(input, { target: { value } });
}

beforeEach(() => {
  vi.useFakeTimers();
  stubWebSocket();
  // The page remembers its view (form or raw) between visits.
  localStorage.clear();
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
  agent = `settings-${++count}`;
  server = fakeAgentConfig(agent, { config: CONFIG });
  mockFetch(server.handler);
});

afterEach(() => {
  vi.useRealTimers();
});

describe("Settings saving through the config write coordinator", () => {
  it("reads the file again before it patches, and again after", async () => {
    mount();
    await settle();
    server.requests.length = 0;

    await type(timeout(), "60");
    await advance(1000);

    expect(server.requests.map((r) => `${r.method} ${r.url}`)).toEqual([
      `GET /api/agents/${agent}/config/raw`,
      `PATCH /api/agents/${agent}/config/patch`,
      `GET /api/agents/${agent}/config/raw`,
    ]);
    expect(JSON.parse(server.requests[1]?.body ?? "{}")).toEqual({ timeout_secs: 60 });
  });

  it("keeps a change made elsewhere to another key", async () => {
    mount();
    await settle();

    server.files.config = "timeout_secs = 30\nmax_tokens = 200\n";
    await type(timeout(), "60");
    await advance(1000);

    expect(server.files.config).toContain("max_tokens = 200");
    expect(server.files.config).toContain("timeout_secs = 60");
    expect(toasts()).toEqual([]);
  });

  it("keeps the page's edit and says what it replaced when the same key changed elsewhere", async () => {
    mount();
    await settle();

    server.files.config = "timeout_secs = 45\nmax_tokens = 100\n";
    await type(timeout(), "60");
    await advance(1000);

    expect(server.files.config).toContain("timeout_secs = 60");
    expect(toasts()).toEqual([
      "Saving replaced changes made elsewhere to timeout_secs in config.toml.",
    ]);
  });

  it("does not reload the form from its own save", async () => {
    mount();
    await settle();
    await type(timeout(), "60");
    await advance(1000);
    server.requests.length = 0;

    await type(maxTokens(), "150");
    await advance(1000);

    // Each save reads before and after and nothing else: no reload of the form.
    expect(server.requests.map((r) => r.method)).toEqual(["GET", "PATCH", "GET"]);
    expect(timeout().value).toBe("60");
  });

  it("saves the next edit against the file as the last save left it", async () => {
    mount();
    await settle();

    await type(timeout(), "60");
    await advance(1000);
    await type(timeout(), "90");
    await advance(1000);

    // The second save's own earlier write is not a change made elsewhere.
    expect(toasts()).toEqual([]);
    expect(server.files.config).toContain("timeout_secs = 90");
  });
});

describe("Settings following changes made elsewhere", () => {
  it("shows a change made outside the page", async () => {
    mount();
    await settle();
    expect(timeout().value).toBe("30");

    server.files.config = "timeout_secs = 99\nmax_tokens = 100\n";
    await configCoordinator.externalChange(agentConfigFile(agent, "config"));
    await settle();

    expect(timeout().value).toBe("99");
  });

  it("shows the main model another view saved", async () => {
    server.files.providers =
      '[providers.anthropic]\ntype = "anthropic"\n\n[models]\nmain = "anthropic/claude-a"\n';
    mount("agent", "model");
    await settle();
    const model = (): HTMLSelectElement =>
      screen.getByLabelText("Model", { selector: "#srole-main-model" });
    await vi.waitFor(() => {
      expect(model().value).toBe("claude-a");
    });
    // The panel fills in the roles the file leaves unset, and saves them.
    await advance(1000);

    await configCoordinator.edit(agentConfigFile(agent, "providers"), () => ({
      models: { main: "anthropic/claude-b" },
    }));

    await vi.waitFor(() => {
      expect(model().value).toBe("claude-b");
    });
  });

  it("shows a history restore", async () => {
    mockFetch((url, init) => {
      if (url.endsWith("/checkpoints/cp1/restore")) {
        server.files.config = "timeout_secs = 10\nmax_tokens = 100\n";
        return jsonResponse({ checkpoint_id: "cp2", restored_paths: ["config.toml"] });
      }
      return server.handler(url, init);
    });
    mount();
    await settle();

    await configCoordinator.restore(agent, "cp1", "agent_config", "config.toml");
    await settle();

    expect(timeout().value).toBe("10");
  });

  it("keeps what is being typed when the file changes elsewhere", async () => {
    mount();
    await settle();

    await type(timeout(), "77");
    server.files.config = "timeout_secs = 99\nmax_tokens = 300\n";
    await configCoordinator.externalChange(agentConfigFile(agent, "config"));
    await settle();

    expect(timeout().value).toBe("77");
    expect(maxTokens().value).toBe("100");
  });

  it("replaces what is being typed when the user restores a checkpoint", async () => {
    mockFetch((url, init) => {
      if (url.endsWith("/checkpoints/cp1/restore")) {
        server.files.config = "timeout_secs = 10\nmax_tokens = 100\n";
        return jsonResponse({ checkpoint_id: "cp2", restored_paths: ["config.toml"] });
      }
      return server.handler(url, init);
    });
    mount();
    await settle();

    await type(timeout(), "77");
    await configCoordinator.restore(agent, "cp1", "agent_config", "config.toml");
    await settle();

    expect(timeout().value).toBe("10");
  });

  it("stops following when it is closed", async () => {
    const view = mount();
    await settle();
    view.unmount();
    server.requests.length = 0;

    await configCoordinator.externalChange(agentConfigFile(agent, "config"));

    expect(server.requests.map((r) => r.method)).toEqual(["GET"]);
  });
});

describe("Settings' raw editors", () => {
  it("write only the files that were edited", async () => {
    server.files.providers = '[models]\nmain = "anthropic/claude-a"\n';
    mount("agent", "raw");
    await settle();
    await fireEvent.click(screen.getByRole("button", { name: "providers.toml" }));
    server.requests.length = 0;

    const editor = screen.getByRole("textbox");
    await fireEvent.input(editor, { target: { value: '[models]\nmain = "anthropic/claude-b"\n' } });
    await advance(1000);

    expect(writes()).toEqual([`PUT /api/agents/${agent}/providers/raw`]);
    expect(server.files.providers).toContain("claude-b");
  });

  it("keep the edit and say what they replaced when the file changed elsewhere", async () => {
    server.files.providers = '[models]\nmain = "anthropic/claude-a"\n';
    mount("agent", "raw");
    await settle();
    await fireEvent.click(screen.getByRole("button", { name: "providers.toml" }));

    server.files.providers = '[models]\nmain = "anthropic/claude-c"\n';
    const editor = screen.getByRole("textbox");
    await fireEvent.input(editor, { target: { value: '[models]\nmain = "anthropic/claude-b"\n' } });
    await advance(1000);

    expect(server.files.providers).toContain("claude-b");
    expect(toasts()).toEqual([
      "Saving replaced changes made elsewhere to models.main in providers.toml.",
    ]);
  });

  it("show the form's saved changes when switching to raw", async () => {
    const view = mount();
    await settle();
    await type(timeout(), "60");
    await advance(1000);

    await view.rerender({ section: "raw" });
    await settle();

    expect(screen.getByRole<HTMLTextAreaElement>("textbox").value).toContain("timeout_secs = 60");
  });
});

describe("the hub's Settings page", () => {
  it("saves the hub's config through the coordinator, reading it before and after", async () => {
    server.files.hub = 'timezone = "UTC"\n';
    mount("hub", "general");
    await settle();
    server.requests.length = 0;

    await fireEvent.input(screen.getByLabelText("Timezone", { selector: "input" }), {
      target: { value: "America/New_York" },
    });
    await advance(1000);

    expect(server.requests.map((r) => `${r.method} ${r.url}`)).toEqual([
      "GET /api/hub/config/raw",
      "PATCH /api/hub/config/patch",
      "GET /api/hub/config/raw",
    ]);
  });

  it("shows a hub config reload that changed the file", async () => {
    server.files.hub = 'timezone = "UTC"\n';
    mount("hub", "general");
    await settle();

    server.files.hub = 'timezone = "Europe/Paris"\n';
    await configCoordinator.externalChange({ kind: "hub" });
    await settle();

    expect(screen.getByLabelText("Timezone", { selector: "input" })).toHaveValue("Europe/Paris");
  });
});
