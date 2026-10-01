import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AgentOverview, OverviewResponse } from "../src/lib/hub-types";
import { MOCK_DETERMINISTIC_BOOT_ID } from "./constants";
import { createMockEnv } from "./env";
import { COALESCE_WINDOW_MS, PREVIEW_CHARS, createOverview, plainPreview } from "./overview";
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
  it("is the agents by name, each with nothing scheduled and no outbound problems", () => {
    const hub = createStubHub(createMockEnv({ deterministic: true }));
    hub.createAgent("scout");
    hub.createAgent("atlas", { runState: "stopped" });

    const { boot_id: bootId, agents } = hub.overview.response();

    expect(bootId).toBe("stub-boot");
    expect(agents.map((agent) => agent.name)).toEqual(["atlas", "scout"]);
    for (const agent of agents) {
      expect(agent.upcoming, agent.name).toEqual([]);
      expect(agent.outbound_problems, agent.name).toEqual([]);
    }
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
});
