import { describe, expect, it } from "vitest";
import userEvent from "@testing-library/user-event";
import { render, screen } from "../test/component";
import type { ThinkingFeedItem } from "../lib/types";
import ThoughtStep from "./ThoughtStep.svelte";

function thought(more: Partial<ThinkingFeedItem> = {}): ThinkingFeedItem {
  return {
    id: 1,
    kind: "thinking",
    content: "The user wants the config.\nIt is set twice.",
    ...more,
  };
}

// The step sits in the activity line's list.
function renderStep(item: ThinkingFeedItem): void {
  render(ThoughtStep, { item });
}

describe("reasoning as it streams", () => {
  it("says the agent is thinking, with what it has thought so far", () => {
    renderStep(thought({ streaming: true, content: "Checking the index" }));
    expect(screen.getByText("Thinking")).toBeVisible();
    expect(screen.getByText("Checking the index")).toBeVisible();
    expect(screen.queryByRole("button")).toBeNull();
  });
});

describe("reasoning that is done", () => {
  it("is one line, with how long it took", () => {
    renderStep(thought({ startedAt: 1_000, endedAt: 7_000 }));
    const toggle = screen.getByRole("button", { name: "Thought for 6s" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText(/It is set twice\./)).toBeNull();
  });

  it("says only Thought when it wasn't timed, or took under a second", () => {
    renderStep(thought());
    expect(screen.getByRole("button", { name: "Thought" })).toBeVisible();
  });

  it("says only Thought for a brief one", () => {
    renderStep(thought({ startedAt: 1_000, endedAt: 1_400 }));
    expect(screen.getByRole("button", { name: "Thought" })).toBeVisible();
  });

  it("opens to all of what was thought, and closes again", async () => {
    renderStep(thought({ startedAt: 1_000, endedAt: 7_000 }));
    const toggle = screen.getByRole("button", { name: "Thought for 6s" });
    await userEvent.click(toggle);

    expect(toggle).toHaveAttribute("aria-expanded", "true");
    const text = document.getElementById(toggle.getAttribute("aria-controls") ?? "");
    expect(text).toHaveTextContent("The user wants the config.");
    expect(text).toHaveTextContent("It is set twice.");

    await userEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText(/It is set twice\./)).toBeNull();
  });
});
