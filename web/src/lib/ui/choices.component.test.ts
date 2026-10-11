import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi, type Mock } from "vitest";
import { render, screen } from "../../test/component";
import SegmentedControl from "./SegmentedControl.svelte";
import Toggle from "./Toggle.svelte";

const THINKING = [
  { value: "off", label: "Off" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium", disabled: true },
  { value: "high", label: "High" },
] as const;

describe("Toggle", () => {
  it("is a switch named by its label and described by its hint", () => {
    render(Toggle, {
      label: "Let others talk to this agent",
      hint: "Off: only you can talk to atlas.",
    });
    const toggle = screen.getByRole("switch", { name: "Let others talk to this agent" });
    expect(toggle).toHaveAttribute("aria-checked", "false");
    expect(toggle).toHaveAccessibleDescription("Off: only you can talk to atlas.");
  });

  it("toggles with a click, Space and Enter, reporting each new state", async () => {
    const user = userEvent.setup();
    const onchange = vi.fn();
    render(Toggle, { label: "Regular checks", onchange });
    const toggle = screen.getByRole("switch", { name: "Regular checks" });

    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "true");
    await user.keyboard(" ");
    expect(toggle).toHaveAttribute("aria-checked", "false");
    await user.keyboard("{Enter}");
    expect(toggle).toHaveAttribute("aria-checked", "true");
    expect(onchange.mock.calls).toEqual([[true], [false], [true]]);
  });

  it("toggles from its label too", async () => {
    const user = userEvent.setup();
    render(Toggle, { label: "Regular checks", checked: true });
    await user.click(screen.getByText("Regular checks"));
    expect(screen.getByRole("switch", { name: "Regular checks" })).toHaveAttribute(
      "aria-checked",
      "false",
    );
  });

  it("stays put while disabled or loading", async () => {
    const user = userEvent.setup();
    const onchange = vi.fn();
    const { rerender } = render(Toggle, { label: "Autostart", disabled: true, onchange });
    const toggle = screen.getByRole("switch", { name: "Autostart" });
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "false");

    await rerender({ disabled: false, loading: true });
    expect(toggle).toHaveAttribute("aria-busy", "true");
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "false");
    expect(onchange).not.toHaveBeenCalled();
  });

  it("wires an error", () => {
    render(Toggle, { label: "Listener", error: "Port 7701 is in use." });
    const toggle = screen.getByRole("switch", { name: "Listener" });
    expect(toggle).toHaveAttribute("aria-invalid", "true");
    expect(toggle).toHaveAccessibleDescription("Port 7701 is in use.");
  });
});

describe("SegmentedControl", () => {
  interface Rendered {
    onchange: Mock;
    radio: (name: string) => HTMLElement;
  }

  function renderThinking(props: Record<string, unknown> = {}): Rendered {
    const onchange = vi.fn();
    render(SegmentedControl, {
      label: "Thinking",
      value: "low",
      options: THINKING,
      onchange,
      ...props,
    });
    return { onchange, radio: (name: string) => screen.getByRole("radio", { name }) };
  }

  it("is a radio group named by its label, with the chosen option checked", () => {
    const { radio } = renderThinking({ hint: "More thinking costs more." });
    const group = screen.getByRole("radiogroup", { name: "Thinking" });
    expect(group).toHaveAccessibleDescription("More thinking costs more.");
    expect(radio("Low")).toHaveAttribute("aria-checked", "true");
    expect(radio("Off")).toHaveAttribute("aria-checked", "false");
    expect(radio("Medium")).toBeDisabled();
  });

  it("is one tab stop, on the chosen option", () => {
    const { radio } = renderThinking();
    expect(radio("Low")).toHaveAttribute("tabindex", "0");
    for (const name of ["Off", "Medium", "High"]) {
      expect(radio(name)).toHaveAttribute("tabindex", "-1");
    }
  });

  it("moves and chooses with the arrow keys, skipping disabled options and wrapping", async () => {
    const user = userEvent.setup();
    const { radio, onchange } = renderThinking();
    radio("Low").focus();

    await user.keyboard("{ArrowRight}");
    expect(radio("High")).toHaveFocus();
    expect(radio("High")).toHaveAttribute("aria-checked", "true");

    await user.keyboard("{ArrowDown}");
    expect(radio("Off")).toHaveFocus();

    await user.keyboard("{ArrowLeft}");
    expect(radio("High")).toHaveFocus();

    await user.keyboard("{ArrowUp}");
    expect(radio("Low")).toHaveFocus();
    expect(radio("Low")).toHaveAttribute("tabindex", "0");
    expect(onchange.mock.calls).toEqual([["high"], ["off"], ["high"], ["low"]]);
  });

  it("jumps to the ends with Home and End", async () => {
    const user = userEvent.setup();
    const { radio } = renderThinking();
    radio("Low").focus();
    await user.keyboard("{End}");
    expect(radio("High")).toHaveFocus();
    await user.keyboard("{Home}");
    expect(radio("Off")).toHaveFocus();
    expect(radio("Off")).toHaveAttribute("aria-checked", "true");
  });

  it("chooses on click", async () => {
    const user = userEvent.setup();
    const { radio, onchange } = renderThinking();
    await user.click(radio("Off"));
    expect(radio("Off")).toHaveAttribute("aria-checked", "true");
    expect(onchange).toHaveBeenCalledWith("off");
  });

  it("ignores everything while disabled", async () => {
    const user = userEvent.setup();
    const { radio, onchange } = renderThinking({ disabled: true });
    expect(screen.getByRole("radiogroup", { name: "Thinking" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await user.click(radio("Off"));
    expect(onchange).not.toHaveBeenCalled();
  });

  it("wires an error to the group", () => {
    renderThinking({ error: "This model can't think." });
    const group = screen.getByRole("radiogroup", { name: "Thinking" });
    expect(group).toHaveAttribute("aria-invalid", "true");
    expect(group).toHaveAccessibleDescription("This model can't think.");
  });
});
