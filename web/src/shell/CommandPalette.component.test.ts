import { cleanup } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "../test/component";
import { actionRegistry, type AppAction } from "../lib/action-registry.svelte";
import { router } from "../lib/router.svelte";
import CommandPalette from "./CommandPalette.svelte";

const observe = vi.fn();
const reflect = vi.fn();
const goHome = vi.fn();
let unregister: () => void = () => undefined;

const ACTIONS: readonly AppAction[] = [
  { id: "go:home", group: "Go to", label: "Home", hint: "Your team", icon: "home", run: goHome },
  {
    id: "chat:observe",
    group: "Actions",
    label: "Summarize older messages now",
    hint: "atlas",
    icon: "layers",
    command: "observe",
    run: observe,
  },
  {
    id: "chat:reflect",
    group: "Actions",
    label: "Condense memories now",
    icon: "memory",
    command: "reflect",
    disabled: "Start atlas first",
    run: reflect,
  },
];

beforeAll(() => {
  router.startForOverlays();
  // jsdom lays nothing out, so it has no scrolling into view.
  Element.prototype.scrollIntoView = vi.fn();
});

afterAll(() => {
  router.stop();
});

beforeEach(() => {
  vi.clearAllMocks();
  // Nor media queries: the palette asks one for its placeholder.
  vi.stubGlobal("matchMedia", (media: string) => ({ media, matches: false }));
  unregister = actionRegistry.register("test", () => ACTIONS);
});

afterEach(async () => {
  unregister();
  cleanup();
  await vi.waitFor(() => {
    expect((window.history.state as { overlay?: string } | null)?.overlay).toBeUndefined();
  });
});

describe("CommandPalette", () => {
  it("lists every action under its heading, with the first one active", () => {
    render(CommandPalette, { open: true });
    const field = screen.getByRole("combobox");
    expect(field).toHaveFocus();
    expect(screen.getByRole("group", { name: "Go to" })).toBeTruthy();
    expect(screen.getByRole("option", { name: /Home/ })).toHaveAttribute("aria-selected", "true");
    expect(field.getAttribute("aria-activedescendant")).toBe(
      screen.getByRole("option", { name: /Home/ }).id,
    );
  });

  it("narrows by what is typed, and Enter runs the active action", async () => {
    const user = userEvent.setup();
    render(CommandPalette, { open: true });
    await user.keyboard("/observe");
    expect(screen.getAllByRole("option").map((option) => option.textContent.trim())).toEqual([
      expect.stringContaining("Summarize older messages now"),
    ]);
    await user.keyboard("{Enter}");
    await vi.waitFor(() => {
      expect(observe).toHaveBeenCalledOnce();
    });
    expect(screen.queryByRole("combobox")).toBeNull();
  });

  it("moves with the arrow keys, wrapping, and shows a disabled action's reason without running it", async () => {
    const user = userEvent.setup();
    render(CommandPalette, { open: true });
    await user.keyboard("{ArrowUp}");
    const reflectOption = screen.getByRole("option", { name: /Condense memories now/ });
    expect(reflectOption).toHaveAttribute("aria-selected", "true");
    expect(reflectOption).toHaveAttribute("aria-disabled", "true");
    expect(reflectOption).toHaveTextContent("Start atlas first");
    await user.keyboard("{Enter}");
    expect(reflect).not.toHaveBeenCalled();
    expect(screen.getByRole("combobox")).toBeTruthy();

    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("option", { name: /Home/ })).toHaveAttribute("aria-selected", "true");
  });

  it("says when nothing matches", async () => {
    const user = userEvent.setup();
    render(CommandPalette, { open: true });
    await user.keyboard("zzz");
    expect(screen.queryAllByRole("option")).toEqual([]);
    expect(screen.getByText(/Nothing matches “zzz”/)).toBeTruthy();
  });

  it("runs an action that is clicked", async () => {
    const user = userEvent.setup();
    render(CommandPalette, { open: true });
    await user.click(screen.getByRole("option", { name: /Home/ }));
    await vi.waitFor(() => {
      expect(goHome).toHaveBeenCalledOnce();
    });
  });
});
