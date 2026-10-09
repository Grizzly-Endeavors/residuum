import { describe, expect, it } from "vitest";
import type { ObservedTurn } from "../lib/observed-turns.svelte";
import {
  activitySteps,
  callSteps,
  gapNote,
  stepText,
  stepsPhrase,
  segmentDuration,
  summarizeSegment,
  thoughtLabel,
  type SegmentStep,
  turnEndingLine,
  type StepCall,
} from "./activity";

let ids = 0;
function call(
  name: string,
  args: Record<string, unknown> = {},
  more: Partial<StepCall> = {},
): StepCall {
  ids++;
  return { id: `c${String(ids)}`, name, arguments: args, status: "done", ...more };
}

function watched(more: Partial<ObservedTurn> = {}): ObservedTurn {
  return {
    startedAt: 1_000,
    endedAt: 15_000,
    ending: "finished",
    stopAsked: false,
    retrying: false,
    gaps: [],
    ...more,
  };
}

describe("step labels", () => {
  it("say what a built-in tool did, and to what", () => {
    const steps = activitySteps([
      call("memory_search", { query: "notification routing" }),
      call("read_file", { path: "team/wiki/channels.md" }),
      call("exec", { command: "git status" }),
      call("web_fetch", { url: "https://svelte.dev/blog" }),
    ]);
    expect(steps.map(stepText)).toEqual([
      "Searched memory for “notification routing”",
      "Read team/wiki/channels.md",
      "Ran git status",
      "Read https://svelte.dev/blog",
    ]);
    expect(steps.map((s) => s.target?.kind)).toEqual(["query", "path", "code", "code"]);
  });

  it("read as ongoing while the call runs", () => {
    const [step] = activitySteps([
      call("read_file", { path: "team/wiki/index.md" }, { status: "running" }),
    ]);
    expect(step?.status).toBe("running");
    expect(step && stepText(step)).toBe("Reading team/wiki/index.md");
  });

  it("read whole when the call names no target", () => {
    expect(activitySteps([call("read_file"), call("memory_search")]).map(stepText)).toEqual([
      "Read a file",
      "Searched memory",
    ]);
  });

  it("link a workspace path, and leave a path outside the workspace plain", () => {
    const [inside, outside] = activitySteps([
      call("write_file", { path: "notes/plan.md" }),
      call("write_file", { path: "/etc/hosts" }),
    ]);
    expect(inside?.target).toEqual({ kind: "path", text: "notes/plan.md" });
    expect(outside?.target).toEqual({ kind: "code", text: "/etc/hosts" });
  });

  it("name the session a spawn started once its result says which", () => {
    const pending = call("subagent_spawn", { task: "Compare fallbacks" }, { status: "running" });
    const started = call(
      "subagent_spawn",
      { task: "Compare fallbacks" },
      { result: "─── result ───\nSession spawned-research-3f9a spawned." },
    );
    const [before, after] = activitySteps([pending, started]);
    expect(before && stepText(before)).toBe("Starting a session");
    expect(after?.target).toEqual({ kind: "session", text: "spawned-research-3f9a" });
  });

  it("cut a long target, which the details hold whole", () => {
    const [step] = activitySteps([call("exec", { command: `echo ${"x".repeat(300)}` })]);
    expect(step?.target?.text).toHaveLength(120);
    expect(step?.target?.text.endsWith("…")).toBe(true);
  });

  it("fall back to the tool's name, and a tool server's tool to its server and name", () => {
    const steps = activitySteps([
      call("lookup_weather", { city: "Oslo" }),
      call("create_issue", {}, { server: "github" }),
      call("constructor"),
    ]);
    expect(steps.map(stepText)).toEqual([
      "Used lookup_weather",
      "Used github: create_issue",
      "Used constructor",
    ]);
    expect(
      activitySteps([call("create_issue", {}, { server: "github", status: "running" })]).map(
        stepText,
      ),
    ).toEqual(["Using github: create_issue"]);
  });

  it("carry each call's status: failed and stopped as well as running and done", () => {
    const steps = activitySteps([
      call("read_file", { path: "a/b.md" }, { status: "error" }),
      call("exec", { command: "sleep 60" }, { status: "stopped" }),
    ]);
    expect(steps.map((s) => s.status)).toEqual(["failed", "stopped"]);
  });
});

describe("the summary", () => {
  it("merges repeats and counts them, in the order they first ran", () => {
    expect(
      stepsPhrase([
        call("memory_search", { query: "a" }),
        call("read_file", { path: "a/b.md" }),
        call("read_file", { path: "a/c.md" }),
        call("subagent_spawn", { task: "t" }),
        call("memory_search", { query: "b" }),
      ]),
    ).toBe("Searched memory 2 times, read 2 files, started 1 session");
    expect(stepsPhrase([call("write_file", { path: "a/b.md" })])).toBe("Wrote 1 file");
  });

  it("merges a tool server's repeats by server and tool", () => {
    expect(
      stepsPhrase([
        call("create_issue", {}, { server: "github" }),
        call("create_issue", {}, { server: "github" }),
        call("create_issue", {}, { server: "linear" }),
      ]),
    ).toBe("Used github: create_issue 2 times, used linear: create_issue");
  });

  it("folds the rest into a count once there are many kinds of step", () => {
    expect(
      stepsPhrase([
        call("memory_search"),
        call("read_file"),
        call("exec"),
        call("web_fetch"),
        call("web_fetch"),
        call("inbox_list"),
      ]),
    ).toBe("Searched memory, read 1 file, ran 1 command and 3 more steps");
  });

  it("adds how long a watched segment took, and flags a failed step", () => {
    const calls = [
      call("memory_search", { query: "x" }, { startedAt: 1_000, endedAt: 4_000 }),
      call("read_file", { path: "a/b.md" }, { status: "error", startedAt: 3_000, endedAt: 15_000 }),
    ];
    expect(summarizeSegment(callSteps(calls), false)).toEqual({
      text: "Searched memory, read 1 file",
      duration: "14s",
      failures: "1 step failed",
    });
  });

  it("shows no timing for steps from history", () => {
    expect(summarizeSegment(callSteps([call("read_file", { path: "a/b.md" })]), false)).toEqual({
      text: "Read 1 file",
      duration: null,
      failures: null,
    });
  });

  it("leaves out a time under a second", () => {
    const quick = [call("exec", {}, { startedAt: 5_000, endedAt: 5_400 })];
    expect(summarizeSegment(callSteps(quick), false)?.duration).toBeNull();
    expect(
      segmentDuration(callSteps([call("exec", {}, { startedAt: 5_000, endedAt: 6_000 })])),
    ).toBe("1s");
  });

  it("leaves out a time it can't tell because a step wasn't watched", () => {
    const calls = [
      call("exec", {}, { startedAt: 1_000, endedAt: 9_000 }),
      call("exec", {}, { startedAt: 2_000 }),
    ];
    expect(segmentDuration(callSteps(calls))).toBeNull();
  });

  it("says the page missed a segment that holds no steps", () => {
    expect(summarizeSegment([], true)?.text).toBe("Worked before this page connected");
    expect(summarizeSegment([], false)).toBeNull();
  });
});

describe("reasoning in a segment's summary", () => {
  const thought = (startedAt?: number, endedAt?: number): SegmentStep => ({
    kind: "thought",
    item: {
      id: 1,
      kind: "thinking",
      content: "hm",
      ...(startedAt === undefined ? {} : { startedAt }),
      ...(endedAt === undefined ? {} : { endedAt }),
    },
  });

  it("follows the tool calls, with how long it thought", () => {
    const steps = [
      ...callSteps([
        call("read_file", { path: "a.md" }, { startedAt: 1_000, endedAt: 3_000 }),
        call("read_file", { path: "b.md" }, { startedAt: 1_000, endedAt: 4_000 }),
      ]),
      thought(4_000, 10_000),
    ];
    expect(summarizeSegment(steps, false)).toEqual({
      text: "Read 2 files, thought 6s",
      duration: "9s",
      failures: null,
    });
  });

  it("adds up several thoughts", () => {
    const steps = [
      thought(0, 2_000),
      ...callSteps([call("exec", {}, { startedAt: 2_000, endedAt: 3_000 })]),
      thought(3_000, 6_000),
    ];
    expect(summarizeSegment(steps, false)?.text).toBe("Ran 1 command, thought 5s");
  });

  it("says thought without a time when history holds none, or it was brief", () => {
    expect(summarizeSegment([...callSteps([call("exec")]), thought()], false)?.text).toBe(
      "Ran 1 command, thought",
    );
    expect(summarizeSegment([...callSteps([call("exec")]), thought(0, 300)], false)?.text).toBe(
      "Ran 1 command, thought",
    );
  });

  it("stands alone as Thought for a segment with no tool calls", () => {
    expect(summarizeSegment([thought(0, 6_000)], false)).toMatchObject({
      text: "Thought for 6s",
      duration: null,
    });
    expect(summarizeSegment([thought()], false)?.text).toBe("Thought");
  });

  it("labels a thought as it streams, and once it is done", () => {
    const item = { id: 1, kind: "thinking", content: "hm" } as const;
    expect(thoughtLabel({ ...item, streaming: true })).toBe("Thinking");
    expect(thoughtLabel({ ...item, startedAt: 0, endedAt: 6_000 })).toBe("Thought for 6s");
    expect(thoughtLabel(item)).toBe("Thought");
  });
});

describe("the line that closes a turn", () => {
  it("says the user stopped it, and for how long", () => {
    expect(turnEndingLine(watched({ ending: "stopped" }), true)).toBe("Stopped by you · 14s");
    expect(turnEndingLine(watched({ ending: "stopped" }), false)).toBe("Stopped by you · 14s");
  });

  it("says the agent stopped under it", () => {
    expect(turnEndingLine(watched({ ending: "interrupted" }), false)).toBe("Didn't finish · 14s");
  });

  it("leaves out a time it can't tell, or one under a second", () => {
    expect(turnEndingLine(watched({ ending: "stopped", startedAt: null }), true)).toBe(
      "Stopped by you",
    );
    expect(turnEndingLine(watched({ ending: "stopped", endedAt: 1_400 }), true)).toBe(
      "Stopped by you",
    );
  });

  it("says how long a turn that did work took, when it ended on its own", () => {
    expect(turnEndingLine(watched(), true)).toBe("Worked for 14s");
  });

  it("has nothing to say about a turn that did no work, or took under a second", () => {
    expect(turnEndingLine(watched(), false)).toBeNull();
    expect(turnEndingLine(watched({ endedAt: 1_900 }), true)).toBeNull();
  });

  it("has nothing to say about a turn the page didn't watch, or one still running", () => {
    expect(turnEndingLine(undefined, true)).toBeNull();
    expect(turnEndingLine(watched({ endedAt: null, ending: null }), true)).toBeNull();
  });
});

describe("gap notes", () => {
  it("say steps came before the page connected, or while it reconnected", () => {
    expect(gapNote(0)).toBe("Earlier steps happened before this page connected");
    expect(gapNote(3)).toBe("Steps taken while this page was reconnecting may be missing");
  });
});
