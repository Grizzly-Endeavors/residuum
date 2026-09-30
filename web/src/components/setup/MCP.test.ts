import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "../../test/component";
import MCP from "./MCP.svelte";
import type { SetupWizardState } from "../../lib/types";

function wizard(overrides: Partial<SetupWizardState> = {}): SetupWizardState {
  return { mcpServers: [], ...overrides } as SetupWizardState;
}

describe("Setup MCP step", () => {
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

    expect(
      screen.getByText("Couldn't load the MCP server catalog. Residuum isn't reachable."),
    ).toBeTruthy();
    expect(screen.queryByText("No catalog entries available.")).toBeNull();

    await fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(onRetryCatalog).toHaveBeenCalledOnce();
  });

  it("shows an empty state, not an error, when the catalog legitimately has no entries", () => {
    render(MCP, {
      wizardState: wizard(),
      catalog: [],
      catalogLoading: false,
      catalogError: null,
      onRetryCatalog: () => {},
      onNext: () => {},
      onBack: () => {},
    });

    expect(screen.getByText("No catalog entries available.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
  });
});
