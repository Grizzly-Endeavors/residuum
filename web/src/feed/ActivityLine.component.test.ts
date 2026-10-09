import { describe, expect, it, vi } from "vitest";
import { within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { render, screen } from "../test/component";
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
    startedAt: 1_000,
    endedAt: 3_000,
  },
  {
    id: "c2",
    name: "read_file",
    arguments: { path: "team/wiki/channels.md" },
    status: "error",
    result: "─── result ───\nno such file",
    startedAt: 3_000,
    endedAt: 4_000,
  },
  {
    id: "c3",
    name: "read_file",
    arguments: { path: "team/wiki/on-call.md" },
    status: "done",
    result: "─── result ───\n   1\t# On call\n   2\tQuiet hours 22:00–07:00",
    startedAt: 4_000,
    endedAt: 15_000,
  },
];

/** The same steps as history holds them, which records no timing. */
function fromHistory(): StepCall[] {
  return calls.map(({ startedAt: _started, endedAt: _ended, ...rest }) => ({
    ...rest,
    status: "done",
  }));
}

describe("a finished run of steps", () => {
  it("is one summary at first, with its time and failures when the page watched it", () => {
    render(ActivityLine, { agent: "atlas", calls, live: false });
    const summary = screen.getByRole("button", { name: /^Searched memory, read 2 files/ });
    expect(summary).toHaveAttribute("aria-expanded", "false");
    expect(summary).toHaveTextContent("Searched memory, read 2 files · 14s · 1 step failed");
    expect(screen.queryByRole("list")).toBeNull();
  });

  it("shows no time or failures for steps from history", () => {
    render(ActivityLine, { agent: "atlas", calls: fromHistory(), live: false });
    expect(screen.getByRole("button", { name: /^Searched memory/ })).toHaveTextContent(
      /^Searched memory, read 2 files$/,
    );
  });

  it("leaves out a time under a second", () => {
    const quick: StepCall[] = [
      { id: "q", name: "exec", arguments: {}, status: "done", startedAt: 1_000, endedAt: 1_300 },
    ];
    render(ActivityLine, { agent: "atlas", calls: quick, live: false });
    expect(screen.getByRole("button", { name: /^Ran 1 command/ })).toHaveTextContent(
      /^Ran 1 command$/,
    );
  });

  it("opens to its steps, each with what it acted on, and paths link into the panel", async () => {
    const openPlace = vi.spyOn(router, "openPlace").mockResolvedValue(true);
    render(ActivityLine, { agent: "atlas", calls, live: false });
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

  it("shows nothing when it holds no steps and nothing was missed", () => {
    const { container } = render(ActivityLine, { agent: "atlas", calls: [], live: false });
    expect(container.textContent).toBe("");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("says the page missed it when it holds no steps but a gap", async () => {
    render(ActivityLine, { agent: "atlas", calls: [], live: false, gaps: [0] });
    const summary = screen.getByRole("button", { name: "Worked before this page connected" });
    await userEvent.click(summary);
    expect(screen.getByRole("listitem")).toHaveTextContent(
      "Earlier steps happened before this page connected",
    );
  });
});

describe("the newest run of steps in a running turn", () => {
  const running: StepCall[] = [
    calls[0] as StepCall,
    { id: "c4", name: "exec", arguments: { command: "git status" }, status: "running" },
  ];

  it("is open: each step as it arrives, with its status, and no summary", () => {
    render(ActivityLine, { agent: "atlas", calls: running, live: true });
    expect(screen.getByRole("button", { name: "Running git status, running" })).toBeVisible();
    expect(screen.getByRole("button", { name: /^Searched memory for/ })).toBeVisible();
    expect(screen.queryByRole("button", { name: /^Searched memory, / })).toBeNull();
  });

  it("collapses to its summary once it gives way, keeping focus from a step on the line", async () => {
    const view = render(ActivityLine, { agent: "atlas", calls: running, live: true });
    screen.getByRole("button", { name: "Running git status, running" }).focus();
    await view.rerender({
      live: false,
      calls: [calls[0] as StepCall, { ...running[1], status: "done" } as StepCall],
    });
    expect(screen.getByRole("button", { name: /^Searched memory, ran 1 command/ })).toHaveFocus();
  });

  it("notes the steps it may have missed, where it missed them", () => {
    render(ActivityLine, {
      agent: "atlas",
      calls: [calls[0] as StepCall],
      live: true,
      gaps: [0, 1],
    });
    const items = within(screen.getByRole("list")).getAllByRole("listitem");
    expect(items[0]).toHaveTextContent("Earlier steps happened before this page connected");
    expect(items[1]).toHaveTextContent("Searched memory for");
    expect(items[2]).toHaveTextContent(
      "Steps taken while this page was reconnecting may be missing",
    );
  });
});
