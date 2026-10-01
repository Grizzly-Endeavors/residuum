import { describe, expect, it, vi } from "vitest";
import { within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { render, screen } from "../test/component";
import type { ObservedTurn } from "../lib/observed-turns.svelte";
import { router } from "../lib/router.svelte";
import type { StepCall } from "./activity";
import ActivityLine from "./ActivityLine.svelte";

const calls: StepCall[] = [
  {
    id: "c1",
    name: "memory_search",
    arguments: { query: "notification routing" },
    status: "done",
    result: "─── result ───\n3 notes found",
  },
  {
    id: "c2",
    name: "read_file",
    arguments: { path: "team/wiki/channels.md" },
    status: "error",
    result: "─── result ───\nno such file",
  },
  {
    id: "c3",
    name: "read_file",
    arguments: { path: "team/wiki/on-call.md" },
    status: "done",
    result: "─── result ───\n   1\t# On call\n   2\tQuiet hours 22:00–07:00",
  },
];

function watched(more: Partial<ObservedTurn> = {}): ObservedTurn {
  return { startedAt: 0, endedAt: 14_000, ending: "finished", stopAsked: false, gaps: [], ...more };
}

describe("a finished turn's line", () => {
  it("is one summary at first, with its time and failures when the page watched it", () => {
    render(ActivityLine, { agent: "atlas", calls, live: false, observed: watched() });
    const summary = screen.getByRole("button", { name: /^Searched memory, read 2 files/ });
    expect(summary).toHaveAttribute("aria-expanded", "false");
    expect(summary).toHaveTextContent("Searched memory, read 2 files · 14s · 1 step failed");
    expect(screen.queryByRole("list")).toBeNull();
  });

  it("shows no time or failures for a turn from history", () => {
    render(ActivityLine, { agent: "atlas", calls, live: false });
    expect(screen.getByRole("button", { name: /^Searched memory/ })).toHaveTextContent(
      /^Searched memory, read 2 files$/,
    );
  });

  it("opens to its steps, each with what it acted on, and paths link into the panel", async () => {
    const openPlace = vi.spyOn(router, "openPlace").mockResolvedValue(true);
    render(ActivityLine, { agent: "atlas", calls, live: false, observed: watched() });
    await userEvent.click(screen.getByRole("button", { name: /^Searched memory, read 2 files/ }));

    const steps = within(screen.getByRole("list")).getAllByRole("listitem");
    expect(steps.map((step) => step.textContent.replace(/\s+/g, " ").trim())).toEqual([
      "Searched memory for “notification routing”",
      "Read team/wiki/channels.md Failed",
      "Read team/wiki/on-call.md",
    ]);
    expect(
      screen.getByRole("button", { name: "Read team/wiki/channels.md, failed" }),
    ).toBeVisible();

    const link = screen.getByRole("link", { name: "team/wiki/on-call.md" });
    expect(link).toHaveAttribute("href", "/agent/atlas?panel=file:team/wiki/on-call.md");
    await userEvent.click(link);
    expect(openPlace).toHaveBeenCalledWith(
      { kind: "chat", agent: "atlas" },
      { panel: { kind: "file", path: "team/wiki/on-call.md" } },
    );
  });

  it("opens a step to its arguments and its formatted result", async () => {
    render(ActivityLine, { agent: "atlas", calls, live: false });
    await userEvent.click(screen.getByRole("button", { name: /^Searched memory/ }));
    const step = screen.getByRole("button", { name: "Read team/wiki/on-call.md" });
    expect(step).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(step);

    expect(step).toHaveAttribute("aria-expanded", "true");
    const detail = document.getElementById(step.getAttribute("aria-controls") ?? "");
    expect(detail).toHaveTextContent("team/wiki/on-call.md");
    expect(detail).toHaveTextContent("Quiet hours 22:00–07:00");
  });

  it("shows nothing for a turn that made no tool calls and ended on its own", () => {
    const { container } = render(ActivityLine, {
      agent: "atlas",
      calls: [],
      live: false,
      observed: watched(),
    });
    expect(container.textContent).toBe("");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("says the user stopped it", () => {
    render(ActivityLine, {
      agent: "atlas",
      calls: [],
      live: false,
      observed: watched({ ending: "stopped", endedAt: 2_000 }),
    });
    expect(screen.getByRole("button", { name: /^Stopped by you/ })).toHaveTextContent(
      "Stopped by you · 2s",
    );
  });
});

describe("a running turn's line", () => {
  it("is open: working, for how long, Stop, and each step with its status", async () => {
    const onStop = vi.fn();
    const running: StepCall[] = [
      calls[0] as StepCall,
      { id: "c4", name: "exec", arguments: { command: "git status" }, status: "running" },
    ];
    render(ActivityLine, {
      agent: "atlas",
      calls: running,
      live: true,
      observed: watched({ startedAt: Date.now() - 5_000, endedAt: null, ending: null }),
      onStop,
    });
    expect(screen.getByText("Working")).toBeInTheDocument();
    expect(screen.getByText("5s")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Running git status, running" })).toBeVisible();

    await userEvent.click(screen.getByRole("button", { name: "Stop the reply" }));
    expect(onStop).toHaveBeenCalledOnce();
  });

  it("says Stopping while the stop is on its way, and ignores more presses", async () => {
    const onStop = vi.fn();
    render(ActivityLine, {
      agent: "atlas",
      calls: [],
      live: true,
      observed: watched({ endedAt: null, ending: null, stopAsked: true }),
      onStop,
    });
    const stop = screen.getByRole("button", { name: "Stop the reply" });
    expect(stop).toHaveTextContent("Stopping…");
    await userEvent.click(stop);
    expect(onStop).not.toHaveBeenCalled();
  });

  it("hands focus from Stop to the summary when the turn ends", async () => {
    const view = render(ActivityLine, {
      agent: "atlas",
      calls: [calls[0] as StepCall],
      live: true,
      observed: watched({ endedAt: null, ending: null }),
      onStop: vi.fn(),
    });
    screen.getByRole("button", { name: "Stop the reply" }).focus();
    await view.rerender({ live: false, observed: watched({ ending: "stopped" }) });
    expect(screen.getByRole("button", { name: /stopped by you$/ })).toHaveFocus();
  });

  it("notes the steps it may have missed, where it missed them", () => {
    render(ActivityLine, {
      agent: "atlas",
      calls: [calls[0] as StepCall],
      live: true,
      observed: watched({ startedAt: null, endedAt: null, ending: null, gaps: [0] }),
    });
    const items = within(screen.getByRole("list")).getAllByRole("listitem");
    expect(items[0]).toHaveTextContent("Earlier steps happened before this page connected");
    expect(items[1]).toHaveTextContent("Searched memory for");
    // Joined partway, it can't say how long the turn has run.
    expect(screen.queryByText(/^\d+s$/)).toBeNull();
    // Without a way to stop it here, it offers none.
    expect(screen.queryByRole("button", { name: "Stop the reply" })).toBeNull();
  });
});
