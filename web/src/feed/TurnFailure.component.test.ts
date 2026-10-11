import { beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { render, screen } from "../test/component";
import type { TurnFailureFeedItem } from "../lib/types";
import TurnFailure from "./TurnFailure.svelte";

const { ws } = vi.hoisted(() => ({
  ws: { agent: "atlas" as string | null, sendChat: vi.fn() },
}));
vi.mock("../lib/ws.svelte", () => ({ ws }));

const failure: TurnFailureFeedItem = {
  id: 1,
  kind: "turn-failure",
  turnId: "t1",
  message: "The model provider didn't answer. Try sending your message again in a moment.",
  details: "model call failed after 3 attempts\n  caused by: provider returned 503",
  retry: { content: "Tidy the wiki index" },
};

beforeEach(() => {
  ws.agent = "atlas";
  ws.sendChat.mockReset();
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

  it("sends the user's message again with Try again, once", async () => {
    const images = [{ media_type: "image/png", data: "AAAA" }];
    render(TurnFailure, {
      item: { ...failure, retry: { content: "Tidy the wiki index", images } },
      agent: "atlas",
    });
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));

    expect(ws.sendChat).toHaveBeenCalledExactlyOnceWith("Tidy the wiki index", images);
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
