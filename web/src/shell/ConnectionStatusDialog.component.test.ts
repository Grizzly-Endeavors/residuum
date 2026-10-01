import userEvent from "@testing-library/user-event";
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { router } from "../lib/router.svelte";
import { render, screen, settle } from "../test/component";
import ConnectionStatusDialog from "./ConnectionStatusDialog.svelte";

const { disk } = vi.hoisted(() => ({ disk: { text: "" } }));

vi.mock("../lib/config-coordinator", () => ({
  agentConfigFile: (agent: string, name: string) => ({ kind: "agent", agent, name }),
  configCoordinator: {
    read: () => Promise.resolve(disk.text),
  },
}));

function stubClipboard(writeText: (text: string) => Promise<void>): ReturnType<typeof vi.fn> {
  const spy = vi.fn(writeText);
  Object.defineProperty(navigator, "clipboard", { value: { writeText: spy }, configurable: true });
  return spy;
}

beforeAll(() => {
  router.startForOverlays();
});

afterAll(() => {
  router.stop();
});

beforeEach(() => {
  disk.text = `[models]\nmain = "anthropic/claude-sonnet-4-6"\n`;
});

function open(agent: string | null = "atlas"): void {
  render(ConnectionStatusDialog, {
    open: true,
    agent,
    agentState: agent === null ? null : "running",
    hubConnection: "connected",
    agentConnection: "connected",
  });
}

describe("ConnectionStatusDialog", () => {
  it("shows the connection and the main model, and copies them", async () => {
    const writeText = stubClipboard(() => Promise.resolve());
    open();
    const dialog = screen.getByRole("dialog", { name: "Connection status" });
    expect(dialog).toHaveTextContent("atlas");
    expect(dialog).toHaveTextContent("Connected");
    expect(await screen.findByText("claude-sonnet-4-6")).toBeInTheDocument();
    const copy = screen.getByRole("button", { name: "Copy" });
    expect(copy).toBeEnabled();
    await userEvent.click(copy);
    expect(writeText).toHaveBeenCalledWith(
      "Residuum: Connected\nAgent: atlas\nConnection: Connected\nModel: claude-sonnet-4-6",
    );
    expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
  });

  it("leaves the model out when no agent is open", async () => {
    open(null);
    expect(screen.getByRole("dialog", { name: "Connection status" })).toHaveTextContent(
      "None open",
    );
    expect(screen.queryByText("Model")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy" })).toBeEnabled();
    await settle();
  });
});
