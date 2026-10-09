import { beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { invalidate } from "../../lib/cache";
import { cacheKeyProvidersRaw } from "../../lib/api";
import { notifications } from "../../lib/notifications.svelte";
import { router } from "../../lib/router.svelte";
import { toast } from "../../lib/toast.svelte";
import { ws } from "../../lib/ws.svelte";
import { jsonResponse, mockFetch, render, screen, settle } from "../../test/component";
import ModelControl from "./ModelControl.svelte";

const FILE = `[models.main]
model = ["anthropic/claude-sonnet-4-6", "openai/gpt-4o"]
thinking = "low"
`;

let disk = FILE;
let patches: unknown[] = [];

beforeEach(() => {
  notifications.clear();
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
  disk = FILE;
  patches = [];
  invalidate(cacheKeyProvidersRaw("atlas"));
  vi.stubGlobal("matchMedia", (media: string) => ({
    matches: false,
    media,
    addEventListener: () => {},
    removeEventListener: () => {},
  }));
  mockFetch((url, init) => {
    if (url === "/api/agents/atlas/providers/raw") return new Response(disk);
    if (url === "/api/agents/atlas/providers/models") {
      return jsonResponse({
        models: [
          { id: "claude-sonnet-4-6", name: "Claude Sonnet 4.6" },
          { id: "claude-haiku-4-5", name: "Claude Haiku 4.5" },
        ],
      });
    }
    if (url === "/api/agents/atlas/providers/patch" && init?.method === "PATCH") {
      patches.push(JSON.parse(typeof init.body === "string" ? init.body : "{}"));
      disk = `[models]\nmain = ["anthropic/claude-haiku-4-5", "openai/gpt-4o"]\n`;
      return jsonResponse({ valid: true });
    }
    return jsonResponse({}, 404);
  });
});

async function opened(): Promise<HTMLElement> {
  render(ModelControl, { agent: "atlas" });
  const trigger = await screen.findByRole("button", {
    name: "Model: Claude Sonnet 4.6, low thinking",
  });
  await userEvent.click(trigger);
  return screen.getByRole("dialog", { name: "Model for atlas" });
}

describe("the model and thinking popover", () => {
  it("lists the main provider's models, with the one in use pressed and focused", async () => {
    const popover = await opened();
    expect(popover).toHaveTextContent("Model, from Anthropic");
    const current = screen.getByRole("button", { name: "Claude Sonnet 4.6" });
    expect(current).toHaveAttribute("aria-pressed", "true");
    await vi.waitFor(() => {
      expect(current).toHaveFocus();
    });
    expect(screen.getByRole("button", { name: "Low" })).toHaveAttribute("aria-pressed", "true");
  });

  it("moves between the models with Up and Down, wrapping at the ends", async () => {
    await opened();
    const sonnet = screen.getByRole("button", { name: "Claude Sonnet 4.6" });
    const haiku = screen.getByRole("button", { name: "Claude Haiku 4.5" });
    await vi.waitFor(() => {
      expect(sonnet).toHaveFocus();
    });

    await userEvent.keyboard("{ArrowDown}");
    expect(haiku).toHaveFocus();
    await userEvent.keyboard("{ArrowDown}");
    expect(sonnet).toHaveFocus();
    await userEvent.keyboard("{ArrowUp}");
    expect(haiku).toHaveFocus();
  });

  it("switches the model through the coordinator, keeping the failover list, and reloads the agent", async () => {
    const send = vi.spyOn(ws, "send").mockImplementation(() => {});
    await opened();
    await userEvent.click(screen.getByRole("button", { name: "Claude Haiku 4.5" }));
    await settle();
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
    expect(send).toHaveBeenCalledWith({ type: "reload" });
    expect(
      await screen.findByRole("button", { name: "Model: Claude Haiku 4.5" }),
    ).toBeInTheDocument();
  });

  it("reloads without announcing the reload, which the popover already explained", async () => {
    vi.spyOn(ws, "send").mockImplementation(() => {});
    await opened();
    await userEvent.click(screen.getByRole("button", { name: "Claude Haiku 4.5" }));
    await settle();

    // What the agent sends back: it is reloading, and then how that went.
    ws.transport.onMessage?.({ type: "reloading" });
    ws.transport.onMessage?.({ type: "notice", message: "configuration reloaded: models" });
    ws.transport.onMessage?.({
      type: "notice",
      message: "configuration reloaded: no changes detected",
    });

    expect(notifications.history).toEqual([]);
    expect([...toast.toasts.values()]).toEqual([]);
  });

  it("still reports a reload failing after a model change", async () => {
    vi.spyOn(ws, "send").mockImplementation(() => {});
    await opened();
    await userEvent.click(screen.getByRole("button", { name: "Claude Haiku 4.5" }));
    await settle();

    ws.transport.onMessage?.({ type: "reloading" });
    ws.transport.onMessage?.({
      type: "notice",
      message: "config reload failed (keeping current config): invalid TOML",
    });

    expect(notifications.history.map((entry) => [entry.kind, entry.message])).toEqual([
      ["error", "Residuum couldn't apply the new settings and is still using the old ones."],
    ]);
  });

  it("clears the thinking level when the chosen one is pressed again", async () => {
    vi.spyOn(ws, "send").mockImplementation(() => {});
    await opened();
    await userEvent.click(screen.getByRole("button", { name: "Low" }));
    await settle();
    expect(patches).toEqual([
      { models: { main: ["anthropic/claude-sonnet-4-6", "openai/gpt-4o"] } },
    ]);
  });

  it("opens the agent's Model settings", async () => {
    const openSettings = vi.spyOn(router, "openSettings").mockResolvedValue(true);
    await opened();
    await userEvent.click(screen.getByRole("button", { name: "More model settings" }));
    expect(openSettings).toHaveBeenCalledWith({ scope: "atlas", section: "model" });
    expect(screen.queryByRole("dialog", { name: "Model for atlas" })).toBeNull();
  });

  it("says when the model can't be read, with Try again", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    mockFetch(() => jsonResponse({}, 500));
    render(ModelControl, { agent: "atlas" });
    await userEvent.click(await screen.findByRole("button", { name: "Model: Model" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn't read atlas's model.");
    expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
  });
});
