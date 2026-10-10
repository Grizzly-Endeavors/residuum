import { describe, expect, it } from "vitest";
import { render, screen } from "../test/component";
import type { AssistantFeedItem } from "../lib/types";
import AssistantMessage from "./AssistantMessage.svelte";

function reply(more: Partial<AssistantFeedItem> = {}): AssistantFeedItem {
  return { id: 1, kind: "assistant", content: "The port is set twice.", ...more };
}

describe("an agent's reply", () => {
  it("is its text, with nothing added to a finished one", () => {
    const { container } = render(AssistantMessage, { item: reply(), agent: "atlas" });
    expect(screen.getByText("The port is set twice.")).toBeVisible();
    expect(container.querySelector("[data-streaming]")).toBeNull();
    expect(container.querySelector("[data-caret]")).toBeNull();
    expect(container.querySelector(".reply-note")).toBeNull();
  });

  it("is marked as streaming while text is still arriving, with a caret on its prose", () => {
    const { container } = render(AssistantMessage, {
      item: reply({ streaming: true }),
      agent: "atlas",
    });
    expect(container.querySelector(".reply")).toHaveAttribute("data-streaming");
    expect(container.querySelector(".prose")).toHaveAttribute("data-caret");
    // The text itself is the same message, in its Markdown.
    expect(screen.getByText("The port is set twice.")).toBeVisible();
  });

  it("says a stop cut it short, or that the agent stopped under it", () => {
    const { unmount } = render(AssistantMessage, {
      item: reply({ cut: "stopped" }),
      agent: "atlas",
    });
    expect(screen.getByText("Stopped here")).toBeVisible();
    unmount();

    render(AssistantMessage, { item: reply({ cut: "interrupted" }), agent: "atlas" });
    expect(screen.getByText("Cut off here")).toBeVisible();
  });

  it("says where a reply that went to a chat interface was sent", () => {
    render(AssistantMessage, { item: reply({ deliveredTo: "telegram" }), agent: "atlas" });
    expect(screen.getByText("Sent to Telegram")).toBeVisible();
  });
});
