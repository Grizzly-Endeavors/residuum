import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "../../test/component";
import NumberField from "./NumberField.svelte";
import SecretField from "./SecretField.svelte";
import SelectField from "./SelectField.svelte";
import TextField from "./TextField.svelte";

const PROVIDERS = [
  { value: "anthropic", label: "Anthropic" },
  { value: "openai", label: "OpenAI" },
  { value: "ollama", label: "Ollama", disabled: true },
];

describe("TextField", () => {
  it("labels its box and describes it with the hint", () => {
    render(TextField, { label: "Name", hint: "Lowercase letters, numbers and hyphens." });
    const box = screen.getByRole("textbox", { name: "Name" });
    expect(box).toHaveAccessibleDescription("Lowercase letters, numbers and hyphens.");
    expect(box).not.toHaveAttribute("aria-invalid");
  });

  it("has no description without a hint or error", () => {
    render(TextField, { label: "Name" });
    expect(screen.getByRole("textbox", { name: "Name" })).not.toHaveAttribute("aria-describedby");
  });

  it("marks the box invalid and describes it with the error after the hint", () => {
    render(TextField, {
      label: "Name",
      hint: "Lowercase letters only.",
      error: "Use lowercase letters only.",
    });
    const box = screen.getByRole("textbox", { name: "Name" });
    expect(box).toHaveAttribute("aria-invalid", "true");
    expect(box).toHaveAccessibleDescription("Lowercase letters only. Use lowercase letters only.");
  });

  it("keeps a hidden label for assistive technology", () => {
    render(TextField, { label: "Search files", labelHidden: true });
    expect(screen.getByRole("textbox", { name: "Search files" })).toBeInTheDocument();
  });

  it("takes typing and passes other attributes to the box", async () => {
    const user = userEvent.setup();
    const oninput = vi.fn();
    render(TextField, { label: "Name", oninput, placeholder: "research-buddy", maxlength: 20 });
    const box = screen.getByRole("textbox", { name: "Name" });
    expect(box).toHaveAttribute("placeholder", "research-buddy");
    expect(box).toHaveAttribute("maxlength", "20");
    await user.type(box, "scout");
    expect(box).toHaveValue("scout");
    expect(oninput).toHaveBeenCalledTimes(5);
  });

  it("draws multi-line text as a text area", () => {
    const { container } = render(TextField, { label: "Notes", multiline: true, rows: 4 });
    const area = screen.getByRole("textbox", { name: "Notes" });
    expect(area.tagName).toBe("TEXTAREA");
    expect(area).toHaveAttribute("rows", "4");
    expect(container.querySelector("input")).toBeNull();
  });

  it("can't be edited while disabled", () => {
    render(TextField, { label: "Name", value: "atlas", disabled: true });
    expect(screen.getByRole("textbox", { name: "Name" })).toBeDisabled();
  });
});

describe("NumberField", () => {
  it("is a spin button described by its unit and hint", () => {
    render(NumberField, {
      label: "Earlier messages to read",
      value: 20,
      unit: "messages",
      hint: "Read before replying.",
    });
    const box = screen.getByRole("spinbutton", { name: "Earlier messages to read" });
    expect(box).toHaveValue(20);
    expect(box).toHaveAccessibleDescription("messages Read before replying.");
  });

  it("takes a typed number and reports the change", async () => {
    const user = userEvent.setup();
    const onchange = vi.fn();
    render(NumberField, { label: "Port", value: 8080, onchange });
    const box = screen.getByRole("spinbutton", { name: "Port" });
    await user.clear(box);
    await user.type(box, "9000");
    await user.tab();
    expect(box).toHaveValue(9000);
    expect(onchange).toHaveBeenCalled();
  });

  it("shows an inline error", () => {
    render(NumberField, { label: "Port", value: 80, error: "Pick 1024 or higher." });
    const box = screen.getByRole("spinbutton", { name: "Port" });
    expect(box).toHaveAttribute("aria-invalid", "true");
    expect(box).toHaveAccessibleDescription("Pick 1024 or higher.");
  });
});

describe("SelectField", () => {
  it("lists its options under its label, with disabled ones unavailable", () => {
    render(SelectField, { label: "Provider", value: "openai", options: PROVIDERS });
    const select = screen.getByRole("combobox", { name: "Provider" });
    expect(select).toHaveValue("openai");
    expect(screen.getByRole("option", { name: "Ollama" })).toBeDisabled();
  });

  it("reports the chosen option", async () => {
    const user = userEvent.setup();
    const onchange = vi.fn();
    render(SelectField, { label: "Provider", value: "anthropic", options: PROVIDERS, onchange });
    const select = screen.getByRole("combobox", { name: "Provider" });
    await user.selectOptions(select, "openai");
    expect(select).toHaveValue("openai");
    expect(onchange).toHaveBeenCalledOnce();
  });

  it("groups options under their labels", () => {
    render(SelectField, {
      label: "Timezone",
      value: "Europe/Berlin",
      options: [{ value: "UTC", label: "UTC" }],
      groups: [{ label: "Europe", options: [{ value: "Europe/Berlin", label: "Europe/Berlin" }] }],
    });
    const select = screen.getByRole("combobox", { name: "Timezone" });
    expect(select).toHaveValue("Europe/Berlin");
    expect(select.querySelector("optgroup")?.getAttribute("label")).toBe("Europe");
    expect(screen.getByRole("option", { name: "UTC" })).toBeInTheDocument();
  });

  it("shows a placeholder while nothing is chosen, which can't be chosen back", () => {
    render(SelectField, {
      label: "Provider",
      options: PROVIDERS,
      placeholder: "Choose a provider",
    });
    const placeholder = screen.getByRole("option", { name: "Choose a provider" });
    expect(placeholder).toBeDisabled();
    expect((placeholder as HTMLOptionElement).selected).toBe(true);
  });

  it("is disabled and says so while its options load", () => {
    render(SelectField, { label: "Model", options: [], loading: true });
    const select = screen.getByRole("combobox", { name: "Model" });
    expect(select).toBeDisabled();
    expect(select).toHaveAttribute("aria-busy", "true");
    expect(screen.getByRole("option", { name: "Loading…" })).toBeInTheDocument();
  });

  it("wires its hint and error", () => {
    render(SelectField, {
      label: "Model",
      value: "anthropic",
      options: PROVIDERS,
      hint: "The model atlas thinks with.",
      error: "Choose another.",
    });
    const select = screen.getByRole("combobox", { name: "Model" });
    expect(select).toHaveAttribute("aria-invalid", "true");
    expect(select).toHaveAccessibleDescription("The model atlas thinks with. Choose another.");
  });
});

describe("SecretField", () => {
  it("says a stored secret is stored securely and offers Change", () => {
    render(SecretField, { label: "API key", source: { kind: "stored" } });
    const group = screen.getByRole("group", { name: "API key" });
    expect(group).toHaveTextContent("Stored securely");
    expect(screen.getByRole("button", { name: "Change API key" })).toBeInTheDocument();
    expect(screen.queryByLabelText("API key", { selector: "input" })).toBeNull();
  });

  it("names the environment variable a secret comes from and offers Replace", () => {
    render(SecretField, {
      label: "Bot token",
      source: { kind: "env", variable: "DISCORD_TOKEN" },
      hint: "Read when atlas starts.",
    });
    const group = screen.getByRole("group", { name: "Bot token" });
    expect(group).toHaveTextContent("From environment variable DISCORD_TOKEN");
    expect(group).toHaveAccessibleDescription("Read when atlas starts.");
    expect(screen.getByRole("button", { name: "Replace Bot token" })).toBeInTheDocument();
  });

  it("opens a focused password box on Change, and Cancel closes it again", async () => {
    const user = userEvent.setup();
    const oncancel = vi.fn();
    const { container } = render(SecretField, {
      label: "API key",
      source: { kind: "stored" },
      oncancel,
    });
    await user.click(screen.getByRole("button", { name: "Change API key" }));
    const box = screen.getByLabelText("API key", { selector: "input" });
    expect(box).toHaveAttribute("type", "password");
    expect(box).toHaveFocus();
    await user.type(box, "sk-new");
    expect(box).toHaveValue("sk-new");

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(oncancel).toHaveBeenCalledOnce();
    expect(container.querySelector("input")).toBeNull();
    expect(screen.getByRole("button", { name: "Change API key" })).toHaveFocus();

    await user.click(screen.getByRole("button", { name: "Change API key" }));
    expect(screen.getByLabelText("API key", { selector: "input" })).toHaveValue("");
  });

  it("opens the box from the start when nothing is saved, with no Cancel", () => {
    render(SecretField, { label: "API key", source: { kind: "none" }, placeholder: "Paste it" });
    expect(screen.getByLabelText("API key", { selector: "input" })).toHaveAttribute(
      "placeholder",
      "Paste it",
    );
    expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
  });

  it("shows and hides what was typed", async () => {
    const user = userEvent.setup();
    render(SecretField, { label: "API key", source: { kind: "none" }, value: "sk-123" });
    const reveal = screen.getByRole("button", { name: "Show API key" });
    expect(reveal).toHaveAttribute("aria-pressed", "false");
    await user.click(reveal);
    expect(reveal).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByLabelText("API key", { selector: "input" })).toHaveAttribute("type", "text");
    await user.click(reveal);
    expect(screen.getByLabelText("API key", { selector: "input" })).toHaveAttribute(
      "type",
      "password",
    );
  });

  it("wires an error to the box", () => {
    render(SecretField, {
      label: "API key",
      source: { kind: "none" },
      error: "Paste a key first.",
    });
    const box = screen.getByLabelText("API key", { selector: "input" });
    expect(box).toHaveAttribute("aria-invalid", "true");
    expect(box).toHaveAccessibleDescription("Paste a key first.");
  });
});
