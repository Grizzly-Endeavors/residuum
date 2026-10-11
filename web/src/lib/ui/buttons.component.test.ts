import userEvent from "@testing-library/user-event";
import type { Snippet } from "svelte";
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "../../test/component";
import { htmlSnippet } from "../../test/snippets";
import TooltipHarness from "../../test/ui/TooltipHarness.svelte";
import Button from "./Button.svelte";
import IconButton from "./IconButton.svelte";

const label = (text: string): Snippet => htmlSnippet(`<span>${text}</span>`);

describe("Button", () => {
  it("is a secondary, medium, non-submitting button unless told otherwise", () => {
    render(Button, { children: label("New agent") });
    const button = screen.getByRole("button", { name: "New agent" });
    expect(button).toHaveAttribute("type", "button");
    expect(button).toHaveAttribute("data-variant", "secondary");
    expect(button).toHaveAttribute("data-size", "md");
  });

  it("takes each variant and size", () => {
    render(Button, {
      variant: "danger",
      size: "sm",
      type: "submit",
      children: label("Disconnect"),
    });
    const button = screen.getByRole("button", { name: "Disconnect" });
    expect(button).toHaveAttribute("data-variant", "danger");
    expect(button).toHaveAttribute("data-size", "sm");
    expect(button).toHaveAttribute("type", "submit");
  });

  it("runs its action from a click, Enter and Space", async () => {
    const user = userEvent.setup();
    const onclick = vi.fn();
    render(Button, { onclick, children: label("Save changes") });
    const button = screen.getByRole("button", { name: "Save changes" });
    await user.click(button);
    button.focus();
    await user.keyboard("{Enter}");
    await user.keyboard(" ");
    expect(onclick).toHaveBeenCalledTimes(3);
  });

  it("ignores presses while disabled", async () => {
    const user = userEvent.setup();
    const onclick = vi.fn();
    render(Button, { onclick, disabled: true, children: label("Save changes") });
    const button = screen.getByRole("button", { name: "Save changes" });
    expect(button).toBeDisabled();
    await user.click(button);
    expect(onclick).not.toHaveBeenCalled();
  });

  it("stays focusable but ignores presses while loading, with a spinner for its icon", async () => {
    const user = userEvent.setup();
    const onclick = vi.fn();
    const { container } = render(Button, {
      onclick,
      loading: true,
      icon: "plus",
      children: label("Save changes"),
    });
    const button = screen.getByRole("button", { name: "Save changes" });
    expect(button).toBeEnabled();
    expect(button).toHaveAttribute("aria-busy", "true");
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(container.querySelector(".ui-spinner")).not.toBeNull();
    expect(container.querySelector("svg")).toBeNull();

    await user.click(button);
    button.focus();
    await user.keyboard("{Enter}");
    expect(onclick).not.toHaveBeenCalled();
    expect(button).toHaveFocus();
  });

  it("draws its icon before the label", () => {
    const { container } = render(Button, { icon: "plus", children: label("New agent") });
    const button = container.querySelector("button");
    expect(button?.firstElementChild?.tagName.toLowerCase()).toBe("svg");
  });
});

describe("IconButton", () => {
  it("is named by its label, since it shows only an icon", () => {
    render(IconButton, { icon: "settings", label: "Settings" });
    const button = screen.getByRole("button", { name: "Settings" });
    expect(button).toHaveAttribute("data-variant", "quiet");
    expect(button).not.toHaveAttribute("aria-pressed");
  });

  it("reports its pressed state when it is a toggle", () => {
    render(IconButton, { icon: "settings", label: "Settings", pressed: false });
    expect(screen.getByRole("button", { name: "Settings" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  });

  it("runs its action from the keyboard and ignores presses while loading", async () => {
    const user = userEvent.setup();
    const onclick = vi.fn();
    const { rerender } = render(IconButton, { icon: "send", label: "Send", onclick });
    screen.getByRole("button", { name: "Send" }).focus();
    await user.keyboard("{Enter}");
    expect(onclick).toHaveBeenCalledOnce();

    await rerender({ loading: true });
    await user.keyboard("{Enter}");
    expect(onclick).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: "Send" })).toHaveAttribute("aria-busy", "true");
  });

  it("asks a tooltip provider for its label", () => {
    const attached: string[] = [];
    const provider = vi.fn((text: string) => (node: HTMLElement) => {
      attached.push(`${node.getAttribute("aria-label") ?? ""}:${text}`);
    });
    render(TooltipHarness, { provider });
    expect(attached).toEqual(["Settings:Settings"]);
  });

  it("uses its own tooltip text, or none, when told to", () => {
    const provider = vi.fn(() => () => undefined);
    const first = render(TooltipHarness, { provider, tooltip: "Open settings" });
    expect(provider).toHaveBeenCalledWith("Open settings");
    first.unmount();

    provider.mockClear();
    render(TooltipHarness, { provider, tooltip: false });
    expect(provider).not.toHaveBeenCalled();
  });

  it("shows no tooltip without a provider", () => {
    render(IconButton, { icon: "settings", label: "Settings" });
    expect(screen.getByRole("button", { name: "Settings" })).not.toHaveAttribute("title");
  });
});
