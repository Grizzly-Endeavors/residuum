import { describe, expect, it } from "vitest";
import type { AgentOverview, AgentSummary, HubInboxItem, OutboundProblem } from "./hub-types";
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

function item(agentName: string, id: string, at: string, read = false): HubInboxItem {
  return { agent: agentName, id, title: id, body: "", source: "agent", at, read, attachments: [] };
}

describe("deriveNeedsYou", () => {
  it("lists failures, then unreachable agents, then inbox items, newest first within each", () => {
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
      unreadItems: [
        item("scout", "newer", "2026-03-14T08:00:00Z"),
        item("atlas", "older", "2026-03-13T08:00:00Z"),
      ],
    });

    expect(needs.items.map((i) => i.key)).toEqual([
      "failed:crumbly",
      "failed:brittle",
      "outbound:scout:t-new",
      "outbound:atlas:t-old",
      "inbox:scout:newer",
      "inbox:atlas:older",
    ]);
    expect(needs.count).toBe(6);
    expect(needs.moreInInbox).toBe(0);
  });

  it("is empty when nothing needs the user", () => {
    expect(deriveNeedsYou({ agents: [agent("atlas")], overviews: {}, unreadItems: [] })).toEqual({
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
      unreadItems: [],
    });
    expect(needs.items).toEqual([]);
    expect(needs.count).toBe(0);
  });

  it("shows the five newest unread items and counts the rest under them, not toward the count", () => {
    const unread = Array.from({ length: 7 }, (_, n) =>
      item("atlas", `i${String(n)}`, `2026-03-14T0${String(9 - n)}:00:00Z`),
    );
    const needs = deriveNeedsYou({
      agents: [agent("atlas")],
      overviews: { atlas: overview("atlas", { inbox_unread: 9 }) },
      unreadItems: unread,
    });
    expect(needs.items.map((i) => i.key)).toEqual([
      "inbox:atlas:i0",
      "inbox:atlas:i1",
      "inbox:atlas:i2",
      "inbox:atlas:i3",
      "inbox:atlas:i4",
    ]);
    expect(needs.moreInInbox).toBe(4);
    expect(needs.count).toBe(5);
  });

  it("counts unread inbox items before their details arrive", () => {
    const needs = deriveNeedsYou({
      agents: [agent("atlas"), failed("brittle", "2026-03-14T10:00:00Z")],
      overviews: { atlas: overview("atlas", { inbox_unread: 2 }) },
      unreadItems: [],
    });
    expect(needs.items.map((i) => i.kind)).toEqual(["failed"]);
    expect(needs.moreInInbox).toBe(2);
    expect(needs.count).toBe(3);
  });

  it("skips items already read and items of agents that no longer exist", () => {
    const needs = deriveNeedsYou({
      agents: [agent("atlas")],
      overviews: { atlas: overview("atlas", { inbox_unread: 1 }) },
      unreadItems: [
        item("atlas", "read", "2026-03-14T10:00:00Z", true),
        item("gone", "orphan", "2026-03-14T10:00:00Z"),
        item("atlas", "unread", "2026-03-14T09:00:00Z"),
      ],
    });
    expect(needs.items.map((i) => i.key)).toEqual(["inbox:atlas:unread"]);
  });

  it("puts a failure with no recorded time after the dated ones", () => {
    const needs = deriveNeedsYou({
      agents: [agent("brittle", { state: "failed" }), failed("crumbly", "2026-03-14T10:00:00Z")],
      overviews: {},
      unreadItems: [],
    });
    expect(needs.items.map((i) => i.key)).toEqual(["failed:crumbly", "failed:brittle"]);
  });
});
