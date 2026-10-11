import { describe, expect, it } from "vitest";
import type { AgentOverview, AgentSummary, UpcomingRun } from "../../lib/hub-types";
import {
  comingUp,
  elapsed,
  eventLocation,
  lastLine,
  lastMessageWhen,
  nowLine,
  pastWhen,
  runTitle,
  runWhen,
  splitSummary,
  teamTally,
  upcomingWhen,
  type RowInput,
} from "./home-model";

// Times are built in the machine's own zone, and expected clock times are
// formatted the way Home formats them, so the tests hold in any zone and locale.

/** Noon on a fixed day, local time. */
const NOW = new Date(2026, 2, 14, 12, 0).getTime();
const at = (day: number, hour: number, minute = 0): string =>
  new Date(2026, 2, day, hour, minute).toISOString();
const clock = (iso: string): string =>
  new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" }).format(
    Date.parse(iso),
  );
const date = (iso: string): string =>
  new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" }).format(Date.parse(iso));

function agent(name: string, overrides: Partial<AgentSummary> = {}): AgentSummary {
  return {
    name,
    display_name: name,
    state: "running",
    last_error: null,
    autostart: true,
    role: null,
    a2a_visibility: "private",
    teams_configured: false,
    ...overrides,
  };
}

function overview(name: string, overrides: Partial<AgentOverview> = {}): AgentOverview {
  return {
    name,
    last_message: null,
    live_sessions: [],
    upcoming: [],
    inbox_unread: 0,
    outbound_problems: [],
    ...overrides,
  };
}

function row(overrides: Partial<RowInput> = {}): RowInput {
  return {
    agent: agent("atlas"),
    activity: { busy: false, busy_since: null, unread: 0 },
    stopping: false,
    overview: overview("atlas"),
    now: NOW,
    ...overrides,
  };
}

const pulse = (when: string, name = "inbox_check"): UpcomingRun => ({
  kind: "pulse",
  name,
  at: when,
});

describe("times", () => {
  it("writes a past time as the clock today and the date before", () => {
    expect(pastWhen(at(14, 9, 5), NOW)).toBe(clock(at(14, 9, 5)));
    expect(pastWhen(at(13, 23), NOW)).toBe(date(at(13, 23)));
    expect(pastWhen("not a time", NOW)).toBe("");
  });

  it("writes a run's time by how soon it comes", () => {
    expect(upcomingWhen(at(14, 11), NOW)).toBe("Due now");
    expect(upcomingWhen(at(14, 12), NOW)).toBe("Due now");
    expect(upcomingWhen(at(14, 12, 34), NOW)).toBe("in 34m");
    expect(upcomingWhen(at(14, 18), NOW)).toBe(`Today at ${clock(at(14, 18))}`);
    expect(upcomingWhen(at(15, 8), NOW)).toBe(`Tomorrow at ${clock(at(15, 8))}`);
    const weekday = new Intl.DateTimeFormat(undefined, { weekday: "short" }).format(
      Date.parse(at(18, 9)),
    );
    expect(upcomingWhen(at(18, 9), NOW)).toBe(`${weekday} at ${clock(at(18, 9))}`);
    expect(upcomingWhen(at(30, 9), NOW)).toBe(`${date(at(30, 9))} at ${clock(at(30, 9))}`);
  });

  it("writes a message known only to the day as that day", () => {
    expect(
      lastMessageWhen(
        { role: "user", preview: "", at: "2026-03-14T00:00:00Z", at_precision: "day" },
        NOW,
      ),
    ).toBe("Today");
    expect(
      lastMessageWhen(
        { role: "user", preview: "", at: "2026-03-10T00:00:00Z", at_precision: "day" },
        NOW,
      ),
    ).toBe(date(at(10, 12)));
  });

  it("writes how long something has run", () => {
    expect(elapsed(new Date(NOW - 14_000).toISOString(), NOW)).toBe("14s");
    expect(elapsed(new Date(NOW - 3 * 60_000).toISOString(), NOW)).toBe("3m");
    expect(elapsed(new Date(NOW - 125 * 60_000).toISOString(), NOW)).toBe("2h 5m");
    expect(elapsed(new Date(NOW - 120 * 60_000).toISOString(), NOW)).toBe("2h");
  });
});

describe("the header tally", () => {
  it("counts starting agents as running", () => {
    expect(
      teamTally([
        agent("a"),
        agent("b", { state: "starting" }),
        agent("c", { state: "stopped" }),
        agent("d", { state: "failed" }),
      ]),
    ).toEqual({ running: 2, stopped: 1, failed: 1 });
  });
});

describe("a board row", () => {
  it("says a busy agent is working on a reply, and for how long", () => {
    const line = nowLine(
      row({
        activity: { busy: true, busy_since: new Date(NOW - 42_000).toISOString(), unread: 0 },
      }),
    );
    expect(line).toEqual({ text: "Working on a reply · 42s", tone: "accent", echo: false });
  });

  it("shows the newest live session's purpose, else Idle", () => {
    const session = (purpose: string, started: string): AgentOverview["live_sessions"][number] => ({
      address: purpose,
      run_id: purpose,
      category: "spawned",
      source_label: "agent:x",
      purpose,
      state: "running",
      started_at: started,
    });
    const busy = overview("atlas", {
      live_sessions: [session("Older", at(14, 10)), session("Newest", at(14, 11))],
    });
    expect(nowLine(row({ overview: busy })).text).toBe("Newest");
    expect(nowLine(row())).toEqual({ text: "Idle", tone: "quiet", echo: false });
  });

  it("speaks to an agent that isn't running", () => {
    const brittle = agent("brittle", {
      state: "failed",
      last_error: { message: "", kind: "config", reason: "", at: at(14, 11) },
    });
    expect(nowLine(row({ agent: brittle }))).toMatchObject({
      text: "Its settings need fixing",
      tone: "danger",
    });
    expect(nowLine(row({ agent: agent("drifter", { state: "stopped" }) }))).toMatchObject({
      echo: true,
    });
    expect(nowLine(row({ stopping: true })).text).toBe("Stopping…");
  });

  it("marks the user's own last message", () => {
    const said = overview("atlas", {
      last_message: { role: "user", preview: "Hi", at: at(14, 9), at_precision: "minute" },
    });
    expect(lastLine(said, NOW)).toEqual({ when: clock(at(14, 9)), text: "You: Hi" });
    expect(lastLine(overview("atlas"), NOW)).toBeNull();
  });

  it("names a run in words and says when it runs, or why it won't", () => {
    expect(runTitle("inbox_check")).toBe("Inbox check");
    expect(runTitle("review-open-prs")).toBe("Review open prs");
    expect(runWhen(pulse(at(14, 12, 30)), "running", NOW)).toBe("in 30m");
    expect(runWhen(pulse(at(14, 11)), "running", NOW)).toBe("Due now");
    expect(runWhen(pulse(at(14, 11)), "stopped", NOW)).toBe("Runs when it starts");
    expect(runWhen(pulse(at(14, 13)), "stopped", NOW)).toBe("Won't run while stopped");
    expect(runWhen(pulse(at(14, 13)), "failed", NOW)).toBe("Won't run until it starts");
  });
});

describe("coming up", () => {
  it("lists the soonest runs of the agents that will run them, at most eight", () => {
    const overviews = {
      atlas: overview("atlas", { upcoming: [pulse(at(14, 15), "a1"), pulse(at(14, 13), "a2")] }),
      drifter: overview("drifter", { upcoming: [pulse(at(14, 12, 30), "d1")] }),
      scout: overview("scout", { upcoming: [pulse(at(14, 13), "s1")] }),
    };
    const runs = comingUp(
      [agent("atlas"), agent("drifter", { state: "stopped" }), agent("scout")],
      overviews,
    );
    expect(runs.map((r) => `${r.agent}:${r.run.name}`)).toEqual([
      "atlas:a2",
      "scout:s1",
      "atlas:a1",
    ]);

    const many = overview("atlas", {
      upcoming: Array.from({ length: 12 }, (_, n) => pulse(at(14, 13, n), `p${String(n)}`)),
    });
    expect(comingUp([agent("atlas")], { atlas: many })).toHaveLength(8);
  });
});

describe("team events", () => {
  it("leads each target to its place", () => {
    expect(eventLocation({ kind: "agent_place", agent: "atlas", place: "activity" })).toEqual({
      place: { kind: "activity", agent: "atlas" },
      panel: null,
      settings: null,
    });
    expect(eventLocation({ kind: "session", agent: "atlas", run_id: "r1" })).toEqual({
      place: { kind: "chat", agent: "atlas" },
      panel: { kind: "session", agent: "atlas", runId: "r1" },
      settings: null,
    });
    expect(eventLocation({ kind: "inbox_item", agent: "atlas", item_id: "i1" }).place).toEqual({
      kind: "inbox",
      agent: null,
      tab: "active",
      item: { agent: "atlas", id: "i1" },
    });
  });

  it("splits a summary around the agent that leads it", () => {
    expect(splitSummary("atlas started", "atlas")).toEqual({ agent: "atlas", rest: " started" });
    expect(splitSummary("Residuum started", null)).toEqual({
      agent: null,
      rest: "Residuum started",
    });
  });
});
