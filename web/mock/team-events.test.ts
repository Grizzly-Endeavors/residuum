import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { HubServerMessage, TeamEvent, TeamEventPage } from "../src/lib/hub-types";
import { MOCK_DETERMINISTIC_BOOT_ID } from "./constants";
import { createMockEnv } from "./env";
import {
  createTeamEvents,
  DEFAULT_PAGE_SIZE,
  LOG_CAPACITY,
  MAX_PAGE_SIZE,
  PROTECTED_ENTRIES,
  type MockTeamEvents,
  type NewTeamEvent,
} from "./team-events";
import { createStubHub, fetchJson, startMockServer, type MockServerHarness } from "./test-support";

function entry(level: TeamEvent["level"], summary: string): NewTeamEvent {
  return {
    at: "2026-03-14T12:00:00.000Z",
    agent: null,
    kind: "hub_notice",
    level,
    summary,
    target: null,
  };
}

const info = (summary: string): NewTeamEvent => entry("info", summary);

function newLog(): { log: MockTeamEvents; frames: HubServerMessage[] } {
  const frames: HubServerMessage[] = [];
  const log = createTeamEvents(createMockEnv({ deterministic: true }), "boot-under-test", (frame) =>
    frames.push(frame),
  );
  return { log, frames };
}

const ids = (events: readonly TeamEvent[]): number[] => events.map((event) => event.id);

/** Every entry in the log, oldest first, read the way a client pages. */
function everyEntry(log: MockTeamEvents): TeamEvent[] {
  const all: TeamEvent[] = [];
  let before: number | undefined;
  for (;;) {
    const page = log.page({ before, limit: MAX_PAGE_SIZE });
    all.push(...page.events);
    if (page.next_before === null) break;
    before = page.next_before;
  }
  return all.reverse();
}

describe("the team event log", () => {
  it("numbers entries from 1, names the boot, and sends each entry as a team_event frame", () => {
    const { log, frames } = newLog();

    const first = log.record(info("first"));
    const second = log.record(info("second"));

    expect([first.id, second.id]).toEqual([1, 2]);
    expect(frames).toEqual([
      { type: "team_event", boot_id: "boot-under-test", event: first },
      { type: "team_event", boot_id: "boot-under-test", event: second },
    ]);
    expect(log.page({})).toEqual({
      boot_id: "boot-under-test",
      events: [second, first],
      next_before: null,
    });
  });

  it("pages back with before, forward with after, and holds the limit between 1 and the maximum", () => {
    const { log } = newLog();
    for (let n = 1; n <= 120; n++) log.record(info(`entry ${String(n)}`));

    const first = log.page({});
    expect(first.events).toHaveLength(DEFAULT_PAGE_SIZE);
    expect(first.events[0]?.id).toBe(120);
    expect(first.next_before).toBe(71);
    const second = log.page({ before: first.next_before ?? undefined, limit: 60 });
    expect(ids(second.events)).toEqual(Array.from({ length: 60 }, (_, i) => 70 - i));
    expect(second.next_before).toBe(11);
    expect(log.page({ before: 11 }).next_before).toBeNull();

    expect(ids(log.page({ after: 117 }).events)).toEqual([120, 119, 118]);
    expect(log.page({ after: 120 }).events).toEqual([]);
    // More newer entries than fit: the newest come first, and before walks back to the client's position.
    const capped = log.page({ after: 100, limit: 3 });
    expect(ids(capped.events)).toEqual([120, 119, 118]);
    expect(capped.next_before).toBe(118);
    expect(ids(log.page({ after: 100, before: 118 }).events)).toEqual(
      Array.from({ length: 17 }, (_, i) => 117 - i),
    );

    expect(log.page({ limit: 10_000 }).events).toHaveLength(120);
    expect(log.page({ limit: 0 }).events).toHaveLength(1);
  });

  it("holds a limit above the maximum to the maximum", () => {
    const { log } = newLog();
    for (let n = 1; n <= 250; n++) log.record(info(`entry ${String(n)}`));

    const page = log.page({ limit: 100_000 });

    expect(page.events).toHaveLength(MAX_PAGE_SIZE);
    expect(page.next_before).toBe(51);
  });

  it("drops routine entries before failures once it is full", () => {
    const { log } = newLog();
    for (let n = 0; n < 50; n++) log.record(entry("error", "failed"));
    for (let n = 0; n < LOG_CAPACITY; n++) log.record(info(`routine ${String(n)}`));

    const all = everyEntry(log);
    expect(all).toHaveLength(LOG_CAPACITY);
    expect(all.filter((event) => event.level === "error")).toHaveLength(50);
    expect(all.at(-1)?.id).toBe(50 + LOG_CAPACITY);
  });

  it("keeps the newest hundred warnings and errors through any amount of routine entries", () => {
    const { log } = newLog();
    for (let n = 0; n < 150; n++) {
      log.record(entry(n % 2 === 0 ? "warn" : "error", `problem ${String(n)}`));
    }
    for (let n = 0; n < 2 * LOG_CAPACITY; n++) log.record(info(`routine ${String(n)}`));

    const all = everyEntry(log);
    expect(all).toHaveLength(LOG_CAPACITY);
    expect(all.filter((event) => event.level !== "info").map((event) => event.summary)).toEqual(
      Array.from({ length: PROTECTED_ENTRIES }, (_, i) => `problem ${String(50 + i)}`),
    );
    expect(ids(all)).toEqual([...ids(all)].sort((a, b) => a - b));
  });

  it("evicts the oldest entry when every entry is a problem", () => {
    const { log } = newLog();
    for (let n = 0; n < LOG_CAPACITY + 10; n++) log.record(entry("error", `problem ${String(n)}`));
    const all = everyEntry(log);
    expect(all).toHaveLength(LOG_CAPACITY);
    expect(all[0]?.id).toBe(11);
  });

  it("starts over as a hub that has just started the agents, with ids from 1 again", () => {
    const { log } = newLog();
    log.record(info("left over from before"));

    log.begin([]);

    expect(log.page({}).events.map((event) => [event.id, event.kind])).toEqual([
      [1, "hub_started"],
    ]);
  });

  it("dates entries by the mock's clock", () => {
    const env = createMockEnv({ deterministic: true });
    const hub = createStubHub(env);
    const atlas = hub.createAgent("atlas");
    hub.teamEvents.begin([]);

    env.clock.advance(90_000);
    hub.teamEvents.userInboxAdded(atlas, "item");

    expect(hub.teamEvents.page({}).events.map((event) => event.at)).toEqual([
      "2026-03-14T12:01:30.000Z",
      "2026-03-14T12:00:00.000Z",
    ]);
  });
});

describe("the team event routes", () => {
  let mock: MockServerHarness;

  beforeEach(async () => {
    mock = await startMockServer({ deterministic: true });
  });

  afterEach(async () => {
    await mock.close();
  });

  const fetchPage = async (query = ""): Promise<TeamEventPage> => {
    const res = await fetchJson(`${mock.baseUrl}/api/hub/events${query}`);
    expect(res.status, JSON.stringify(res.body)).toBe(200);
    return res.body as TeamEventPage;
  };

  /** What the log says, oldest first, as `kind: summary`. */
  const told = async (): Promise<string[]> =>
    (await fetchPage("?limit=200")).events.reverse().map((e) => `${e.kind}: ${e.summary}`);

  const post = (path: string, body?: object): Promise<{ status: number; body: unknown }> =>
    fetchJson(`${mock.baseUrl}${path}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });

  it("starts with the hub's start and what starting its agents did", async () => {
    const page = await fetchPage();

    expect(page.boot_id).toBe(MOCK_DETERMINISTIC_BOOT_ID);
    expect(await told()).toEqual([
      "hub_started: Residuum started",
      "agent_started: atlas started",
      "agent_failed: brittle couldn't start: config error: providers.toml: model 'gpt-9' is not offered by provider 'openai'",
      "agent_started: scout started",
    ]);
    const failed = page.events.find((event) => event.kind === "agent_failed");
    expect(failed).toEqual({
      id: 3,
      at: "2026-03-14T12:00:00.000Z",
      agent: "brittle",
      kind: "agent_failed",
      level: "error",
      summary: expect.stringContaining("brittle couldn't start") as string,
      target: { kind: "agent_place", agent: "brittle", place: "chat" },
    });
  });

  it("tells starts, stops, creations and deletions in the backend's words", async () => {
    await post("/api/hub/agents/drifter/start");
    await post("/api/hub/agents/scout/stop");
    await post("/api/hub/agents", { name: "nova", providers_toml: "x = 1" });
    await fetchJson(`${mock.baseUrl}/api/hub/agents/atlas`, { method: "DELETE" });
    await post("/api/hub/agents/restore", { name: "atlas" });

    expect((await told()).slice(4)).toEqual([
      "agent_started: drifter started",
      "agent_stopped: scout stopped",
      "agent_started: nova started",
      "agent_created: nova was created",
      // A running agent stops before it is deleted, as it does in the backend.
      "agent_stopped: atlas stopped",
      "agent_deleted: atlas was deleted",
      "agent_started: atlas started",
      "agent_restored: atlas was restored",
    ]);
  });

  it("points a start, stop, creation and restoration at the agent's chat and a deletion nowhere", async () => {
    await post("/api/hub/agents/scout/stop");
    await fetchJson(`${mock.baseUrl}/api/hub/agents/atlas`, { method: "DELETE" });

    const { events } = await fetchPage();
    const stopped = events.find((event) => event.summary === "scout stopped");
    const deleted = events.find((event) => event.kind === "agent_deleted");
    expect(stopped?.target).toEqual({ kind: "agent_place", agent: "scout", place: "chat" });
    expect(deleted?.target).toBeNull();
  });

  it("tells a running agent's stop before its deletion, and no stop for an agent that wasn't running", async () => {
    await fetchJson(`${mock.baseUrl}/api/hub/agents/scout`, { method: "DELETE" });
    await fetchJson(`${mock.baseUrl}/api/hub/agents/drifter`, { method: "DELETE" });

    expect((await told()).slice(4)).toEqual([
      "agent_stopped: scout stopped",
      "agent_deleted: scout was deleted",
      "agent_deleted: drifter was deleted",
    ]);
  });

  it("tells a failed start again, and a change of settings not at all", async () => {
    await post("/api/hub/agents/brittle/start");
    await fetchJson(`${mock.baseUrl}/api/hub/agents/scout`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ autostart: false }),
    });

    const all = await told();
    expect(all.filter((line) => line.startsWith("agent_failed"))).toHaveLength(2);
    expect(all.filter((line) => line.startsWith("agent_started"))).toHaveLength(2);
  });

  it("tells a hub notice at its own level", async () => {
    await fetch(`${mock.baseUrl}/api/hub/config/raw`, {
      method: "PUT",
      body: "not valid toml [[[",
    });

    const notice = (await fetchPage()).events.find((event) => event.kind === "hub_notice");
    expect(notice).toMatchObject({
      agent: null,
      level: "warn",
      target: null,
      summary: expect.stringContaining("hub config reload failed") as string,
    });
  });

  it("tells the reply of a finished turn once, and not a turn that was stopped", async () => {
    const socket = await mock.openSocket("/api/agents/scout/ws");
    socket.send({ type: "send_message", id: "m1", content: "hello" });
    await socket.nextOfType("turn_ended");
    socket.send({ type: "send_message", id: "m2", content: "drop this one" });
    socket.send({ type: "cancel", reply_to: "m2" });
    await socket.settled();

    const replies = (await fetchPage()).events.filter((event) => event.kind === "agent_replied");
    expect(replies.map((event) => event.summary)).toEqual(["scout replied in your conversation"]);
    expect(replies[0]?.target).toEqual({ kind: "agent_place", agent: "scout", place: "chat" });
  });

  it("tells a teammate's reply the same way", async () => {
    await post("/api/mock/teammate-message?agent=atlas&from=scout");
    const replies = (await fetchPage()).events.filter((event) => event.kind === "agent_replied");
    expect(replies.map((event) => [event.agent, event.summary])).toEqual([
      ["atlas", "atlas replied in your conversation"],
    ]);
  });

  it("tells an item an agent saves in the user's inbox, and points at it", async () => {
    const saved = await post("/api/mock/user-inbox-add?agent=atlas", {
      title: "Pelican at the pier",
      body: "Seen at 8.",
    });
    expect(saved.status).toBe(200);
    const { id } = saved.body as { id: string };

    const [added] = (await fetchPage()).events;
    expect(added).toMatchObject({
      agent: "atlas",
      kind: "inbox_item_added",
      level: "info",
      summary: "atlas added an item to your inbox",
      target: { kind: "inbox_item", agent: "atlas", item_id: id },
    });
    const inbox = await fetchJson(`${mock.baseUrl}/api/hub/inbox?agent=atlas`);
    expect(JSON.stringify(inbox.body)).toContain(id);
  });

  it("tells a session when it starts and when it is stopped", async () => {
    const socket = await mock.openSocket("/api/agents/scout/ws");
    socket.send({ type: "send_message", id: "m1", content: "spawn compare retry windows" });
    const started = await socket.nextOfType("session_started");
    const session = started.session as { address: string; run_id: string };
    await socket.nextOfType("session_turn_ended");
    socket.send({ type: "session_stop", id: "s1", address: session.address });
    await socket.nextOfType("session_completed");

    const sessions = (await fetchPage()).events
      .filter((event) => event.kind.startsWith("session_"))
      .reverse();
    expect(sessions.map((event) => [event.kind, event.level, event.summary])).toEqual([
      ["session_started", "info", "scout started a session: compare retry windows"],
      ["session_finished", "warn", "scout's session was stopped: compare retry windows"],
    ]);
    expect(sessions.map((event) => event.target)).toEqual([
      { kind: "session", agent: "scout", run_id: session.run_id },
      { kind: "session", agent: "scout", run_id: session.run_id },
    ]);
  });

  it("tells a scheduled run only when it finishes, never as a session", async () => {
    const scout = mock.hub.agents.get("scout");
    if (scout === undefined) throw new Error("the scenario has scout");
    const [sample] = scout.state.sessions.live;
    if (sample === undefined) throw new Error("scout has a live session");
    const run = {
      ...sample,
      address: "scheduled-email-check-1a2b",
      run_id: "run-pulse-1",
      category: "scheduled" as const,
      source_label: "pulse:email_check",
      purpose: "Check the inbox",
    };
    scout.state.broadcast({ type: "session_started", session: run });
    scout.state.sessions.completed.unshift(run);
    scout.state.broadcast({
      type: "session_completed",
      address: run.address,
      run_id: run.run_id,
      status: "completed",
      error: null,
      error_details: null,
      episode_id: null,
    });
    const failing = { ...run, run_id: "run-action-2", source_label: "action:nightly digest" };
    scout.state.sessions.completed.unshift(failing);
    scout.state.broadcast({
      type: "session_completed",
      address: failing.address,
      run_id: failing.run_id,
      status: "failed",
      error: "The model is unavailable.",
      error_details: null,
      episode_id: null,
    });

    const runs = (await fetchPage()).events.filter(
      (event) => event.kind === "scheduled_run_finished" || event.kind.startsWith("session_"),
    );
    expect(runs.reverse().map((event) => [event.kind, event.level, event.summary])).toEqual([
      ["scheduled_run_finished", "info", 'scout finished the pulse "email_check"'],
      [
        "scheduled_run_finished",
        "error",
        'scout\'s scheduled action "nightly digest" failed: The model is unavailable.',
      ],
    ]);
  });

  it("sends each new entry to the hub socket under the boot id hub_boot announced", async () => {
    const socket = await mock.openSocket("/api/hub/ws");
    const boot = await socket.nextOfType("hub_boot");
    await socket.nextOfType("agents_snapshot");

    await post("/api/hub/agents/scout/stop");

    const frame = await socket.nextOfType("team_event");
    expect(frame.boot_id).toBe(boot.boot_id);
    expect(frame.event).toMatchObject({ id: 5, kind: "agent_stopped", summary: "scout stopped" });
    expect((await fetchPage()).boot_id).toBe(boot.boot_id);
  });

  it("answers a query it can't read with a JSON error that names the field", async () => {
    for (const [query, field] of [
      ["?before=newest", "before"],
      ["?after=-1", "after"],
      ["?limit=0", "limit"],
      ["?limit=many", "limit"],
    ] as const) {
      const res = await fetchJson(`${mock.baseUrl}/api/hub/events${query}`);
      expect(res.status, query).toBe(400);
      expect((res.body as { error: string }).error, query).toContain(field);
    }
  });

  it("starts over when the mock is reset, and gives the same ids and times again", async () => {
    await post("/api/hub/agents/scout/stop");
    const before = await fetchPage("?limit=200");
    expect(before.events).toHaveLength(5);

    await post("/api/mock/reset");
    const after = await fetchPage("?limit=200");

    expect(after.events).toHaveLength(4);
    expect(after.events.map((event) => [event.id, event.at])).toEqual(
      before.events.slice(1).map((event) => [event.id, event.at]),
    );
  });
});
