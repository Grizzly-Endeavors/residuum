import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi, type Mock } from "vitest";
import { render, screen } from "../../test/component";
import { htmlSnippet } from "../../test/snippets";
import Disclosure from "./Disclosure.svelte";
import TabsHarness from "../../test/ui/TabsHarness.svelte";

describe("Disclosure", () => {
  it("starts closed, with its content hidden and controlled by the button", () => {
    render(Disclosure, { summary: "More options", children: htmlSnippet("<p>Port</p>") });
    const button = screen.getByRole("button", { name: "More options" });
    expect(button).toHaveAttribute("aria-expanded", "false");
    const panel = document.getElementById(button.getAttribute("aria-controls") ?? "");
    expect(panel).not.toBeNull();
    expect(panel).not.toBeVisible();
    expect(panel).toHaveTextContent("Port");
  });

  it("opens and closes with a click, Enter and Space", async () => {
    const user = userEvent.setup();
    const ontoggle = vi.fn();
    render(Disclosure, {
      summary: "Details",
      ontoggle,
      children: htmlSnippet("<p>provider rejected the model</p>"),
    });
    const button = screen.getByRole("button", { name: "Details" });

    await user.click(button);
    expect(button).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("provider rejected the model")).toBeVisible();

    await user.keyboard("{Enter}");
    expect(button).toHaveAttribute("aria-expanded", "false");
    await user.keyboard(" ");
    expect(button).toHaveAttribute("aria-expanded", "true");
    expect(ontoggle.mock.calls).toEqual([[true], [false], [true]]);
  });

  it("can start open", () => {
    render(Disclosure, { summary: "More options", open: true, children: htmlSnippet("<p>x</p>") });
    expect(screen.getByRole("button", { name: "More options" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(screen.getByText("x")).toBeVisible();
  });
});

describe("Tabs", () => {
  const TABS = [
    { value: "inbox", label: "Inbox", count: 3 },
    { value: "later", label: "Later", disabled: true },
    { value: "archived", label: "Archived" },
    { value: "sent", label: "Sent" },
  ] as const;

  interface Rendered {
    onchange: Mock;
    tab: (name: string | RegExp) => HTMLElement;
  }

  function renderTabs(selected = "inbox"): Rendered {
    const onchange = vi.fn();
    render(TabsHarness, { tabs: TABS, selected, onchange });
    return { onchange, tab: (name: string | RegExp) => screen.getByRole("tab", { name }) };
  }

  it("names the tab list and labels the panel with the selected tab", () => {
    const { tab } = renderTabs();
    expect(screen.getByRole("tablist", { name: "Inbox views" })).toBeInTheDocument();
    const inbox = tab(/^Inbox/);
    expect(inbox).toHaveAttribute("aria-selected", "true");
    const tabpanel = screen.getByRole("tabpanel");
    expect(tabpanel).toHaveAccessibleName(inbox.textContent.trim());
    expect(inbox).toHaveAttribute("aria-controls", tabpanel.id);
    expect(tabpanel).toHaveTextContent("Showing inbox");
  });

  it("is one tab stop, on the selected tab", () => {
    const { tab } = renderTabs("archived");
    expect(tab("Archived")).toHaveAttribute("tabindex", "0");
    expect(tab(/^Inbox/)).toHaveAttribute("tabindex", "-1");
    expect(tab("Sent")).toHaveAttribute("tabindex", "-1");
  });

  it("moves and selects with the arrow keys, skipping disabled tabs and wrapping", async () => {
    const user = userEvent.setup();
    const { tab, onchange } = renderTabs();
    tab(/^Inbox/).focus();

    await user.keyboard("{ArrowRight}");
    expect(tab("Archived")).toHaveFocus();
    expect(tab("Archived")).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tabpanel")).toHaveTextContent("Showing archived");

    await user.keyboard("{ArrowRight}{ArrowRight}");
    expect(tab(/^Inbox/)).toHaveFocus();

    await user.keyboard("{ArrowLeft}");
    expect(tab("Sent")).toHaveFocus();
    expect(onchange.mock.calls).toEqual([["archived"], ["sent"], ["inbox"], ["sent"]]);
  });

  it("jumps to the ends with Home and End", async () => {
    const user = userEvent.setup();
    const { tab } = renderTabs("archived");
    tab("Archived").focus();
    await user.keyboard("{End}");
    expect(tab("Sent")).toHaveFocus();
    await user.keyboard("{Home}");
    expect(tab(/^Inbox/)).toHaveFocus();
  });

  it("selects on click, and not a disabled tab", async () => {
    const user = userEvent.setup();
    const { tab, onchange } = renderTabs();
    await user.click(tab("Later"));
    expect(onchange).not.toHaveBeenCalled();
    await user.click(tab("Sent"));
    expect(tab("Sent")).toHaveAttribute("aria-selected", "true");
    expect(onchange).toHaveBeenCalledWith("sent");
  });

  it("shows a tab's count", () => {
    const { tab } = renderTabs();
    expect(tab(/^Inbox/)).toHaveTextContent("3");
  });
});
