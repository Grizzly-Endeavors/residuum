import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "../../test/component";
import MCP from "./MCP.svelte";
import type { McpCatalogEntry, SetupWizardState } from "../../lib/types";

// Reactive, as the wizard's own state is, so the step redraws after a change.
function wizard(overrides: Partial<SetupWizardState> = {}): SetupWizardState {
  const state = $state({ mcpServers: [], ...overrides } as SetupWizardState);
  return state;
}

function entry(name: string, fields: string[] = []): McpCatalogEntry {
  return {
    name,
    description: `${name} server`,
    command: "npx",
    args: [name],
    env: {},
    category: "tools",
    requires_input: fields.map((field) => ({ field, label: `${field} value` })),
    install_hint: "",
  } as McpCatalogEntry;
}

function renderStep(state: SetupWizardState, catalog: McpCatalogEntry[]): void {
  render(MCP, {
    wizardState: state,
    catalog,
    catalogLoading: false,
    catalogError: null,
    onRetryCatalog: () => {},
    onNext: () => {},
    onBack: () => {},
  });
}

describe("Setup tool servers step", () => {
  it("shows an error with a retry button when the catalog fails to load", async () => {
    const onRetryCatalog = vi.fn();
    render(MCP, {
      wizardState: wizard(),
      catalog: [],
      catalogLoading: false,
      catalogError: "Couldn't load the MCP server catalog. Residuum isn't reachable.",
      onRetryCatalog,
      onNext: () => {},
      onBack: () => {},
    });

    expect(screen.getByRole("alert")).toHaveTextContent(
      "Couldn't load the MCP server catalog. Residuum isn't reachable.",
    );
    expect(screen.queryByText(/has no tool servers/)).toBeNull();

    await fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(onRetryCatalog).toHaveBeenCalledOnce();
  });

  it("shows an empty state, not an error, when the catalog legitimately has no entries", () => {
    renderStep(wizard(), []);

    expect(screen.getByText(/The catalog has no tool servers to offer/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
  });

  it("adds a server with no inputs at once, and removes it again", async () => {
    const state = wizard();
    renderStep(state, [entry("fetch")]);

    await fireEvent.click(screen.getByRole("button", { name: "Add fetch" }));
    expect(state.mcpServers.map((s) => s.name)).toEqual(["fetch"]);

    await fireEvent.click(screen.getByRole("button", { name: "Remove fetch" }));
    expect(state.mcpServers).toEqual([]);
  });

  it("asks for a server's required inputs and won't add it with one left empty", async () => {
    const state = wizard();
    renderStep(state, [entry("tavily", ["env.TAVILY_API_KEY"])]);

    await fireEvent.click(screen.getByRole("button", { name: "Add tavily" }));
    await fireEvent.click(screen.getByRole("button", { name: "Add tavily" }));
    const input = screen.getByLabelText("env.TAVILY_API_KEY value");
    expect(input).toBeInvalid();
    expect(state.mcpServers).toEqual([]);

    await fireEvent.input(input, { target: { value: " tvly-123 " } });
    await fireEvent.click(screen.getByRole("button", { name: "Add tavily" }));
    expect(state.mcpServers).toEqual([
      {
        name: "tavily",
        command: "npx",
        args: ["tavily"],
        env: { TAVILY_API_KEY: "tvly-123" },
        secretEnvKeys: ["TAVILY_API_KEY"],
      },
    ]);
  });

  it("marks a server with no required inputs as having no secret env keys to strip", async () => {
    const state = wizard();
    renderStep(state, [entry("fetch")]);

    await fireEvent.click(screen.getByRole("button", { name: "Add fetch" }));
    expect(state.mcpServers).toEqual([{ name: "fetch", command: "npx", args: ["fetch"], env: {} }]);
  });
});
