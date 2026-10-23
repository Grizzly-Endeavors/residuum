import { describe, expect, it } from "vitest";
import type { AgentOverview, AgentSummary, MailThreadSummary, OutboundProblem } from "./hub-types";
import { deriveNeedsYou } from "./needs-you";

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

function failed(name: string, at: string): AgentSummary {
  return agent(name, {
    state: "failed",
    last_error: { message: `${name} couldn't start`, kind: "config", reason: "bad", at },
  });
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

function problem(taskId: string, since: string): OutboundProblem {
  return { task_id: taskId, remote_agent: "laptop", status_text: null, unreachable_since: since };
}

function thread(
  agentName: string,
  id: string,
  at: string,
  unread = true,
  participants = [agentName],
): MailThreadSummary {
  return {
    thread_id: `${agentName}/user/${id}`,
    subject: id,
    participants,
    message_count: 1,
    latest_at: at,
    snippet: "",
    unread,
    has_attachments: false,
    in_inbox: true,
    in_sent: false,
    archived: false,
  };
}

describe("deriveNeedsYou", () => {
  it("lists failures, then unreachable agents, then inbox threads, newest first within each", () => {
    const needs = deriveNeedsYou({
      agents: [
        agent("atlas"),
        failed("brittle", "2026-03-14T10:00:00Z"),
        failed("crumbly", "2026-03-14T11:00:00Z"),
        agent("scout"),
      ],
      overviews: {
        atlas: overview("atlas", {
          inbox_unread: 1,
          outbound_problems: [problem("t-old", "2026-03-14T09:00:00Z")],
        }),
        scout: overview("scout", {
          inbox_unread: 1,
          outbound_problems: [problem("t-new", "2026-03-14T11:30:00Z")],
        }),
      },
      unreadThreads: [
        thread("scout", "newer", "2026-03-14T08:00:00Z"),
        thread("atlas", "older", "2026-03-13T08:00:00Z"),
      ],
    });

    expect(needs.items.map((i) => i.key)).toEqual([
      "failed:crumbly",
      "failed:brittle",
      "outbound:scout:t-new",
      "outbound:atlas:t-old",
      "inbox:scout/user/newer",
      "inbox:atlas/user/older",
    ]);
    expect(needs.count).toBe(6);
    expect(needs.moreInInbox).toBe(0);
  });

  it("is empty when nothing needs the user", () => {
    expect(deriveNeedsYou({ agents: [agent("atlas")], overviews: {}, unreadThreads: [] })).toEqual({
      items: [],
      moreInInbox: 0,
      count: 0,
    });
  });

  it("leaves out the tasks of an agent that isn't running", () => {
    const needs = deriveNeedsYou({
      agents: [agent("atlas", { state: "stopped" }), agent("scout", { state: "starting" })],
      overviews: {
        atlas: overview("atlas", { outbound_problems: [problem("t1", "2026-03-14T09:00:00Z")] }),
        scout: overview("scout", { outbound_problems: [problem("t2", "2026-03-14T09:00:00Z")] }),
      },
      unreadThreads: [],
    });
    expect(needs.items).toEqual([]);
    expect(needs.count).toBe(0);
  });

  it("shows the five newest unread threads and counts the rest under them, not toward the count", () => {
    const unread = Array.from({ length: 7 }, (_, n) =>
      thread("atlas", `i${String(n)}`, `2026-03-14T0${String(9 - n)}:00:00Z`),
    );
    const needs = deriveNeedsYou({
      agents: [agent("atlas")],
      overviews: { atlas: overview("atlas", { inbox_unread: 9 }) },
      unreadThreads: unread,
    });
    expect(needs.items.map((i) => i.key)).toEqual([
      "inbox:atlas/user/i0",
      "inbox:atlas/user/i1",
      "inbox:atlas/user/i2",
      "inbox:atlas/user/i3",
      "inbox:atlas/user/i4",
    ]);
    expect(needs.moreInInbox).toBe(4);
    expect(needs.count).toBe(5);
  });

  it("counts unread inbox items before their threads arrive", () => {
    const needs = deriveNeedsYou({
      agents: [agent("atlas"), failed("brittle", "2026-03-14T10:00:00Z")],
      overviews: { atlas: overview("atlas", { inbox_unread: 2 }) },
      unreadThreads: [],
    });
    expect(needs.items.map((i) => i.kind)).toEqual(["failed"]);
    expect(needs.moreInInbox).toBe(2);
    expect(needs.count).toBe(3);
  });

  it("skips threads already read and threads only agents that no longer exist are in", () => {
    const needs = deriveNeedsYou({
      agents: [agent("atlas")],
      overviews: { atlas: overview("atlas", { inbox_unread: 2 }) },
      unreadThreads: [
        thread("atlas", "read", "2026-03-14T10:00:00Z", false),
        thread("gone", "orphan", "2026-03-14T10:00:00Z"),
        thread("gone", "shared", "2026-03-14T09:30:00Z", true, ["gone", "atlas"]),
        thread("atlas", "unread", "2026-03-14T09:00:00Z"),
      ],
    });
    expect(needs.items.map((i) => i.key)).toEqual([
      "inbox:gone/user/shared",
      "inbox:atlas/user/unread",
    ]);
  });

  it("counts nothing more in the Inbox once the threads listed are all there are", () => {
    // One thread holding three unread items.
    const needs = deriveNeedsYou({
      agents: [agent("atlas")],
      overviews: { atlas: overview("atlas", { inbox_unread: 3 }) },
      unreadThreads: [thread("atlas", "a", "2026-03-14T10:00:00Z")],
      unreadThreadsComplete: true,
    });
    expect(needs.moreInInbox).toBe(0);
    expect(needs.count).toBe(1);
  });

  it("puts a failure with no recorded time after the dated ones", () => {
    const needs = deriveNeedsYou({
      agents: [agent("brittle", { state: "failed" }), failed("crumbly", "2026-03-14T10:00:00Z")],
      overviews: {},
      unreadThreads: [],
    });
    expect(needs.items.map((i) => i.key)).toEqual(["failed:crumbly", "failed:brittle"]);
  });

  it("shows one decision model item while it is down, and none once it answers", () => {
    const outage = {
      kind: "unreachable" as const,
      message: "Couldn't reach Ollama at http://localhost:11434.",
      since: "2026-10-03T14:00:00Z",
    };
    const down = deriveNeedsYou({
      agents: [failed("brittle", "2026-10-03T15:00:00Z")],
      overviews: {},
      unreadThreads: [],
      systemOne: { configured: true, provider: "Ollama", model: "nimble", outage },
    });
    expect(down.items.map((i) => [i.key, i.severity])).toEqual([
      ["failed:brittle", "error"],
      ["system_one", "warn"],
    ]);
    expect(down.count).toBe(2);

    const back = deriveNeedsYou({
      agents: [],
      overviews: {},
      unreadThreads: [],
      systemOne: { configured: true, provider: "Ollama", model: "nimble", outage: null },
    });
    expect(back.items).toEqual([]);
  });

  it("says nothing about an unconfigured decision model nobody has asked", () => {
    const needs = deriveNeedsYou({
      agents: [],
      overviews: {},
      unreadThreads: [],
      systemOne: { configured: false, provider: null, model: null, outage: null },
    });
    expect(needs.items).toEqual([]);
  });
});
