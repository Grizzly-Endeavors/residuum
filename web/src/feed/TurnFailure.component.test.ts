import { beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { render, screen } from "../test/component";
import type { TurnFailureFeedItem } from "../lib/types";
import TurnFailure from "./TurnFailure.svelte";

const { ws } = vi.hoisted(() => ({
  ws: { agent: "atlas" as string | null, retryTurn: vi.fn() },
}));
vi.mock("../lib/ws.svelte", () => ({ ws }));

const failure: TurnFailureFeedItem = {
  id: 1,
  kind: "turn-failure",
  turnId: "t1",
  message: "The model provider didn't answer. Try sending your message again in a moment.",
  details: "model call failed after 3 attempts\n  caused by: provider returned 503",
  retry: {},
};

beforeEach(() => {
  ws.agent = "atlas";
  ws.retryTurn.mockReset();
});

describe("a turn that couldn't finish", () => {
  it("says so in plain words, with what went wrong", () => {
    render(TurnFailure, { item: failure, agent: "atlas" });
    expect(screen.getByText("atlas couldn't finish this reply")).toBeVisible();
    expect(screen.getByText(failure.message)).toBeVisible();
  });

  it("keeps the technical cause behind Details", async () => {
    render(TurnFailure, { item: failure, agent: "atlas" });
    const details = screen.getByRole("button", { name: "Details" });
    expect(details).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByText(/caused by: provider returned 503/)).not.toBeVisible();

    await userEvent.click(details);
    expect(screen.getByText(/caused by: provider returned 503/)).toBeVisible();
  });

  it("offers no Details when there is no cause to show", () => {
    render(TurnFailure, { item: { ...failure, details: undefined }, agent: "atlas" });
    expect(screen.queryByRole("button", { name: "Details" })).toBeNull();
  });

  it("asks the agent to run the turn again with Try again, naming the turn", async () => {
    render(TurnFailure, { item: failure, agent: "atlas" });
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));

    expect(ws.retryTurn).toHaveBeenCalledExactlyOnceWith("t1");
  });

  it("waits for the agent's answer once Try again is asked, without offering it again", () => {
    render(TurnFailure, { item: { ...failure, retry: { asked: true } }, agent: "atlas" });
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
    expect(screen.getByText(failure.message)).toBeVisible();
  });

  it("gives the agent's reason in place of Try again when it won't run the turn again", () => {
    const reason = "That message can't be retried anymore because the conversation has moved on.";
    render(TurnFailure, { item: { ...failure, retry: { refused: reason } }, agent: "atlas" });
    expect(screen.getByText(reason)).toBeVisible();
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
  });

  it("offers no Try again without the message that began the turn", () => {
    render(TurnFailure, { item: { ...failure, retry: undefined }, agent: "atlas" });
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
  });

  it("offers no Try again on a conversation that isn't the bound agent's", () => {
    ws.agent = "scout";
    render(TurnFailure, { item: failure, agent: "atlas" });
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
  });
});
