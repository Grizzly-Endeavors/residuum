import { act, cleanup, fireEvent } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { render, screen } from "../../test/component";
import FloatingHarness from "../../test/ui/FloatingHarness.svelte";
import { router } from "../router.svelte";
import { waitFor } from "../../test/wait";

beforeAll(() => {
  router.startForOverlays();
});

afterAll(() => {
  router.stop();
});

afterEach(async () => {
  vi.useRealTimers();
  cleanup();
  await waitFor(() => {
    expect((window.history.state as { overlay?: string } | null)?.overlay).toBeUndefined();
  });
});

function focusedLabel(): string | undefined {
  return document.activeElement?.textContent.trim();
}

function menuButton(): HTMLElement {
  return screen.getByRole("button", { name: "Manage" });
}

describe("Menu", () => {
  it("is a menu button that opens on Enter, starting on the first item", async () => {
    const user = userEvent.setup();
    render(FloatingHarness);
    const button = menuButton();
    expect(button).toHaveAttribute("aria-haspopup", "menu");
    expect(button).toHaveAttribute("aria-expanded", "false");
    button.focus();
    await user.keyboard("{Enter}");
    const menu = screen.getByRole("menu", { name: "Manage atlas" });
    expect(button).toHaveAttribute("aria-expanded", "true");
    expect(button).toHaveAttribute("aria-controls", menu.id);
    expect(menu).toHaveAccessibleDescription("atlas, running");
    expect(menu.closest("[data-overlay-host]")).not.toBeNull();
    expect(screen.getByRole("menuitem", { name: "Open chat" })).toHaveFocus();
  });

  it("opens from Arrow Up on its last item, and from a pointer on the menu itself", async () => {
    const user = userEvent.setup();
    render(FloatingHarness);
    menuButton().focus();
    await user.keyboard("{ArrowUp}");
    expect(screen.getByRole("menuitem", { name: "Settings" })).toHaveFocus();
    await user.keyboard("{Escape}");
    await user.click(menuButton());
    expect(screen.getByRole("menu")).toHaveFocus();
  });

  it("moves with the arrow keys, wrapping, and jumps with Home and End", async () => {
    const user = userEvent.setup();
    render(FloatingHarness);
    menuButton().focus();
    await user.keyboard("{ArrowDown}");
    await user.keyboard("{ArrowUp}");
    expect(focusedLabel()).toBe("Settings");
    await user.keyboard("{ArrowDown}");
    expect(focusedLabel()).toBe("Open chat");
    // A disabled item is reached, so its label is heard.
    await user.keyboard("{ArrowDown}");
    expect(focusedLabel()).toBe("Start");
    expect(document.activeElement).toHaveAttribute("aria-disabled", "true");
    await user.keyboard("{End}");
    expect(focusedLabel()).toBe("Settings");
    await user.keyboard("{Home}");
    expect(focusedLabel()).toBe("Open chat");
  });

  it("jumps to what is typed: one letter again moves on, more letters narrow it", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    const user = userEvent.setup();
    render(FloatingHarness);
    menuButton().focus();
    await user.keyboard("{ArrowDown}");
    await user.keyboard("s");
    expect(focusedLabel()).toBe("Start");
    await user.keyboard("s");
    expect(focusedLabel()).toBe("Stop");
    await user.keyboard("s");
    expect(focusedLabel()).toBe("Start automatically");
    vi.advanceTimersByTime(600);
    await user.keyboard("se");
    expect(focusedLabel()).toBe("Settings");
    vi.advanceTimersByTime(600);
    await user.keyboard("r");
    expect(focusedLabel()).toBe("Restart");
  });

  it("closes once an item is chosen, and focus goes back to its button", async () => {
    const user = userEvent.setup();
    const onselect = vi.fn();
    render(FloatingHarness, { onselect });
    await user.click(menuButton());
    await user.click(screen.getByRole("menuitem", { name: "Stop" }));
    expect(onselect).toHaveBeenCalledWith("Stop");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(menuButton()).toHaveFocus();
    expect(menuButton()).toHaveAttribute("aria-expanded", "false");
  });

  it("ignores a disabled item, and keeps the menu open for a checkbox item", async () => {
    const user = userEvent.setup();
    const onselect = vi.fn();
    render(FloatingHarness, { onselect });
    await user.click(menuButton());
    await user.click(screen.getByRole("menuitem", { name: "Start" }));
    expect(onselect).not.toHaveBeenCalled();
    const autostart = screen.getByRole("menuitemcheckbox", { name: "Start automatically" });
    expect(autostart).toHaveAttribute("aria-checked", "false");
    autostart.focus();
    await user.keyboard("{Enter}");
    expect(autostart).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("menu")).toBeInTheDocument();
  });

  it("closes on Esc, on Tab, and on a press outside, but not on a press on its button", async () => {
    const user = userEvent.setup();
    render(FloatingHarness);
    menuButton().focus();
    await user.keyboard("{Enter}");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(menuButton()).toHaveFocus();

    await user.keyboard("{Enter}");
    // Tab leaves for the menu button, which the next Tab moves on from.
    await fireEvent.keyDown(screen.getByRole("menuitem", { name: "Open chat" }), { key: "Tab" });
    expect(screen.queryByRole("menu")).toBeNull();
    expect(menuButton()).toHaveFocus();

    await user.click(menuButton());
    await user.click(screen.getByRole("button", { name: "After" }));
    expect(screen.queryByRole("menu")).toBeNull();

    await user.click(menuButton());
    await user.click(menuButton());
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("highlights the item under the pointer by focusing it", async () => {
    const user = userEvent.setup();
    render(FloatingHarness);
    await user.click(menuButton());
    const restart = screen.getByRole("menuitem", { name: "Restart" });
    await user.hover(restart);
    expect(restart).toHaveFocus();
    await user.unhover(restart);
    expect(screen.getByRole("menu")).toHaveFocus();
  });
});

describe("Popover", () => {
  it("opens as a labelled dialog that leaves the page live, focusing its first control", async () => {
    const user = userEvent.setup();
    const { container } = render(FloatingHarness);
    const button = screen.getByRole("button", { name: "Model" });
    expect(button).toHaveAttribute("aria-haspopup", "dialog");
    await user.click(button);
    const popover = screen.getByRole("dialog", { name: "Model for atlas" });
    expect(popover).not.toHaveAttribute("aria-modal");
    expect(button).toHaveAttribute("aria-controls", popover.id);
    expect(screen.getByRole("button", { name: "claude-9" })).toHaveFocus();
    expect(container).not.toHaveAttribute("inert");
  });

  it("closes on Esc and on a press outside, returning focus to its button", async () => {
    const user = userEvent.setup();
    render(FloatingHarness);
    const button = screen.getByRole("button", { name: "Model" });
    await user.click(button);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(button).toHaveFocus();
    await user.click(button);
    await user.click(screen.getByRole("button", { name: "Before" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("keeps Tab inside until it passes either end, then hands focus back to its button", async () => {
    const user = userEvent.setup();
    render(FloatingHarness);
    const button = screen.getByRole("button", { name: "Model" });
    await user.click(button);
    await user.tab();
    expect(screen.getByRole("button", { name: "claude-9-fast" })).toHaveFocus();
    await fireEvent.keyDown(document.activeElement as Element, { key: "Tab" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(button).toHaveFocus();

    await user.click(button);
    await fireEvent.keyDown(document.activeElement as Element, { key: "Tab", shiftKey: true });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(button).toHaveFocus();
  });

  it("closes when a dialog opens over the page", async () => {
    const user = userEvent.setup();
    render(FloatingHarness);
    await user.click(screen.getByRole("button", { name: "Model" }));
    expect(screen.getByRole("dialog", { name: "Model for atlas" })).toBeInTheDocument();
    screen.getByRole("button", { name: "Open dialog" }).click();
    await act();
    expect(screen.queryByRole("dialog", { name: "Model for atlas" })).toBeNull();
    expect(screen.getByRole("dialog", { name: "Inside" })).toBeInTheDocument();
  });
});

describe("Tooltips", () => {
  it("shows an icon button's label when keyboard focus reaches it, and hides it on blur", async () => {
    const user = userEvent.setup();
    render(FloatingHarness);
    screen.getByRole("button", { name: "Before" }).focus();
    await user.tab();
    await user.tab();
    await user.tab();
    expect(screen.getByRole("button", { name: "Settings" })).toHaveFocus();
    const tooltip = screen.getByRole("tooltip");
    expect(tooltip).toHaveTextContent("Settings");
    expect(tooltip.closest("[data-overlay-host]")).not.toBeNull();
    await user.tab();
    expect(screen.getByRole("tooltip")).toHaveTextContent("Copy the address");
    await user.tab();
    expect(screen.queryByRole("tooltip")).toBeNull();
    // Hiding never moves focus.
    expect(screen.getByRole("button", { name: "Open dialog" })).toHaveFocus();
  });

  it("describes the control with its text when that says more than the control's name", async () => {
    const user = userEvent.setup();
    render(FloatingHarness);
    screen.getByRole("button", { name: "Model" }).focus();
    await user.tab();
    expect(screen.getByRole("button", { name: "Settings" })).not.toHaveAttribute(
      "aria-describedby",
    );
    await user.tab();
    const copy = screen.getByRole("button", { name: "Copy" });
    expect(copy).toHaveAccessibleDescription("Copy the address");
    await user.tab();
    expect(copy).not.toHaveAttribute("aria-describedby");
  });

  it("shows after a pointer rests, and goes on a press or when the pointer leaves", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
    // Long after any tooltip closed, so this one waits for the pointer to rest.
    vi.setSystemTime(Date.now() + 60_000);
    const user = userEvent.setup({ advanceTimers: (ms) => vi.advanceTimersByTime(ms) });
    render(FloatingHarness);
    const settings = screen.getByRole("button", { name: "Settings" });
    await user.hover(settings);
    expect(screen.queryByRole("tooltip")).toBeNull();
    await act(() => vi.advanceTimersByTime(500));
    expect(screen.getByRole("tooltip")).toHaveTextContent("Settings");
    await user.pointer({ keys: "[MouseLeft>]", target: settings });
    expect(screen.queryByRole("tooltip")).toBeNull();
    await user.pointer({ keys: "[/MouseLeft]", target: settings });
    // A mouse press focuses the button, but only keyboard focus shows a tooltip.
    expect(screen.queryByRole("tooltip")).toBeNull();
    await user.unhover(settings);
    await user.hover(settings);
    await act(() => vi.advanceTimersByTime(500));
    expect(screen.getByRole("tooltip")).toBeInTheDocument();
    await user.unhover(settings);
    expect(screen.queryByRole("tooltip")).toBeNull();
  });

  it("keeps Tab inside a dialog while a tooltip shows on its last control", async () => {
    const user = userEvent.setup();
    render(FloatingHarness);
    screen.getByRole("button", { name: "Open dialog" }).focus();
    await user.keyboard("{Enter}");
    // The tooltip is a layer above the dialog, but holds no focus: Tab is still the dialog's.
    expect(screen.getByRole("button", { name: "Reload" })).toHaveFocus();
    expect(screen.getByRole("tooltip")).toHaveTextContent("Reload");

    await user.tab();
    expect(screen.getByRole("button", { name: "Close" })).toHaveFocus();
    await user.tab({ shift: true });
    expect(screen.getByRole("button", { name: "Reload" })).toHaveFocus();
  });

  it("takes the first Esc, before the dialog under it", async () => {
    const user = userEvent.setup();
    render(FloatingHarness);
    screen.getByRole("button", { name: "Open dialog" }).focus();
    await user.keyboard("{Enter}");
    const dialog = screen.getByRole("dialog", { name: "Inside" });
    // Opened from the keyboard, the dialog's first control shows its tooltip.
    expect(screen.getByRole("button", { name: "Reload" })).toHaveFocus();
    expect(screen.getByRole("tooltip")).toHaveTextContent("Reload");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("tooltip")).toBeNull();
    expect(dialog).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
