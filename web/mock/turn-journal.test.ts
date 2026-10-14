import { describe, expect, it } from "vitest";
import type { ServerMessage } from "../src/lib/generated/protocol";
import { MockTurnJournal } from "./turn-journal";

const started: ServerMessage = {
  type: "turn_started",
  reply_to: "t1",
  origin: { endpoint: "ws", visibility: "user" },
};
const delta = (text: string, call = 0): ServerMessage => ({
  type: "text_delta",
  reply_to: "t1",
  call,
  text,
});
const usage = (tokens: number): ServerMessage => ({
  type: "turn_usage",
  reply_to: "t1",
  output_tokens: tokens,
  has_usage: true,
  tool_calls: 0,
  session_totals: null,
});

describe("MockTurnJournal", () => {
  it("holds the turn so far, each call's pieces joined and the newest usage", () => {
    let tick = 0;
    const journal = new MockTurnJournal(() => `2026-03-14T12:00:0${String(tick++)}.000Z`);
    for (const frame of [
      started,
      usage(1),
      delta("Hel"),
      delta("lo"),
      delta("Next", 1),
      usage(2),
    ]) {
      journal.record(frame);
    }
    const turn = journal.snapshot();
    expect(turn?.reply_to).toBe("t1");
    expect(turn?.started_at).toBe("2026-03-14T12:00:00.000Z");
    expect(turn?.frames.map((entry) => entry.frame)).toEqual([
      started,
      delta("Hello"),
      delta("Next", 1),
      usage(2),
    ]);
    // The joined piece keeps the time of its first.
    expect(turn?.frames[1]?.at).toBe("2026-03-14T12:00:02.000Z");
  });

  it("lets go of a turn when it ends, and ignores what belongs to no turn", () => {
    const journal = new MockTurnJournal(() => "2026-03-14T12:00:00.000Z");
    journal.record(started);
    journal.record({ type: "turn_ended", reply_to: "t1" });
    journal.record({ type: "response", reply_to: "", endpoint: "ws", content: "posted" });
    journal.record({ type: "notice", message: "hello" });
    expect(journal.snapshot()).toBeNull();
  });

  it("gives out copies, so what a page is sent doesn't change under it", () => {
    const journal = new MockTurnJournal(() => "2026-03-14T12:00:00.000Z");
    journal.record(started);
    journal.record(delta("a"));
    const first = journal.snapshot();
    journal.record(delta("b"));
    expect(first?.frames[1]?.frame).toEqual(delta("a"));
  });
});
