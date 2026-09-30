import { afterEach, describe, expect, it } from "vitest";
import {
  startMockServer,
  type Frame,
  type MockServerHarness,
  type TestSocket,
} from "./test-support";
import { sleep } from "./util";

interface Step {
  method: string;
  path: string;
  body?: unknown;
}

/** An interaction across the mock's areas: reads, writes, lifecycle changes and ids it hands out. */
const STEPS: readonly Step[] = [
  { method: "GET", path: "/api/hub/status" },
  { method: "GET", path: "/api/hub/agents" },
  { method: "GET", path: "/api/agents/scout/chat/history" },
  { method: "GET", path: "/api/agents/scout/chat/history?episode=ep-003" },
  { method: "GET", path: "/api/agents/scout/sessions" },
  { method: "GET", path: "/api/agents/scout/sessions/runs/run-live-research/transcript" },
  { method: "GET", path: "/api/agents/scout/inbox" },
  { method: "GET", path: "/api/hub/inbox" },
  { method: "GET", path: "/api/agents/scout/scheduled/pulses" },
  { method: "GET", path: "/api/agents/scout/scheduled/actions" },
  { method: "GET", path: "/api/agents/scout/checkpoints?repo=workspace" },
  { method: "GET", path: "/api/hub/checkpoints?repo=team" },
  { method: "GET", path: "/api/agents/scout/status" },
  { method: "GET", path: "/api/agents/scout/workspace/files" },
  { method: "GET", path: "/api/agents/scout/workspace/tree?content=true" },
  { method: "GET", path: "/api/team/workbench/artifacts" },
  { method: "GET", path: "/api/team/workbench/info" },
  { method: "GET", path: "/api/hub/a2a/keys" },
  { method: "POST", path: "/api/hub/a2a/keys", body: { name: "phone", description: "my phone" } },
  { method: "GET", path: "/api/hub/a2a/keys" },
  { method: "PUT", path: "/api/agents/scout/workspace/file", body: { path: "n.md", content: "x" } },
  { method: "POST", path: "/api/agents/scout/workspace/dir", body: { path: "d/e" } },
  { method: "GET", path: "/api/agents/scout/workspace/files" },
  { method: "POST", path: "/api/hub/agents/drifter/start" },
  { method: "POST", path: "/api/hub/agents/scout/restart" },
  { method: "DELETE", path: "/api/hub/agents/atlas" },
  { method: "GET", path: "/api/hub/agents/deleted" },
  { method: "POST", path: "/api/hub/agents/restore", body: { name: "atlas" } },
  { method: "POST", path: "/api/agents/scout/agent-inbox", body: { body: "Look at this" } },
  { method: "POST", path: "/api/hub/update/check" },
  { method: "GET", path: "/api/hub/status" },
];

/** Run the steps, and record each response exactly as the server sent it. */
async function respond(mock: MockServerHarness): Promise<string[]> {
  const seen: string[] = [];
  for (const step of STEPS) {
    const res = await fetch(`${mock.baseUrl}${step.path}`, {
      method: step.method,
      headers: { "Content-Type": "application/json" },
      body: step.body === undefined ? undefined : JSON.stringify(step.body),
    });
    seen.push(`${step.method} ${step.path} -> ${String(res.status)} ${await res.text()}`);
  }
  return seen;
}

/** Chat with scout and keep every frame of its turn, tool calls included. */
async function chat(mock: MockServerHarness): Promise<Frame[]> {
  const socket: TestSocket = await mock.openSocket("/api/agents/scout/ws");
  socket.send({ type: "set_verbose", enabled: true });
  socket.send({ type: "send_message", id: "m1", content: "spawn look into the fallback" });
  await socket.nextOfType("turn_ended");
  await socket.nextOfType("session_turn_ended");
  await socket.close();
  return socket.frames;
}

describe("a deterministic mock", () => {
  let mock: MockServerHarness | null = null;
  const others: MockServerHarness[] = [];

  afterEach(async () => {
    await mock?.close();
    mock = null;
    await Promise.all(others.splice(0).map((other) => other.close()));
  });

  async function start(
    options: Parameters<typeof startMockServer>[0] = {},
  ): Promise<MockServerHarness> {
    mock = await startMockServer({ deterministic: true, ...options });
    return mock;
  }

  const reset = async (server: MockServerHarness): Promise<void> => {
    const res = await fetch(`${server.baseUrl}/api/mock/reset`, { method: "POST" });
    expect(res.status).toBe(200);
  };

  it("gives byte-identical responses to the same interaction after a reset", async () => {
    const server = await start();
    const first = await respond(server);
    await reset(server);
    const second = await respond(server);
    expect(second).toEqual(first);
    // Not an interaction that changed nothing.
    expect(first.join("\n")).toContain("rsdm_a2a_mock");
    expect(first.join("\n")).toContain("ckpt-atlas-");
  });

  it("gives the same responses as a second mock started fresh", async () => {
    const first = await respond(await start());
    const other = await startMockServer({ deterministic: true });
    others.push(other);
    expect(await respond(other)).toEqual(first);
  });

  it("gives the same frames to the same chat after a reset", async () => {
    const server = await start();
    const first = await chat(server);
    await reset(server);
    const second = await chat(server);
    expect(second).toEqual(first);
    expect(first.map((frame) => frame.type)).toContain("session_started");
    expect(first.map((frame) => frame.type)).toContain("tool_call");
  });

  it("keeps the clock still, and moves it only when told", async () => {
    const server = await start();
    const now = async (): Promise<string> =>
      (
        (await (
          await fetch(`${server.baseUrl}/api/agents/scout/scheduled/pulses`)
        ).json()) as Array<{
          next_fire_at: string;
        }>
      )[0]?.next_fire_at ?? "";
    const before = await now();
    await sleep(30);
    expect(await now()).toBe(before);

    const advanced = await fetch(`${server.baseUrl}/api/mock/clock/advance`, {
      method: "POST",
      body: JSON.stringify({ ms: 3_600_000 }),
    });
    expect(await advanced.json()).toEqual({ now: "2026-03-14T13:00:00.000Z" });
    await reset(server);
    expect(server.hub.env.clock.iso()).toBe("2026-03-14T12:00:00.000Z");
  });

  it("puts the hub and its agents back as they started", async () => {
    const server = await start();
    const created = await fetch(`${server.baseUrl}/api/hub/agents`, {
      method: "POST",
      body: JSON.stringify({ name: "extra", providers_toml: "" }),
    });
    expect(created.status).toBe(201);
    await fetch(`${server.baseUrl}/api/hub/agents/scout/stop`, { method: "POST" });
    await fetch(`${server.baseUrl}/api/agents/atlas/workspace/file`, {
      method: "PUT",
      body: JSON.stringify({ path: "gone.md", content: "x" }),
    });
    await fetch(`${server.baseUrl}/api/hub/secrets`, {
      method: "POST",
      body: JSON.stringify({ name: "temp", value: "v" }),
    });
    await fetch(`${server.baseUrl}/api/team/workbench/artifacts/tip-splitter`, {
      method: "DELETE",
    });

    await reset(server);

    const agents = (await (await fetch(`${server.baseUrl}/api/hub/agents`)).json()) as {
      agents: Array<{ name: string; state: string }>;
    };
    expect(agents.agents.map((a) => [a.name, a.state])).toEqual([
      ["atlas", "running"],
      ["brittle", "failed"],
      ["drifter", "stopped"],
      ["scout", "running"],
    ]);
    expect(
      (await fetch(`${server.baseUrl}/api/agents/atlas/workspace/file?path=gone.md`)).status,
    ).toBe(404);
    expect(
      ((await (await fetch(`${server.baseUrl}/api/hub/secrets`)).json()) as { names: string[] })
        .names,
    ).toEqual(["anthropic_key", "openai_key"]);
    expect(
      ((await (await fetch(`${server.baseUrl}/api/team/workbench/artifacts`)).json()) as unknown[])
        .length,
    ).toBe(1);
    expect((await fetch(`${server.baseUrl}/api/hub/agents/deleted`)).status).toBe(200);
  });

  it("brings back an agent the test deleted, with its socket", async () => {
    const server = await start();
    await fetch(`${server.baseUrl}/api/hub/agents/atlas`, { method: "DELETE" });
    await reset(server);
    expect((await fetch(`${server.baseUrl}/api/agents/atlas/chat/history`)).status).toBe(200);
    const socket = await server.openSocket("/api/agents/atlas/ws");
    socket.send({ type: "ping" });
    await socket.nextOfType("pong");
    await socket.close();
  });

  it("drops every connected page on reset, and they find the initial scenario on reconnecting", async () => {
    const server = await start();
    const hub = await server.openSocket("/api/hub/ws");
    const agent = await server.openSocket("/api/agents/scout/ws");
    await fetch(`${server.baseUrl}/api/hub/agents/drifter/start`, { method: "POST" });
    await reset(server);
    await Promise.all([hub.closed, agent.closed]);

    const again = await server.openSocket("/api/hub/ws");
    const snapshot = await again.nextOfType("agents_snapshot");
    expect((snapshot.agents as Array<{ name: string; state: string }>).map((a) => a.state)).toEqual(
      ["running", "failed", "stopped", "running"],
    );
    await again.close();
  });

  it("announces the same boot id on every run", async () => {
    const server = await start();
    const socket = await server.openSocket("/api/hub/ws");
    const first = await socket.nextOfType("hub_boot");
    await socket.close();
    const other = await startMockServer({ deterministic: true });
    others.push(other);
    const otherSocket = await other.openSocket("/api/hub/ws");
    expect((await otherSocket.nextOfType("hub_boot")).boot_id).toBe(first.boot_id);
    await otherSocket.close();
  });

  it("drops the timers of a turn that was running when it was reset", async () => {
    const server = await start({ delayScale: 0.2 });
    const agent = await server.openSocket("/api/agents/scout/ws");
    agent.send({ type: "send_message", id: "m1", content: "hello" });
    await agent.nextOfType("turn_started");
    await reset(server);
    const hub = await server.openSocket("/api/hub/ws");
    await hub.nextOfType("agents_snapshot");
    // The turn would have ended, and reported scout idle, by now.
    await sleep(500);
    expect(hub.frames.map((frame) => frame.type)).toEqual(["hub_boot", "agents_snapshot"]);
    expect(server.hub.agents.get("scout")?.busySince).toBeNull();
    await hub.close();
  });

  it("answers a request that was waiting on simulated time when the mock was reset with 503", async () => {
    const server = await start({ delayScale: 1 });
    const waiting = fetch(`${server.baseUrl}/api/hub/agents/drifter/start`, { method: "POST" });
    await sleep(50);
    await reset(server);
    const res = await waiting;
    expect(res.status).toBe(503);
    expect(await res.json()).toEqual({ error: "the mock was reset" });
  });

  it("changes how long simulated work takes through the delay control, until the next reset", async () => {
    const server = await start();
    const set = (scale: number): Promise<Response> =>
      fetch(`${server.baseUrl}/api/mock/delays`, {
        method: "POST",
        body: JSON.stringify({ scale }),
      });
    expect((await set(0.5)).status).toBe(200);
    expect(server.hub.env.delayScale()).toBe(0.5);
    expect((await set(-1)).status).toBe(422);
    await reset(server);
    expect(server.hub.env.delayScale()).toBe(0);
  });

  it("stamps the start of a turn, and closes the stopping window, without waiting", async () => {
    const server = await start();
    const hub = await server.openSocket("/api/hub/ws");
    await hub.nextOfType("agents_snapshot");
    const agent = await server.openSocket("/api/agents/scout/ws");
    agent.send({ type: "send_message", id: "m1", content: "hello" });
    expect(await hub.next((f) => f.type === "agent_activity" && f.busy === true)).toMatchObject({
      name: "scout",
      busy_since: "2026-03-14T12:00:00.000Z",
    });
    await agent.nextOfType("turn_ended");

    await fetch(`${server.baseUrl}/api/hub/agents/atlas/stop`, { method: "POST" });
    await hub.next(
      (f) => f.type === "agent_state" && (f.agent as { name: string }).name === "atlas",
    );
    const types = hub.frames.map((f) => f.type);
    expect(types).toContain("agent_stopping");
    expect(types.indexOf("agent_stopping")).toBeLessThan(types.lastIndexOf("agent_state"));
    await agent.close();
    await hub.close();
  });

  it("refuses a clock advance that isn't a number of milliseconds", async () => {
    const server = await start();
    for (const ms of ["soon", -5, null]) {
      const res = await fetch(`${server.baseUrl}/api/mock/clock/advance`, {
        method: "POST",
        body: JSON.stringify({ ms }),
      });
      expect(res.status).toBe(422);
    }
  });
});
