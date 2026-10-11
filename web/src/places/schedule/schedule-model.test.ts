import { describe, expect, it } from "vitest";
import type { ActionInfo, PulseInfo } from "../../lib/types";
import {
  actionWhen,
  lastRun,
  notRunningTitle,
  overlapNote,
  pulseCadence,
  pulseIcon,
  pulseNext,
  pulsesHeld,
} from "./schedule-model";

const NOW = Date.parse("2026-03-14T12:00:00Z");
const MINUTE = 60_000;
const iso = (offsetMs: number): string => new Date(NOW + offsetMs).toISOString();

function pulse(overrides: Partial<PulseInfo> = {}): PulseInfo {
  return {
    name: "inbox_check",
    enabled: true,
    schedule: "2h",
    active_hours: null,
    agent: null,
    next_fire_at: iso(34 * MINUTE),
    last_outcome: null,
    current_run: null,
    problems: [],
    ...overrides,
  };
}

describe("pulseCadence", () => {
  it.each([
    ["2h", "Every 2 hours"],
    ["30m", "Every 30 minutes"],
    ["1d", "Every day"],
    ["45s", "Every 45 seconds"],
    ["fortnightly", "Every fortnightly"],
  ])("reads %s as %s", (schedule, words) => {
    expect(pulseCadence(pulse({ schedule }))).toBe(words);
  });

  it("adds the active hours", () => {
    expect(pulseCadence(pulse({ schedule: "24h", active_hours: "02:00-06:00" }))).toBe(
      "Every 24 hours, between 02:00 and 06:00",
    );
    expect(pulseCadence(pulse({ active_hours: "nights" }))).toBe("Every 2 hours, nights");
  });

  it("is null for a pulse with no schedule", () => {
    expect(pulseCadence(pulse({ schedule: null }))).toBeNull();
  });
});

describe("pulseNext", () => {
  it("says when a running agent's pulse runs, as Home does", () => {
    expect(pulseNext(pulse(), "running", NOW)).toBe("in 34m");
    expect(pulseNext(pulse({ next_fire_at: iso(-MINUTE) }), "running", NOW)).toBe("Due now");
  });

  it("says a stopped agent's pulse waits for it", () => {
    expect(pulseNext(pulse(), "stopped", NOW)).toBe("Won't run while stopped");
    expect(pulseNext(pulse({ next_fire_at: iso(-MINUTE) }), "stopped", NOW)).toBe(
      "Runs when it starts",
    );
  });

  it("says why a pulse won't run", () => {
    expect(pulseNext(pulse({ schedule: null, enabled: false }), "running", NOW)).toBe(
      "Won't run until fixed",
    );
    expect(pulseNext(pulse({ enabled: false, next_fire_at: null }), "running", NOW)).toBe("Paused");
    expect(pulseNext(pulse({ next_fire_at: null }), "running", NOW)).toBe("No next run");
  });
});

describe("pulsesHeld", () => {
  it("is true when no pulse that should run has a next run", () => {
    expect(pulsesHeld([pulse({ next_fire_at: null }), pulse({ enabled: false })])).toBe(true);
  });

  it("is false while any pulse that should run has one, or none should run", () => {
    expect(pulsesHeld([pulse({ next_fire_at: null }), pulse()])).toBe(false);
    expect(pulsesHeld([pulse({ enabled: false, next_fire_at: null })])).toBe(false);
    expect(pulsesHeld([])).toBe(false);
  });
});

describe("lastRun", () => {
  it("says how the last run went and when", () => {
    expect(lastRun({ status: "completed", at: iso(-60 * MINUTE), error: null }, NOW)).toBe(
      "Last ran 1h ago",
    );
    expect(lastRun({ status: "cancelled", at: iso(-5 * MINUTE), error: null }, NOW)).toBe(
      "Last run was stopped 5m ago",
    );
    expect(
      lastRun({ status: "failed", at: iso(-5 * MINUTE), error: "the model timed out" }, NOW),
    ).toBe("Last run failed 5m ago: the model timed out");
    expect(lastRun({ status: "failed", at: iso(-5 * MINUTE), error: null }, NOW)).toBe(
      "Last run failed 5m ago",
    );
  });
});

describe("overlapNote", () => {
  it("names when the run it overlapped started, and is null for a run that didn't", () => {
    const run = { address: "scheduled-1", run_id: "run-2", overlap: null };
    expect(overlapNote(run, NOW)).toBeNull();
    expect(
      overlapNote(
        { ...run, overlap: { previous_run_id: "run-1", previous_started_at: iso(-12 * MINUTE) } },
        NOW,
      ),
    ).toBe("Started while the run from 12m ago was still going");
  });
});

describe("the rest of the place's words", () => {
  it("marks a pulse by its state", () => {
    expect(pulseIcon(pulse())).toBe("clock");
    expect(pulseIcon(pulse({ enabled: false }))).toBe("pause");
    expect(pulseIcon(pulse({ problems: ["bad schedule"] }))).toBe("warning");
  });

  it("says when an action is due, as Home does", () => {
    const action: ActionInfo = {
      id: "act-1",
      name: "weekly_digest",
      run_at: iso(10 * MINUTE),
      agent: null,
      model_tier: null,
      current_run: null,
    };
    expect(actionWhen(action, "running", NOW)).toBe("in 10m");
    expect(actionWhen(action, "failed", NOW)).toBe("Won't run until it starts");
  });

  it("names an agent that isn't running", () => {
    expect(notRunningTitle("drifter", "stopped")).toBe("drifter is stopped");
    expect(notRunningTitle("brittle", "failed")).toBe("brittle can't start");
    expect(notRunningTitle("drifter", "starting")).toBe("drifter is starting");
  });
});
