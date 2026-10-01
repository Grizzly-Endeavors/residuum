import { beforeEach, describe, expect, it, vi } from "vitest";
import type * as Models from "../../lib/models";
import { MainModel, modelControlLabel } from "./main-model.svelte";

// The coordinator writes a patch the server merges into the file; these tests
// check the patch the control builds from the file as it is.

type Listener = (change: { source: symbol | null }) => void;

const { disk, patches, listeners, surface } = vi.hoisted(() => ({
  disk: { text: "", valid: true },
  patches: [] as Record<string, unknown>[],
  listeners: new Set<Listener>(),
  surface: vi.fn(),
}));

vi.mock("../../lib/config-coordinator", () => ({
  agentConfigFile: (agent: string, name: string) => ({ kind: "agent", agent, name }),
  configCoordinator: {
    read: () => Promise.resolve(disk.text),
    edit: (
      _file: unknown,
      build: (raw: string) => Record<string, unknown> | null,
    ): Promise<unknown> => {
      const patch = build(disk.text);
      if (patch !== null) patches.push(patch);
      return Promise.resolve({
        result: disk.valid ? { valid: true } : { valid: false, error: "models.main: unknown" },
        written: patch !== null && disk.valid,
        raw: disk.text,
      });
    },
    subscribe: (_file: unknown, listener: Listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  },
}));

vi.mock("../../lib/models", async (importOriginal) => ({
  ...(await importOriginal<typeof Models>()),
  fetchModels: () =>
    Promise.resolve({
      models: [
        { id: "claude-sonnet-4-6", name: "Claude Sonnet 4.6" },
        { id: "claude-haiku-4-5", name: "Claude Haiku 4.5" },
      ],
      error: null,
    }),
}));

vi.mock("../../lib/notifications.svelte", () => ({ notifications: { surface } }));

const WITH_FAILOVER = `[models.main]
model = ["anthropic/claude-sonnet-4-6", "openai/gpt-4o"]
thinking = "low"
`;

beforeEach(() => {
  disk.text = WITH_FAILOVER;
  disk.valid = true;
  patches.length = 0;
  listeners.clear();
  surface.mockReset();
});

async function loaded(onWritten = vi.fn()): Promise<MainModel> {
  const main = new MainModel("atlas", onWritten);
  await main.load();
  return main;
}

describe("the composer's model control", () => {
  it("reads the main model, its thinking and its provider's models", async () => {
    const main = await loaded();
    expect(main.model).toBe("claude-sonnet-4-6");
    expect(main.modelName).toBe("Claude Sonnet 4.6");
    expect(main.thinking).toBe("low");
    expect(main.providerLabel).toBe("Anthropic");
    expect(main.models.map((entry) => entry.id)).toEqual(["claude-sonnet-4-6", "claude-haiku-4-5"]);
  });

  it("switches the model and keeps the failover list and the thinking level", async () => {
    const written = vi.fn();
    const main = await loaded(written);
    await main.choose("claude-haiku-4-5");
    expect(patches).toEqual([
      {
        models: {
          main: {
            model: ["anthropic/claude-haiku-4-5", "openai/gpt-4o"],
            temperature: null,
            thinking: "low",
          },
        },
      },
    ]);
    expect(written).toHaveBeenCalledOnce();
  });

  it("sets a thinking level, and pressing the one that is set clears it", async () => {
    const main = await loaded();
    await main.toggleThinking("high");
    await main.toggleThinking("low");
    expect(patches.map((patch) => patch.models)).toEqual([
      {
        main: {
          model: ["anthropic/claude-sonnet-4-6", "openai/gpt-4o"],
          temperature: null,
          thinking: "high",
        },
      },
      { main: ["anthropic/claude-sonnet-4-6", "openai/gpt-4o"] },
    ]);
  });

  it("writes a change made while another is still being written", async () => {
    const main = await loaded();
    await Promise.all([main.choose("claude-haiku-4-5"), main.toggleThinking("low")]);
    expect(patches).toHaveLength(2);
    expect(main.saving).toBe(false);
  });

  it("says why when the change isn't accepted, and doesn't reload the agent", async () => {
    disk.valid = false;
    const written = vi.fn();
    const main = await loaded(written);
    await main.choose("claude-haiku-4-5");
    expect(surface).toHaveBeenCalledWith(
      "error",
      "Couldn't switch the model. Residuum didn't accept the change. Open Model settings to see why.",
      "models.main: unknown",
    );
    expect(written).not.toHaveBeenCalled();
  });

  it("reads the file again when it changes elsewhere, not after its own write", async () => {
    const main = new MainModel("atlas", vi.fn());
    const stop = main.follow();
    await vi.waitFor(() => {
      expect(main.loaded).toBe(true);
    });
    disk.text = `[models]\nmain = "anthropic/claude-haiku-4-5"\n`;
    for (const listener of listeners) listener({ source: null });
    await vi.waitFor(() => {
      expect(main.model).toBe("claude-haiku-4-5");
    });
    expect(main.thinking).toBe("");
    stop();
    expect(listeners.size).toBe(0);
  });
});

describe("the control's words", () => {
  it("names the model, and the thinking level when one is set", () => {
    expect(modelControlLabel("Claude Sonnet 4.6", "")).toBe("Claude Sonnet 4.6");
    expect(modelControlLabel("Claude Sonnet 4.6", "off")).toBe(
      "Claude Sonnet 4.6, no extra thinking",
    );
    expect(modelControlLabel("Claude Sonnet 4.6", "high")).toBe("Claude Sonnet 4.6, high thinking");
  });
});
