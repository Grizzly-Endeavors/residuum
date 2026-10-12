import { describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { render, screen } from "../../test/component";
import type { WaitingMessage } from "../../lib/types";
import WaitingMessages from "./WaitingMessages.svelte";

const LOGS: WaitingMessage = {
  id: "m2",
  content: "also check the logs",
  endpoint: "ws",
  connection: 1,
};

function show(
  messages: WaitingMessage[],
  offline = false,
): { onsendnow: ReturnType<typeof vi.fn>; onmultitask: ReturnType<typeof vi.fn> } {
  const onsendnow = vi.fn();
  const onmultitask = vi.fn();
  render(WaitingMessages, {
    props: { messages, agentName: "atlas", offline, onsendnow, onmultitask },
  });
  return { onsendnow, onmultitask };
}

describe("messages waiting for the agent", () => {
  it("shows nothing when none wait", () => {
    show([]);
    expect(screen.queryByRole("region")).toBeNull();
  });

  it("lists each under who it waits for, with Send now and Multitask for it", async () => {
    const { onsendnow, onmultitask } = show([LOGS]);
    const region = screen.getByRole("region", { name: "Waiting for atlas to read" });
    expect(region).toHaveTextContent("also check the logs");

    const send = screen.getByRole("button", { name: "Send now" });
    expect(send).toHaveAccessibleDescription("also check the logs");
    await userEvent.click(send);
    expect(onsendnow).toHaveBeenCalledWith(LOGS);
    await userEvent.click(screen.getByRole("button", { name: "Multitask" }));
    expect(onmultitask).toHaveBeenCalledWith(LOGS);
  });

  it("names the person behind one from a chat interface", () => {
    show([
      {
        ...LOGS,
        endpoint: "telegram",
        sender: { name: "Alex", id: "42", interface: "telegram", location: "direct message" },
      },
    ]);
    expect(screen.getByText("Alex · telegram · direct message")).toBeInTheDocument();
  });

  it("asks nothing of a message while the agent's connection is down", () => {
    show([LOGS], true);
    expect(screen.getByRole("button", { name: "Send now" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Multitask" })).toBeDisabled();
  });

  it("shows which ask is under way, and holds the other back", () => {
    show([{ ...LOGS, asked: "withdraw" }]);
    expect(screen.getByRole("button", { name: "Multitask" })).toHaveAttribute("aria-busy", "true");
    expect(screen.getByRole("button", { name: "Send now" })).toBeDisabled();
  });
});
