import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { OutboundA2aTaskSummary, PulseInfo } from "../src/lib/generated/protocol";
import type { AgentOverview, OverviewResponse } from "../src/lib/hub-types";
import { MOCK_DETERMINISTIC_BOOT_ID } from "./constants";
import { createMockEnv, type MockEnv } from "./env";
import {
  COALESCE_WINDOW_MS,
  OUTBOUND_NOTICE_MS,
  PREVIEW_CHARS,
  createOverview,
  plainPreview,
} from "./overview";
import type { MockPulse } from "./scheduled";
import type { MockAgent, MockHub } from "./state";
import {
  createStubHub,
  fetchJson,
  startMockServer,
  type Frame,
  type MockServerHarness,
  type TestSocket,
} from "./test-support";

describe("a preview", () => {
  it("drops markdown syntax and keeps the words, as the backend's does", () => {
    for (const [markdown, preview] of [
      ["**Done**: see [the report](https://example.com/r)", "Done: see the report"],
      ["# Plan\n\n- first\n- second\n\n1. third", "Plan first second third"],
      ["Run `cargo test` now", "Run cargo test now"],
      ["![a chart](chart.png) shows growth", "a chart shows growth"],
      ["~~old~~ new and *emphasis* and _more_", "old new and emphasis and more"],
      ["> quoted\n> text", "quoted text"],
      ["```rust\nlet x = 1;\n```\nafter", "let x = 1; after"],
      ["| a | b |\n|---|---|\n| 1 | 2 |", "a b 1 2"],
      ["- [x] shipped\n- [ ] pending", "shipped pending"],
      ["a <b>bold</b> word", "a bold word"],
      ["bold**ly**", "boldly"],
      ["keep snake_case_names whole", "keep snake_case_names whole"],
      ["  first line  \n\n\n second\tline\r\nthird  ", "first line second line third"],
    ] as const) {
      expect(plainPreview(markdown), markdown).toBe(preview);
    }
  });

  it("is empty when the text has nothing to show", () => {
    for (const text of ["", "   \n\t", "---", "<!-- a comment -->", "<div></div>"]) {
      expect(plainPreview(text), text).toBe("");
    }
  });

  it("is cut with an ellipsis within the limit, counting characters and dropping a trailing space", () => {
    expect(plainPreview("a".repeat(PREVIEW_CHARS))).toBe("a".repeat(PREVIEW_CHARS));

    const cut = plainPreview("b".repeat(PREVIEW_CHARS + 1));
    expect(Array.from(cut)).toHaveLength(PREVIEW_CHARS);
    expect(cut.endsWith("…")).toBe(true);

    const spaced = plainPreview(`${"é".repeat(PREVIEW_CHARS - 2)} ${"ü".repeat(50)}`);
    expect(spaced).toBe(`${"é".repeat(PREVIEW_CHARS - 2)}…`);
  });
});

describe("the overview of a stub hub", () => {
  it("is the agents by name, a stopped one that never ran with nothing scheduled and no outbound problems", () => {
    const hub = createStubHub(createMockEnv({ deterministic: true }));
    hub.createAgent("scout");
    hub.createAgent("atlas", { runState: "stopped" });

    const { boot_id: bootId, agents } = hub.overview.response();

    expect(bootId).toBe("stub-boot");
    expect(agents.map((agent) => agent.name)).toEqual(["atlas", "scout"]);
    const atlas = agents.find((agent) => agent.name === "atlas");
    expect(atlas?.upcoming).toEqual([]);
    expect(atlas?.outbound_problems).toEqual([]);
  });

  it("gathers a burst of changes into one frame that shows the last state", async () => {
    const env = createMockEnv({ deterministic: true });
    const hub = createStubHub(env);
    const scout = hub.createAgent("scout");
    const frames: AgentOverview[] = [];
    const overview = createOverview(env, "boot", hub.agents, (frame) => {
      if (frame.type === "agent_overview") frames.push(frame.overview);
    });
    overview.response();
    const unread = (): number => scout.state.inboxItems.filter((item) => !item.read).length;
    const before = unread();

    for (let n = 0; n < 5; n++) {
      scout.state.inboxItems.push({
        id: `new-${String(n)}`,
        title: "x",
        body: "",
        source: "agent",
        timestamp: env.clock.iso(),
        read: false,
        attachments: [],
      });
      overview.changed(scout);
    }
    await new Promise((done) => setTimeout(done, 20));

    expect(frames.map((frame) => frame.inbox_unread)).toEqual([before + 5]);
    expect(COALESCE_WINDOW_MS).toBe(1000);
  });
});

/**
 * A stub hub on a fixed clock at noon UTC on 2026-03-14, which is 08:00 in the
 * hub's New York zone, with the running agent `scout` and its sample schedule
 * (`createScheduled`) and the stopped `atlas`, which has none.
 */
function stubHub(): { env: MockEnv; hub: MockHub; scout: MockAgent; atlas: MockAgent } {
  const env = createMockEnv({ deterministic: true });
  const hub = createStubHub(env);
  const scout = hub.createAgent("scout");
  const atlas = hub.createAgent("atlas", { runState: "stopped" });
  return { env, hub, scout, atlas };
}

function overviewIn(hub: MockHub, name: string): AgentOverview {
  const found = hub.overview.response().agents.find((agent) => agent.name === name);
  if (found === undefined) throw new Error(`the overview has no ${name}`);
  return found;
}

/** What `upcoming` of the agent says, as `kind:name@at` for each run. */
function upcomingIn(hub: MockHub, name: string): string[] {
  return overviewIn(hub, name).upcoming.map((run) => `${run.kind}:${run.name}@${run.at}`);
}

function pulseOf(agent: MockAgent, name: string): MockPulse {
  const pulse = agent.state.scheduled.pulses.find((candidate) => candidate.name === name);
  if (pulse === undefined) throw new Error(`${agent.name} has no pulse ${name}`);
  return pulse;
}

describe("the upcoming runs of an overview", () => {
  it("lists the next three runs, pulses and actions together, soonest first, in the hub's timezone", () => {
    const { env, hub, scout } = stubHub();

    expect(upcomingIn(hub, "scout")).toEqual([
      "pulse:inbox_check@2026-03-14T09:00:00-04:00",
      "action:weekly_digest@2026-03-14T11:00:00-04:00",
      "action:review_open_prs@2026-03-15T10:00:00-04:00",
    ]);

    scout.state.scheduled.actions.push({
      id: "act-new",
      name: "stand_up",
      run_at: env.clock.isoIn(30 * 60_000),
      agent: null,
      model_tier: null,
    });
    expect(upcomingIn(hub, "scout")).toEqual([
      "action:stand_up@2026-03-14T08:30:00-04:00",
      "pulse:inbox_check@2026-03-14T09:00:00-04:00",
      "action:weekly_digest@2026-03-14T11:00:00-04:00",
    ]);
  });

  it("lists pulses before actions that run at the same moment", () => {
    const { env, hub, scout } = stubHub();
    scout.state.scheduled.actions = [
      {
        id: "act-same",
        name: "same_time",
        run_at: "2026-03-14T13:00:00.000Z",
        agent: null,
        model_tier: null,
      },
    ];
    expect(env.clock.iso()).toBe("2026-03-14T12:00:00.000Z");

    expect(upcomingIn(hub, "scout")).toEqual([
      "pulse:inbox_check@2026-03-14T09:00:00-04:00",
      "action:same_time@2026-03-14T09:00:00-04:00",
    ]);
  });

  it("pushes a run that falls after the active hours close to when they open again", () => {
    const { hub, scout } = stubHub();
    // The last run was at 07:00 and the schedule is two hours: 09:00 is when the window closes.
    pulseOf(scout, "inbox_check").activeHours = "08:00-09:00";

    expect(upcomingIn(hub, "scout")).toEqual([
      "action:weekly_digest@2026-03-14T11:00:00-04:00",
      "pulse:inbox_check@2026-03-15T08:00:00-04:00",
      "action:review_open_prs@2026-03-15T10:00:00-04:00",
    ]);
  });

  it("reads a pulse that has never run as due at the start of the current minute", () => {
    const { env, hub, scout } = stubHub();
    pulseOf(scout, "inbox_check").lastRunAt = null;
    expect(upcomingIn(hub, "scout")[0]).toBe("pulse:inbox_check@2026-03-14T08:00:00-04:00");

    env.clock.advance(37_000);

    expect(upcomingIn(hub, "scout")[0]).toBe("pulse:inbox_check@2026-03-14T08:00:00-04:00");
  });

  it("leaves out a pulse that is disabled, whose schedule or active hours can't be read, or whose active hours never open", () => {
    for (const change of [
      { enabled: false },
      { schedule: "soon" },
      { schedule: null },
      { activeHours: "eight to nine" },
      { activeHours: "25:00-26:00" },
      { activeHours: "09:00-09:00" },
    ] satisfies Array<Partial<MockPulse>>) {
      const { hub, scout } = stubHub();
      Object.assign(pulseOf(scout, "inbox_check"), change);

      expect(
        overviewIn(hub, "scout").upcoming.filter((run) => run.kind === "pulse"),
        JSON.stringify(change),
      ).toEqual([]);
    }
  });

  it("opens an overnight window in the evening of the same day", () => {
    const { hub, scout } = stubHub();
    // 08:00 in New York, and the window runs from 22:00 to 06:00.
    Object.assign(pulseOf(scout, "inbox_check"), { activeHours: "22:00-06:00", lastRunAt: null });

    expect(upcomingIn(hub, "scout")[1]).toBe("pulse:inbox_check@2026-03-14T22:00:00-04:00");
  });

  it("lists a stopped agent's runs too, an overdue action at the time it was set for", () => {
    const { env, hub, atlas } = stubHub();
    atlas.state.scheduled.actions.push({
      id: "act-overdue",
      name: "missed_while_stopped",
      run_at: env.clock.isoAgo(2 * 3_600_000),
      agent: null,
      model_tier: null,
    });

    expect(upcomingIn(hub, "atlas")).toEqual([
      "action:missed_while_stopped@2026-03-14T06:00:00-04:00",
    ]);
  });
});

describe("the outbound problems of an overview", () => {
  const task = (
    id: string,
    remote: string,
    unreachableForMs: number | null,
    env: MockEnv,
  ): OutboundA2aTaskSummary => ({
    task_id: id,
    agent: remote,
    sender_address: "main",
    state: "working",
    status_text: null,
    open: true,
    started_at: env.clock.isoAgo(3 * 3_600_000),
    unreachable_since: unreachableForMs === null ? null : env.clock.isoAgo(unreachableForMs),
  });

  it("lists the sample task that has been unreachable past the threshold, and not the one that answers", () => {
    const { env, hub } = stubHub();

    expect(overviewIn(hub, "scout").outbound_problems).toEqual([
      {
        task_id: "task-19c2",
        remote_agent: "laptop",
        status_text: null,
        unreachable_since: env.clock.isoAgo(17 * 60_000),
      },
    ]);
  });

  it("lists the longest unreachable task first, and leaves out a task still short of the threshold or closed", () => {
    const { env, hub, scout } = stubHub();
    const closed = {
      ...task("closed", "desktop", 5 * 3_600_000, env),
      open: false,
      state: "canceled",
    };
    scout.state.outboundTasks = [
      task("recent", "phone", 12 * 60_000, env),
      task("long", "laptop", 3 * 3_600_000, env),
      task("reachable", "lab", null, env),
      task("short", "nas", 2 * 60_000, env),
      closed,
    ];

    expect(overviewIn(hub, "scout").outbound_problems.map((problem) => problem.task_id)).toEqual([
      "long",
      "recent",
    ]);
  });

  it("is empty for an agent that is not running", () => {
    const { env, hub, atlas } = stubHub();
    atlas.state.outboundTasks = [task("stuck", "laptop", 3_600_000, env)];

    expect(overviewIn(hub, "atlas").outbound_problems).toEqual([]);
  });

  describe("when a streak passes the threshold with nothing to announce it", () => {
    const unreachableUntilNotice = (env: MockEnv, aheadMs: number): OutboundA2aTaskSummary => ({
      ...task("task-1", "laptop", null, env),
      unreachable_since: new Date(env.clock.now() - OUTBOUND_NOTICE_MS + aheadMs).toISOString(),
    });

    /** An overview over a hub whose `scout` has just that task, and the frames it sends. */
    function watching(
      env: MockEnv,
      aheadMs: number,
    ): { problems: () => number[]; frames: AgentOverview[] } {
      const hub = createStubHub(env);
      const scout = hub.createAgent("scout");
      scout.state.outboundTasks = [unreachableUntilNotice(env, aheadMs)];
      const frames: AgentOverview[] = [];
      const overview = createOverview(env, "boot", hub.agents, (frame) => {
        if (frame.type === "agent_overview") frames.push(frame.overview);
      });
      return {
        problems: () => overview.response().agents.map((agent) => agent.outbound_problems.length),
        frames,
      };
    }

    it("sends the agent's overview once the clock reaches the threshold", async () => {
      const env = createMockEnv({ delayScale: 1 });
      const { problems, frames } = watching(env, 150);
      expect(problems()).toEqual([0]);

      await vi.waitFor(
        () => {
          expect(frames).toHaveLength(1);
        },
        { timeout: 4000 },
      );

      expect(frames[0]?.outbound_problems.map((problem) => problem.task_id)).toEqual(["task-1"]);
      env.reset();
    });

    it("waits for a clock that has not got there, and keeps no timer spinning", async () => {
      const env = createMockEnv({ deterministic: true });
      const { problems, frames } = watching(env, 150);
      expect(problems()).toEqual([0]);

      await new Promise((done) => setTimeout(done, 50));
      expect(frames).toEqual([]);

      env.clock.advance(200);
      expect(problems()).toEqual([1]);
    });
  });
});

describe("the overview routes", () => {
  let mock: MockServerHarness;
  let socket: TestSocket;

  beforeEach(async () => {
    mock = await startMockServer({ deterministic: true });
    socket = await mock.openSocket("/api/hub/ws");
    await socket.nextOfType("agents_snapshot");
  });

  afterEach(async () => {
    await mock.close();
  });

  const post = (path: string, body?: object): Promise<{ status: number; body: unknown }> =>
    fetchJson(`${mock.baseUrl}${path}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });

  const fetchOverview = async (): Promise<OverviewResponse> => {
    const res = await fetchJson(`${mock.baseUrl}/api/hub/overview`);
    expect(res.status, JSON.stringify(res.body)).toBe(200);
    return res.body as OverviewResponse;
  };

  const overviewOf = async (name: string): Promise<AgentOverview> => {
    const found = (await fetchOverview()).agents.find((agent) => agent.name === name);
    if (found === undefined) throw new Error(`the overview has no ${name}`);
    return found;
  };

  /** The next `agent_overview` frame about `name` that satisfies `wanted`. */
  const nextFrame = async (
    name: string,
    wanted: (frame: AgentOverview) => boolean = () => true,
  ): Promise<AgentOverview> => {
    const frame = await socket.next(
      (candidate) =>
        candidate.type === "agent_overview" &&
        (candidate.overview as AgentOverview).name === name &&
        wanted(candidate.overview as AgentOverview),
    );
    return (frame as Frame & { overview: AgentOverview }).overview;
  };

  it("lists every agent by name under the boot id", async () => {
    const body = await fetchOverview();

    expect(body.boot_id).toBe(MOCK_DETERMINISTIC_BOOT_ID);
    expect(body.agents.map((agent) => agent.name)).toEqual([
      "atlas",
      "brittle",
      "drifter",
      "scout",
    ]);
  });

  it("shows an agent that never ran with nothing, and one that has with its conversation, sessions and unread items", async () => {
    const drifter = await overviewOf("drifter");
    expect(drifter).toEqual({
      name: "drifter",
      last_message: null,
      live_sessions: [],
      upcoming: [],
      inbox_unread: 0,
      outbound_problems: [],
    });

    const scout = await overviewOf("scout");
    expect(scout.last_message).toMatchObject({ role: expect.any(String) as string });
    expect(scout.last_message?.preview.length).toBeGreaterThan(0);
    expect(scout.live_sessions.map((session) => session.source_label)).toEqual([
      "discord:#builds",
      "agent:researcher",
      "artifact:wiki-graph",
    ]);
    expect(scout.live_sessions[0]).toEqual({
      address: "external-discord-4f1c9a2e7b3d0856",
      run_id: "run-live-discord",
      category: "external",
      source_label: "discord:#builds",
      purpose: "Conversation in #builds",
      state: "idle",
      started_at: "2026-03-14T11:34:00.000Z",
    });
    const unread = mock.hub.agents.get("scout")?.state.inboxItems.filter((item) => !item.read);
    expect(scout.inbox_unread).toBe(unread?.length);
  });

  it("sends the reply of a finished turn as the agent's last message", async () => {
    const chat = await mock.openSocket("/api/agents/scout/ws");
    chat.send({ type: "send_message", id: "m1", content: "tell me about the memory settings" });

    const frame = await nextFrame("scout", (overview) =>
      (overview.last_message?.preview ?? "").startsWith("I've looked into that"),
    );

    expect(frame.last_message).toMatchObject({
      role: "assistant",
      at: "2026-03-14T12:00:00Z",
      at_precision: "minute",
    });
    expect(frame.last_message?.preview).toContain("Key Points Configuration — The settings");
    expect(frame.last_message?.preview).not.toMatch(/[*`#]/);
    expect((await overviewOf("scout")).last_message).toEqual(frame.last_message);
  });

  it("shows what the user said when a turn is stopped before its reply", async () => {
    const chat = await mock.openSocket("/api/agents/atlas/ws");
    chat.send({ type: "send_message", id: "m1", content: "drop this one" });
    chat.send({ type: "cancel", reply_to: "m1" });

    const frame = await nextFrame("atlas", (overview) => overview.last_message?.role === "user");

    expect(frame.last_message?.preview).toBe("drop this one");
  });

  it("sends a new session as it starts and clears an agent's sessions when it stops", async () => {
    const chat = await mock.openSocket("/api/agents/scout/ws");
    chat.send({ type: "send_message", id: "m1", content: "spawn compare retry windows" });

    const started = await nextFrame("scout", (overview) => overview.live_sessions.length > 3);
    expect(started.live_sessions.at(-1)?.purpose).toBe("compare retry windows");

    await post("/api/hub/agents/scout/stop");
    const stopped = await nextFrame("scout", (overview) => overview.live_sessions.length === 0);
    expect(stopped.live_sessions).toEqual([]);
    expect((await overviewOf("scout")).live_sessions).toEqual([]);
  });

  it("counts the inbox again after an agent saves an item and after the hub's actions", async () => {
    const before = (await overviewOf("atlas")).inbox_unread;

    const saved = (await post("/api/mock/user-inbox-add?agent=atlas", { title: "Pelican" }))
      .body as {
      id: string;
    };
    expect(
      (await nextFrame("atlas", (overview) => overview.inbox_unread === before + 1)).name,
    ).toBe("atlas");

    await fetch(`${mock.baseUrl}/api/hub/inbox/atlas/${saved.id}/read`, { method: "PUT" });
    await nextFrame("atlas", (overview) => overview.inbox_unread === before);

    await post(`/api/hub/inbox/atlas/${saved.id}/archive`);
    await post(`/api/hub/inbox/atlas/${saved.id}/restore`);
    await fetch(`${mock.baseUrl}/api/hub/inbox/atlas/${saved.id}/read`, { method: "PUT" });
    expect((await overviewOf("atlas")).inbox_unread).toBe(before);
  });

  it("sends a created agent at once and sends nothing more about a deleted one", async () => {
    await post("/api/hub/agents", { name: "nova", providers_toml: "x = 1" });
    const created = await nextFrame("nova");
    expect(created.name).toBe("nova");
    expect(created.last_message?.preview).toContain("this is nova");

    await post("/api/mock/user-inbox-add?agent=atlas", { title: "Pelican" });
    await fetchJson(`${mock.baseUrl}/api/hub/agents/atlas`, { method: "DELETE" });

    const rest = await socket.quietFrames(100);
    expect(
      rest.filter(
        (frame) =>
          frame.type === "agent_overview" && (frame.overview as AgentOverview).name === "atlas",
      ),
    ).toEqual([]);
    expect((await fetchOverview()).agents.map((agent) => agent.name)).not.toContain("atlas");
  });

  it("tells clients what a request found that they hadn't been told", async () => {
    const drifter = mock.hub.agents.get("drifter");
    if (drifter === undefined) throw new Error("the scenario has drifter");
    await fetchOverview();

    drifter.state.inboxItems.push({
      id: "hand-placed",
      title: "Left by hand",
      body: "",
      source: "agent",
      timestamp: mock.hub.env.clock.iso(),
      read: false,
      attachments: [],
    });

    expect((await overviewOf("drifter")).inbox_unread).toBe(1);
    expect((await nextFrame("drifter")).inbox_unread).toBe(1);
  });

  it("gives a pulse the same next time as the Scheduled view does", async () => {
    const pulses = (await fetchJson(`${mock.baseUrl}/api/agents/scout/scheduled/pulses`))
      .body as PulseInfo[];
    const listed = (await overviewOf("scout")).upcoming.filter((run) => run.kind === "pulse");

    expect(listed.length).toBeGreaterThan(0);
    for (const run of listed) {
      const pulse = pulses.find((candidate) => candidate.name === run.name);
      expect(Date.parse(run.at), run.name).toBe(Date.parse(pulse?.next_fire_at ?? ""));
    }
    expect(
      pulses.filter((pulse) => pulse.next_fire_at !== null).map((pulse) => pulse.name),
    ).toEqual(listed.map((run) => run.name));
  });

  it("sends an agent's runs again when a pulse is switched on or an action is cancelled", async () => {
    const names = (overview: AgentOverview): string[] => overview.upcoming.map((run) => run.name);
    expect(names(await overviewOf("scout"))).toEqual([
      "inbox_check",
      "weekly_digest",
      "review_open_prs",
    ]);

    await fetchJson(`${mock.baseUrl}/api/agents/scout/scheduled/pulses/nightly_review/enabled`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ enabled: true }),
    });
    const switchedOn = await nextFrame("scout", (overview) =>
      names(overview).includes("nightly_review"),
    );
    expect(names(switchedOn)).toEqual(["inbox_check", "weekly_digest", "nightly_review"]);

    await fetchJson(`${mock.baseUrl}/api/agents/scout/scheduled/actions/act-7f3a2c`, {
      method: "DELETE",
    });
    const cancelled = await nextFrame(
      "scout",
      (overview) => !names(overview).includes("weekly_digest"),
    );
    expect(names(cancelled)).toEqual(["inbox_check", "nightly_review", "review_open_prs"]);
  });

  it("clears an outbound problem when the user stops watching the task", async () => {
    expect((await overviewOf("scout")).outbound_problems.map((problem) => problem.task_id)).toEqual(
      ["task-19c2"],
    );

    const stopped = await post("/api/agents/scout/a2a/outbound/task-19c2/stop-watching");
    expect(stopped.status).toBe(200);

    const frame = await nextFrame("scout", (overview) => overview.outbound_problems.length === 0);
    expect(frame.outbound_problems).toEqual([]);
  });
});
