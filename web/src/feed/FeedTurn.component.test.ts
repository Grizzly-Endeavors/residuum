import { describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { render, screen } from "../test/component";
import type { ObservedTurn } from "../lib/observed-turns.svelte";
import type { AssistantFeedItem, ToolCallState } from "../lib/types";
import { callSteps } from "./activity";
import FeedTurn from "./FeedTurn.svelte";
import type { FeedTurn as Turn, TurnPart } from "./turns";

class NoObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
vi.stubGlobal("ResizeObserver", NoObserver);

let ids = 0;
function say(content: string): TurnPart {
  const item: AssistantFeedItem = { id: ++ids, kind: "assistant", content };
  return { kind: "message", key: `item-${String(item.id)}`, item };
}

function steps(
  callsBefore: number,
  ...more: Array<Partial<ToolCallState> & { name: string }>
): TurnPart {
  const calls = more.map((call) => ({
    id: `c${String(++ids)}`,
    arguments: {},
    status: "done" as const,
    ...call,
  }));
  return {
    kind: "activity",
    key: `activity-${String(++ids)}`,
    callsBefore,
    steps: callSteps(calls),
    calls,
  };
}

function turn(parts: TurnPart[], live: boolean): Turn {
  return { kind: "turn", key: "turn:t1", turnId: "t1", parts, live };
}

function watched(more: Partial<ObservedTurn> = {}): ObservedTurn {
  return {
    startedAt: 0,
    endedAt: 14_000,
    ending: "finished",
    stopAsked: false,
    retrying: false,
    gaps: [],
    ...more,
  };
}

const running = (more: Partial<ObservedTurn> = {}): ObservedTurn =>
  watched({ startedAt: Date.now() - 5_000, endedAt: null, ending: null, ...more });

describe("a running turn", () => {
  it("lays out what happened in order: collapsed runs between texts, the newest run open, the head last", () => {
    render(FeedTurn, {
      agent: "atlas",
      turn: turn(
        [
          steps(0, { name: "read_file", arguments: { path: "a.md" } }),
          say("Let me check the config first."),
          steps(1, { name: "exec", arguments: { command: "git status" }, status: "running" }),
        ],
        true,
      ),
      observed: running(),
      onStop: vi.fn(),
    });

    const summary = screen.getByRole("button", { name: /^Read 1 file/ });
    const text = screen.getByText("Let me check the config first.");
    const step = screen.getByRole("button", { name: "Running git status, running" });
    const head = screen.getByText("Working");
    const order = [summary, text, step, head];
    for (const [index, element] of order.entries()) {
      const next = order[index + 1];
      if (next === undefined) continue;
      expect(element.compareDocumentPosition(next) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    }
    // The earlier run is a summary; only the newest shows its steps.
    expect(summary).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("button", { name: /^Read a\.md/ })).toBeNull();
  });

  it("puts the head below the newest text when the agent has just spoken", () => {
    render(FeedTurn, {
      agent: "atlas",
      turn: turn([steps(0, { name: "exec" }), say("The port is set twice.")], true),
      observed: running(),
      onStop: vi.fn(),
    });
    expect(screen.getByRole("button", { name: /^Ran 1 command/ })).toBeVisible();
    const text = screen.getByText("The port is set twice.");
    expect(
      text.compareDocumentPosition(screen.getByText("Working")) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("shows the head alone before the turn has made anything", () => {
    render(FeedTurn, { agent: "atlas", turn: turn([], true), observed: running() });
    expect(screen.getByText("Working")).toBeInTheDocument();
    expect(screen.getByText("5s")).toBeInTheDocument();
    // Without a way to stop it here, it offers none.
    expect(screen.queryByRole("button", { name: "Stop the reply" })).toBeNull();
  });

  it("says it is retrying, quietly, while a stream starts over", () => {
    const view = render(FeedTurn, {
      agent: "atlas",
      turn: turn([say("Let me look.")], true),
      observed: running({ retrying: true }),
    });
    expect(screen.getByText("Retrying…")).toBeVisible();
    // Beside the working mark, not in place of it.
    expect(screen.getByText("Working")).toBeVisible();
    view.unmount();

    render(FeedTurn, {
      agent: "atlas",
      turn: turn([say("Let me look.")], true),
      observed: running(),
    });
    expect(screen.queryByText("Retrying…")).toBeNull();
  });

  it("stops the turn from the head", async () => {
    const onStop = vi.fn();
    render(FeedTurn, { agent: "atlas", turn: turn([], true), observed: running(), onStop });
    await userEvent.click(screen.getByRole("button", { name: "Stop the reply" }));
    expect(onStop).toHaveBeenCalledOnce();
  });

  it("says Stopping while the stop is on its way, and ignores more presses", async () => {
    const onStop = vi.fn();
    render(FeedTurn, {
      agent: "atlas",
      turn: turn([], true),
      observed: running({ stopAsked: true }),
      onStop,
    });
    const stop = screen.getByRole("button", { name: "Stop the reply" });
    expect(stop).toHaveTextContent("Stopping…");
    await userEvent.click(stop);
    expect(onStop).not.toHaveBeenCalled();
  });

  it("can't say how long a turn it joined partway has run, and notes the steps it missed", () => {
    render(FeedTurn, {
      agent: "atlas",
      turn: turn([steps(0, { name: "exec" })], true),
      observed: running({ startedAt: null, gaps: [0] }),
    });
    expect(screen.queryByText(/^\d+s$/)).toBeNull();
    expect(screen.getByText("Earlier steps happened before this page connected")).toBeVisible();
  });

  it("holds the note for a turn it joined at a text in a line of its own at the head", async () => {
    render(FeedTurn, {
      agent: "atlas",
      turn: turn([say("Looking first."), steps(0, { name: "exec" })], true),
      observed: running({ startedAt: null, gaps: [0] }),
    });
    const lead = screen.getByRole("button", { name: "Worked before this page connected" });
    const text = screen.getByText("Looking first.");
    expect(lead.compareDocumentPosition(text) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    await userEvent.click(lead);
    expect(screen.getByText("Earlier steps happened before this page connected")).toBeVisible();
  });
});

describe("a turn that ended", () => {
  it("hands focus from Stop to the newest summary", async () => {
    const parts = [steps(0, { name: "exec" })];
    const view = render(FeedTurn, {
      agent: "atlas",
      turn: turn(parts, true),
      observed: running(),
      onStop: vi.fn(),
    });
    screen.getByRole("button", { name: "Stop the reply" }).focus();
    await view.rerender({ turn: turn(parts, false), observed: watched({ ending: "stopped" }) });
    expect(screen.getByRole("button", { name: /^Ran 1 command/ })).toHaveFocus();
  });

  it("says the user stopped it, and for how long", () => {
    render(FeedTurn, {
      agent: "atlas",
      turn: turn([], false),
      observed: watched({ ending: "stopped", endedAt: 2_000 }),
    });
    expect(screen.getByText("Stopped by you · 2s")).toBeVisible();
    expect(screen.queryByText("Working")).toBeNull();
  });

  it("says how long a turn that did work took", () => {
    render(FeedTurn, {
      agent: "atlas",
      turn: turn([steps(0, { name: "exec" }), say("Done.")], false),
      observed: watched(),
    });
    expect(screen.getByText("Worked for 14s")).toBeVisible();
  });

  it("says nothing for a reply with no work, or a turn from history", () => {
    const { container, unmount } = render(FeedTurn, {
      agent: "atlas",
      turn: turn([say("Hi.")], false),
      observed: watched(),
    });
    expect(container).not.toHaveTextContent("Worked");
    unmount();

    const history = render(FeedTurn, {
      agent: "atlas",
      turn: turn([steps(0, { name: "exec" }), say("Done.")], false),
    });
    expect(history.container).not.toHaveTextContent("Worked");
    expect(screen.getByRole("button", { name: /^Ran 1 command/ })).toBeVisible();
  });

  it("ends a failed turn on its failure, not on how long it worked", () => {
    const failure: TurnPart = {
      kind: "message",
      key: "item-failure",
      item: { id: 900, kind: "turn-failure", turnId: "t1", message: "The provider didn't answer." },
    };
    render(FeedTurn, {
      agent: "atlas",
      turn: turn([steps(0, { name: "exec" }), failure], false),
      observed: watched(),
    });
    expect(screen.getByText("atlas couldn't finish this reply")).toBeVisible();
    expect(screen.queryByText(/^Worked for/)).toBeNull();
  });

  it("notes steps the page missed in the run they belong to", async () => {
    render(FeedTurn, {
      agent: "atlas",
      turn: turn(
        [
          steps(0, { name: "exec" }, { name: "exec" }),
          say("Halfway."),
          steps(2, { name: "read_file", arguments: { path: "a.md" } }),
        ],
        false,
      ),
      observed: watched({ gaps: [0, 2] }),
    });
    const [first, second] = screen.getAllByRole("button", { name: /^(Ran|Read) / });
    await userEvent.click(first as HTMLElement);
    expect(screen.getByText("Earlier steps happened before this page connected")).toBeVisible();
    expect(
      screen.getByText("Steps taken while this page was reconnecting may be missing"),
    ).toBeVisible();
    await userEvent.click(second as HTMLElement);
  });
});
